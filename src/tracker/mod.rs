use std::io::Write;

use anyhow::{Result, anyhow};
use log::{info, warn};
use url::Url;
use url::form_urlencoded::byte_serialize;

use crate::tracker::response::TrackerResponse;
use crate::{Config, bencoding::parser::Parser, torrent::Torrent};

pub mod response;

fn url_encode(bytes: &[u8]) -> String {
    String::from_iter(byte_serialize(bytes))
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

fn request_url(torrent: &Torrent, config: &Config, event: Option<Event>) -> Url {
    let mut url = torrent.announce.clone();
    let mut query = format!(
        "info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&left={}",
        url_encode(&torrent.info.info_hash.0),
        url_encode(&config.clinet_id.0),
        config.port,
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
    url
}

pub async fn request(
    torrent: &Torrent,
    config: &Config,
    event: Option<Event>,
) -> Result<TrackerResponse> {
    let url = request_url(torrent, config, event);
    info!("sending request to tracker: {}", &url);
    let mut response = reqwest::get(url).await?;
    if !response.status().is_success() {
        warn!("request to tracker failed {}", response.status());
        return Err(anyhow!("server returned status {}", response.status()));
    }
    let value = {
        let mut parser = Parser::new();
        while let Some(chunk) = response.chunk().await? {
            parser.write_all(&chunk)?;
        }
        parser.result()?
    };
    let result = TrackerResponse::try_from(value)?;
    info!(
        "request to tracker succeeded. got {} peers",
        result.peers.len()
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use crate::{
        crypto::Sha1,
        peer::PeerId,
        torrent::{DownloadType, Info},
    };

    use super::*;

    use size::Size;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param},
    };

    const PEER_ID: [u8; 20] = [0x2d; 20];
    const PORT: u16 = 6881;
    const CONFIG: Config = Config {
        clinet_id: PeerId(PEER_ID),
        port: PORT,
    };

    #[tokio::test]
    async fn http_api() {
        let mock_tracker = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/announce"))
            //.and(query_param("peer_id", "%C4%7D%18pg%C6%CF%952E%F1%28%B5%FD%E6*%3B%8F%A3%B0"))
            .and(query_param("peer_id", "--------------------"))
            .and(query_param("port", "6881"))
            .and(query_param("left", "1234"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                "d8:completei12e10:incompletei34e8:intervali1800e5:peersld2:ip11:12.34.56.78\
                        7:peer id20:-TR3000-47qm0ov7eav44:porti51413eeee",
                "application/octet-stream",
            ))
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

        let result = request(&torrent, &CONFIG, None)
            .await
            .expect("failed to contact tracker");

        assert_eq!(result.peers.len(), 1);
    }
}
