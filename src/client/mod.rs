mod config;
mod download;
mod notification;
mod timers;

pub use config::*;
pub use download::*;
pub use notification::*;
use timers::Timers;

use std::sync::Arc;

use bit_set::BitSet;
use log::trace;
use tokio::fs::OpenOptions;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio_util::sync::CancellationToken;

use crate::command::{CommandExecutor, ExecutionResult};
use crate::core::GracefulShutdown;
use crate::event::{Event, EventHandler};

pub struct Client {
    pub notifications: Receiver<Notification>,
    graceful_shutdown: GracefulShutdown,
}

impl Client {
    pub async fn spawn(listener: TcpListener, download: Arc<Download>, seeding: bool) -> Self {
        let has_pieces = {
            let total_pieces = download.torrent.info.total_pieces();
            if seeding {
                BitSet::from_iter(0..total_pieces)
            } else {
                BitSet::with_capacity(total_pieces)
            }
        };
        if !seeding {
            create_empty_file(&download).await.unwrap();
        }

        let (tx, rx) = mpsc::channel(download.config.channel_buffer);
        let cancellation_token = CancellationToken::new();
        let token_clone = cancellation_token.clone();
        let join_handle =
            tokio::spawn(async move { run(listener, download, has_pieces, tx, token_clone).await });
        let graceful_shutdown = GracefulShutdown::new(join_handle, cancellation_token);

        Self {
            notifications: rx,
            graceful_shutdown,
        }
    }

    pub async fn shutdown(self) -> anyhow::Result<()> {
        self.graceful_shutdown.shutdown().await
    }
}

async fn run(
    listener: TcpListener,
    download: Arc<Download>,
    has_pieces: BitSet,
    notificaitons: Sender<Notification>,
    cancellation_token: CancellationToken,
) {
    let (tx, mut rx) = mpsc::channel(download.config.events_buffer);
    let mut handler = EventHandler::new(Arc::clone(&download), has_pieces);
    let mut executor = CommandExecutor::new(Arc::clone(&download), tx, notificaitons);
    let mut timers = Timers::new(&download.config);

    let mut running = true;
    while running {
        let event = tokio::select! {
            _ = cancellation_token.cancelled() => Event::ShutdownRequested,
            event = timers.tick() => event,
            Some(event) = rx.recv() => event,
            Ok((socket, addr)) = listener.accept() => {
                Event::ConnectionAccepted(addr, socket)
            }
        };
        trace!("handling event: {:?}", &event);
        for command in handler.handle(event) {
            trace!("command to perform: {:?}", &command);
            if executor.execute(command) == ExecutionResult::Stop {
                running = false;
            }
        }
    }

    executor.shutdown().await;
}

async fn create_empty_file(download: &Download) -> anyhow::Result<()> {
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&download.config.download_path)
        .await?;

    file.set_len(download.torrent.info.total_size() as u64)
        .await?;

    Ok(())
}

#[cfg(test)]
pub mod tests {
    use std::{net::SocketAddr, time::Duration};
    use tokio::io::AsyncReadExt;

    use url::Url;
    use wiremock::{
        Mock, MockServer, Request, ResponseTemplate,
        matchers::{method, path},
    };

    use crate::{
        bencoding::Value,
        core::{Encoder, Md5, Sha1},
        torrent::{DownloadType, Info, Torrent},
    };

    use super::*;

    pub fn test_config(download_path: &str) -> Config {
        Config::new(download_path.into())
            .with_keep_alive_interval(Duration::from_secs(5))
            .with_unchoking_interval(Duration::from_secs(1))
            .with_optimistic_unchoking_cycle(2)
    }

    // TODO: remove duplication
    pub fn create_download() -> Download {
        let torrent = test_torrent();
        let config = Config::new("/tmp".into());
        Download { torrent, config }
    }

    pub fn test_torrent() -> Torrent {
        test_torrent_with_announce_url("https://foo.bar".try_into().unwrap())
    }

