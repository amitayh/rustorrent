mod request;
mod response;

use std::io::{Error, ErrorKind};

use anyhow::{Result, anyhow};
use log::debug;
use log::info;
use size::Size;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use tokio_util::io::StreamReader;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::bencoding::Value;
use crate::codec::AsyncDecoder;
use crate::peer;
use crate::peer::PeerId;
use crate::torrent::Torrent;
use crate::tracker::request::{ResponseMode, TrackerRequest};
use crate::tracker::response::TrackerResponse;

pub struct Tracker {
    tx: watch::Sender<DownloadProgress>,
    join_handle: JoinHandle<anyhow::Result<()>>,
    cancellation_token: CancellationToken,
}

impl Tracker {
    // TODO: change all clones to Arc?
    pub fn spawn(torrent: &Torrent, config: &Config, events_tx: mpsc::Sender<peer::Event>) -> Self {
        let (tx, mut rx) =
            watch::channel(DownloadProgress::new(torrent.info.download_type.length()));
        let cancellation_token = CancellationToken::new();
        let token_clone = cancellation_token.clone();
        let tracker_url = torrent.announce.clone();
        let info_hash = torrent.info.info_hash.clone();
        let peer_id = config.client_id.clone();
        let port = config.port;
        let join_handle = tokio::spawn(async move {
            let mut event = Some(request::Event::Started);
            let mut running = true;

            while running {
                let request = {
                    let download_progress = rx.borrow_and_update();
                    TrackerRequest {
                        announce: tracker_url.clone(),
                        info_hash: info_hash.clone(),
                        peer_id: peer_id.clone(),
                        port,
                        uploaded: download_progress.uploaded,
                        downloaded: download_progress.downloaded,
                        left: download_progress.left(),
                        mode: ResponseMode::Compact,
                        event,
                    }
                };

                info!("refreshing list of peers...");
                debug!("request to tracker: {:?}", &request);
                let response = send_request(request).await?;
                info!("got {} peers from tracker", response.peers.len());
                for peer in response.peers {
                    events_tx.send(peer::Event::Connect(peer.address)).await?;
                }

                event = None;
                tokio::select! {
                    _ = tokio::time::sleep(response.interval) => (),
                    _ = token_clone.cancelled() => {
                        info!("tracker shutting down...");
                        running = false;
                    }
                }
            }
            //tracker::request(&torrent, &config, Some(tracker::Event::Stopped)).await
            Ok(())
        });
        Self {
            tx,
            join_handle,
            cancellation_token,
        }
    }

    pub fn update_progress(&self, downloaded: Size, uploaded: Size) -> anyhow::Result<()> {
        let updated_progress = self.tx.borrow().update(downloaded, uploaded);
        self.tx.send(updated_progress)?;
        Ok(())
    }

    pub async fn shutdown(self) -> anyhow::Result<()> {
        self.cancellation_token.cancel();
        self.join_handle.await??;
        Ok(())
    }
}

#[derive(Clone)]
struct DownloadProgress {
    total: Size,
    uploaded: Size,
    downloaded: Size,
}

impl DownloadProgress {
    fn new(total: Size) -> Self {
        Self {
            total,
            downloaded: Size::from_bytes(0),
            uploaded: Size::from_bytes(0),
        }
    }

    fn left(&self) -> Size {
        self.total - self.downloaded
    }

    fn update(&self, downloaded: Size, uploaded: Size) -> Self {
        Self {
            total: self.total,
            downloaded,
            uploaded,
        }
    }
}

pub struct Config {
    pub client_id: PeerId,
    pub port: u16,
}

async fn send_request(request: TrackerRequest) -> Result<TrackerResponse> {
    let response = reqwest::get(Url::from(request)).await?;
    if !response.status().is_success() {
        return Err(anyhow!("server returned status {}", response.status()));
    }
    let stream = response
        .bytes_stream()
        .map(|bytes| bytes.map_err(|err| Error::new(ErrorKind::Other, err)));
    let mut reader = StreamReader::new(stream);
    let value = Value::decode(&mut reader).await?;
    let result = TrackerResponse::try_from(value)?;
    Ok(result)
}

/*
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
        client_id: PeerId(PEER_ID),
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
*/
