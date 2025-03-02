use std::io::Write;

use anyhow::{Result, anyhow};
use url::Url;
use url::form_urlencoded::byte_serialize;

use crate::{bencoding::parser::Parser, crypto::Sha1, torrent::Torrent};

use self::response::TrackerResponse;

pub mod response;

const PEER_ID: &str = "rustorrent-v0.1-----";
const PORT: u16 = 6881;

impl Sha1 {
    fn url_encoded(&self) -> String {
        String::from_iter(byte_serialize(&self.0))
    }
}

pub enum Event {
    Started,
    Stopped,
    Completed,
}

impl From<&Event> for &str {
    fn from(value: &Event) -> Self {
        match value {
            Event::Started => "started",
            Event::Stopped => "stopped",
            Event::Completed => "completed",
        }
    }
}

fn request_url(torrent: &Torrent, event: Option<Event>) -> Url {
    let mut url = torrent.announce.clone();
    let mut query = format!(
        "info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&left={}",
        torrent.info.info_hash.url_encoded(),
        PEER_ID,
        PORT,
        0,
        0,
        torrent.info.download_type.length().bytes()
    );
    if let Some(event) = &event {
        query.push_str("&event=");
        query.push_str(event.into());
    }
    // TODO: preseve old query if exists
    url.set_query(Some(&query));
    dbg!(&url);
    url
}

pub async fn request(torrent: &Torrent, event: Option<Event>) -> Result<TrackerResponse> {
    let mut response = reqwest::get(request_url(torrent, event)).await?;
    if !response.status().is_success() {
        return Err(anyhow!("server returned status {}", response.status()));
    }
    let value = {
        let mut parser = Parser::new();
        while let Some(chunk) = response.chunk().await? {
            parser.write_all(&chunk)?;
        }
        parser.result()?
    };
    TrackerResponse::try_from(value)
}

#[cfg(test)]
mod tests {
    use crate::torrent::{DownloadType, Info};

    use super::*;

    use size::Size;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    #[tokio::test]
    #[ignore]
    async fn test() {
        let mock_tracker = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/announce"))
            //.and(query_param(
            //    "info_hash",
            //    "%01%02%03%04%05%06%07%08%09%0A%0B%0C%0D%0E%0F%10%11%12%13%14",
            //))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw("5:hello", "application/octet-stream"),
            )
            .mount(&mock_tracker)
            .await;

        let announce_url = format!("{}/announce", mock_tracker.uri());
        let info_hash = hex::decode("c47d187067c6cf953245f128b5fde62a3b8fa3b0").unwrap();
        let info_hash = Sha1(info_hash.try_into().unwrap());
        let torrent = Torrent {
            announce: Url::parse(&announce_url).unwrap(),
            info: Info {
                info_hash,
                piece_length: Size::from_bytes(1234),
                pieces: Vec::new(),
                download_type: DownloadType::SingleFile {
                    name: "foo".to_string(),
                    length: Size::from_bytes(1234),
                    md5sum: None,
                },
            },
        };

        let result = request(&torrent, None)
            .await
            .expect("failed to contact tracker");

        dbg!(&result);

        //assert_eq!(result, Value::string("hello"));
    }
}