    fn test_torrent_with_announce_url(announce_url: Url) -> Torrent {
        Torrent {
            announce: announce_url,
            info: Info {
                info_hash: Sha1::from_hex("e90cf5ec83e174d7dcb94821560dac201ae1f663").unwrap(),
                piece_size: 1024 * 32,
                pieces: vec![
                    Sha1::from_hex("35d2d43083c603e8afdf9762a6bb29ad6e12d216").unwrap(),
                    Sha1::from_hex("e0abb4bae6a0bd01fa554138a83450371d552917").unwrap(),
                    Sha1::from_hex("6bf99993625de463dd7c2c584c7c590767762058").unwrap(),
                    Sha1::from_hex("0656e1ab2f59bf11f0c11fcbd3c0cab87326ab53").unwrap(),
                    Sha1::from_hex("5be107ddea900b7710f619beafe81a14164d3dee").unwrap(),
                    Sha1::from_hex("08a840f1713e297bf07c077fd76e5476ed9b9edd").unwrap(),
                ],
                download_type: DownloadType::SingleFile {
                    name: "alice_in_wonderland.txt".to_string(),
                    size: 170597,
                    md5sum: Some(Md5::from_hex("1399df3245f0d70f4b25d4ae748f008e").unwrap()),
                },
            },
        }
    }

    async fn mock_tracker(peers: &[SocketAddr]) -> Url {
        let mock_tracker = MockServer::start().await;
        let peers = Value::List(peers.iter().map(peer_entry).collect());
        Mock::given(method("GET"))
            .and(path("/announce"))
            .respond_with(move |_: &Request| {
                let response = Value::dictionary()
                    .with_entry("complete", Value::Integer(1))
                    .with_entry("incomplete", Value::Integer(1))
                    .with_entry("interval", Value::Integer(600))
                    .with_entry("peers", peers.clone());

                let mut responses_bytes = Vec::new();
                response.encode(&mut responses_bytes).unwrap();

                ResponseTemplate::new(200).set_body_bytes(responses_bytes)
            })
            .mount(&mock_tracker)
            .await;

        Url::parse(&format!("{}/announce", mock_tracker.uri())).unwrap()
    }

    fn peer_entry(addr: &SocketAddr) -> Value {
        Value::dictionary()
            .with_entry("ip", Value::string(&addr.ip().to_string()))
            .with_entry("port", Value::Integer(addr.port().into()))
    }

    #[tokio::test]
    async fn one_seeder_one_leecher() {
        let _ = env_logger::try_init();

        let seeder_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let leecher_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

        let seeder_addr = seeder_listener.local_addr().unwrap();
        let announce_url = mock_tracker(&[seeder_addr]).await;

        let source_path = "assets/alice_in_wonderland.txt";
        let seeder_handle = Client::spawn(
            seeder_listener,
            Arc::new(Download {
                torrent: test_torrent_with_announce_url(announce_url.clone()),
                config: test_config(source_path).with_unchoking_interval(Duration::from_secs(2)),
            }),
            true,
        )
        .await;

        let download_path = "/tmp/download.txt";
        let mut leecher_handle = Client::spawn(
            leecher_listener,
            Arc::new(Download {
                torrent: test_torrent_with_announce_url(announce_url),
                config: test_config(download_path),
            }),
            false,
        )
        .await;

        loop {
            // Wait for leecher to finish downloading
            let notification = leecher_handle.notifications.recv().await;
            if let Some(Notification::DownloadComplete) = notification {
                break;
            }
        }

        // Shutdown the swarm
        seeder_handle.shutdown().await.unwrap();
        leecher_handle.shutdown().await.unwrap();

        // Verify the leecher downloaded the file by comparing MD5 checksums
        let source_md5 = md5sum(source_path).await;
        let downloaded_md5 = md5sum(download_path).await;
        tokio::fs::remove_file(download_path).await.unwrap();
        assert_eq!(source_md5, downloaded_md5);
    }

    async fn md5sum(path: &str) -> md5::Digest {
        let mut file = tokio::fs::File::open(path)
            .await
            .expect("Failed to open file");
        let mut md5 = md5::Context::new();
        let mut buf = [0u8; 4096];

        loop {
            let len = file.read(&mut buf).await.expect("Failed to read file");
            if len == 0 {
                break;
            }
            md5.consume(&buf[..len]);
        }

        md5.compute()
    }
}
