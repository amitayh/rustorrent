mod request;
mod response;

use std::io::{Error, ErrorKind};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use log::error;
use log::info;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use tokio_util::io::StreamReader;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::bencoding::Value;
use crate::client::Download;
use crate::codec::AsyncDecoder;
use crate::event::Event;
use crate::tracker::request::{Mode, TrackerRequest};
use crate::tracker::response::TrackerResponse;

pub struct Tracker {
    tx: watch::Sender<DownloadProgress>,
    join_handle: JoinHandle<anyhow::Result<()>>,
    cancellation_token: CancellationToken,
}

impl Tracker {
    pub fn spawn(download: Arc<Download>, events_tx: mpsc::Sender<Event>) -> Self {
        let (tx, mut rx) =
            watch::channel(DownloadProgress::new(download.torrent.info.total_size()));
        let cancellation_token = CancellationToken::new();
        let token_clone = cancellation_token.clone();
        let join_handle = tokio::spawn(async move {
            let mut event = Some(request::Event::Started);
            let mut tracker_id = None;
            let mut running = true;

            while running {
                let request = {
                    let download_progress = rx.borrow_and_update();
                    TrackerRequest {
                        announce: download.torrent.announce.clone(),
                        info_hash: download.torrent.info.info_hash.clone(),
                        peer_id: download.config.client_id.clone(),
                        port: download.config.port,
                        uploaded: download_progress.uploaded,
                        downloaded: download_progress.downloaded,
                        left: download_progress.left(),
                        mode: Mode::Compact,
                        event,
                        tracker_id: tracker_id.clone(),
                    }
                };

                info!("refreshing list of peers...");
                let mut response = send_request(request).await?;
                info!("got {} peers from tracker", response.peers.len());
                for peer in response.peers {
                    events_tx
                        .send(Event::ConnectionRequested(peer.address))
                        .await?;
                }

                event = None;
                if let Some(new_id) = response.tracker_id.take() {
                    tracker_id = Some(new_id);
                }

                tokio::select! {
                    _ = tokio::time::sleep(response.interval) => (),
                    _ = token_clone.cancelled() => {
                        info!("tracker shutting down...");
                        // Send "stopped" event before shutting down
                        let request = {
                            let download_progress = rx.borrow_and_update();
                            TrackerRequest {
                                announce: download.torrent.announce.clone(),
                                info_hash: download.torrent.info.info_hash.clone(),
                                peer_id: download.config.client_id.clone(),
                                port: download.config.port,
                                uploaded: download_progress.uploaded,
                                downloaded: download_progress.downloaded,
                                left: download_progress.left(),
                                mode: Mode::Compact,
                                event: Some(request::Event::Stopped),
                                tracker_id: tracker_id.clone(),
                            }
                        };
                        if let Err(err) = send_request(request).await {
                            error!("error when shutting down tracker: {}", err);
                        }
                        running = false;
                    }
                }
            }
            Ok(())
        });
        Self {
            tx,
            join_handle,
            cancellation_token,
        }
    }

    pub fn update_progress(&self, downloaded: usize, uploaded: usize) -> anyhow::Result<()> {
        self.tx.send_modify(|progress| {
            progress.downloaded = downloaded;
            progress.uploaded = uploaded;
        });
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
    total: usize,
    uploaded: usize,
    downloaded: usize,
}

impl DownloadProgress {
    fn new(total: usize) -> Self {
        Self {
            total,
            downloaded: 0,
            uploaded: 0,
        }
    }

    fn left(&self) -> usize {
        self.total - self.downloaded
    }
}

async fn send_request(request: TrackerRequest) -> Result<TrackerResponse> {
    let url = Url::from(request);
    info!("sending request to tracker {}", &url);
    let response = reqwest::get(url).await?;
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
