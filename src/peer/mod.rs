mod blocks;
mod choke;
mod config;
mod connection;
mod download;
mod event;
mod event_handler;
mod fs;
mod peer_id;
mod piece;
mod scheduler;
mod stats;
mod sweeper;
mod transfer_rate;

pub use config::*;
pub use download::*;
pub use event::Event;
use fs::FileReaderWriter;
pub use peer_id::*;
use tokio::sync::Mutex;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use bit_set::BitSet;
use log::{info, trace, warn};
use tokio::fs::OpenOptions;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::task::JoinSet;
use tokio::time::{self, Instant, Interval, timeout};

use crate::peer::connection::Connection;
use crate::peer::event_handler::Action;
use crate::peer::event_handler::EventHandler;
use crate::tracker::Tracker;

pub struct Peer {
    listener: TcpListener,
    download: Arc<Download>,
    peers: HashMap<SocketAddr, Connection>,
    event_handler: EventHandler,
    file_reader_writer: Arc<Mutex<FileReaderWriter>>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
}

impl Peer {
    pub async fn new(listener: TcpListener, download: Arc<Download>, seeding: bool) -> Self {
        let total_pieces = download.torrent.info.pieces.len();
        let has_pieces = if seeding {
            BitSet::from_iter(0..total_pieces)
        } else {
            BitSet::with_capacity(total_pieces)
        };
        let event_handler = EventHandler::new(Arc::clone(&download), has_pieces);
        let file_reader_writer = Arc::new(Mutex::new(FileReaderWriter::new(Arc::clone(&download))));

        if !seeding {
            let file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&download.config.download_path)
                .await
                .unwrap();

            file.set_len(download.torrent.info.total_size() as u64)
                .await
                .unwrap();
        }

        let (tx, rx) = mpsc::channel(1024);
        Self {
            listener,
            download,
            peers: HashMap::new(),
            event_handler,
            file_reader_writer,
            tx: tx.clone(),
            rx,
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        let config = &self.download.config;
        let mut keep_alive = interval_with_delay(config.keep_alive_interval);
        let mut choke = interval_with_delay(config.choking_interval);
        let mut sweep = interval_with_delay(config.sweep_interval);
        let mut stats = interval_with_delay(config.update_stats_interval);

        let tracker = Tracker::spawn(Arc::clone(&self.download), self.tx.clone());

        let mut running = true;
        while running {
            let event = tokio::select! {
                _ = tokio::signal::ctrl_c() => Event::Shutdown,
                _ = keep_alive.tick() => Event::KeepAliveTick,
                _ = choke.tick() => Event::ChokeTick,
                _ = stats.tick() => Event::StatsTick,
                now = sweep.tick() => Event::SweepTick(now),
                Some(event) = self.rx.recv() => event,
                Ok((socket, addr)) = self.listener.accept() => {
                    Event::AcceptConnection(addr, socket)
                }
            };

            for action in self.event_handler.handle(event) {
                trace!("action to perform: {:?}", &action);
                match action {
                    Action::EstablishConnection(addr, socket)
                        if !self.peers.contains_key(&addr) =>
                    {
                        self.peers.insert(
                            addr,
                            Connection::spawn(
                                addr,
                                socket,
                                self.tx.clone(),
                                Arc::clone(&self.download),
                            ),
                        );
                    }

                    Action::Send(addr, message) => {
                        let peer = self.peers.get(&addr).expect("invalid peer");
                        peer.send(message);
                    }

                    Action::Broadcast(message) => {
                        for peer in self.peers.values() {
                            peer.send(message.clone());
                        }
                    }

                    Action::Upload(addr, block) => {
                        let peer = self.peers.get(&addr).expect("invalid peer");
                        let file_reader_writer = Arc::clone(&self.file_reader_writer);
                        let tx = peer.tx.clone();
                        tokio::spawn(async move {
                            file_reader_writer.lock().await.read(block, tx).await
                        });
                    }

                    Action::IntegrateBlock(block_data) => {
                        let file_reader_writer = Arc::clone(&self.file_reader_writer);
                        let tx = self.tx.clone();
                        tokio::spawn(async move {
                            file_reader_writer.lock().await.write(block_data, tx).await
                        });
                    }

                    Action::RemovePeer(addr) => {
                        let peer = self.peers.remove(&addr).expect("invalid peer");
                        peer.abort();
                    }

                    Action::UpdateStats(stats) => {
                        tracker.update_progress(stats.downloaded, stats.uploaded)?;
                        println!(
                            "Downloaded {}/{} pieces ({:.2}%) | Down: {} | Up: {} | Peers: {}",
                            stats.completed_pieces,
                            stats.total_pieces,
                            stats.completed(),
                            stats.download_rate,
                            stats.upload_rate,
                            stats.connected_peers
                        );
                    }

                    Action::Shutdown => {
                        running = false;
                    }

                    action => warn!("unhandled action: {:?}", action),
                }
            }
        }

        info!("shutting down...");
        if timeout(self.download.config.shutdown_timeout, self.shutdown())
            .await
            .is_err()
        {
            warn!("timeout for graceful shutdown expired");
        }
        tracker.shutdown().await?;
        info!("done");

        Ok(())
    }

    async fn shutdown(&mut self) -> Result<()> {
        let mut join_set = JoinSet::new();
        for (_, peer) in self.peers.drain() {
            join_set.spawn(async move { peer.shutdown().await });
        }
        join_set.join_all().await;
        Ok(())
    }
}

fn interval_with_delay(period: Duration) -> Interval {
    let start = Instant::now() + period;
    time::interval_at(start, period)
}

#[cfg(test)]
mod tests {

    use std::time::Duration;

    use tokio::{task::JoinSet, time::timeout};
    use url::Url;
    use wiremock::{
        Mock, MockServer, Request, ResponseTemplate,
        matchers::{method, path},
    };

    use crate::{
        bencoding::Value,
        codec::Encoder,
        crypto::{Md5, Sha1},
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
                    Sha1::from_hex("8fdfb566405fc084761b1fe0b6b7f8c6a37234ed").unwrap(),
                    Sha1::from_hex("2494039151d7db3e56b3ec021d233742e3de55a6").unwrap(),
                    Sha1::from_hex("af99be061f2c5eee12374055cf1a81909d276db5").unwrap(),
                    Sha1::from_hex("3c12e1fcba504fedc13ee17ea76b62901dc8c9f7").unwrap(),
                    Sha1::from_hex("d5facb89cbdc2e3ed1a1cd1050e217ec534f1fad").unwrap(),
                    Sha1::from_hex("d5d2b296f52ab11791aad35a7d493833d39c6786").unwrap(),
                ],
                download_type: DownloadType::SingleFile {
                    name: "alice_in_wonderland.txt".to_string(),
                    size: 174357,
                    md5sum: Some(Md5::from_hex("9a930de3cfc64468c05715237a6b4061").unwrap()),
                },
            },
        }
    }

    fn peer_entry(addr: &SocketAddr) -> Value {
        Value::dictionary()
            .with_entry("ip", Value::string(&addr.ip().to_string()))
            .with_entry("port", Value::Integer(addr.port().into()))
    }

    #[tokio::test]
    async fn one_seeder_one_leecher() {
        env_logger::init();

        let seeder_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let leecher_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let leecher2_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

        let seeder_addr = seeder_listener.local_addr().unwrap();
        let leecher1_addr = seeder_listener.local_addr().unwrap();
        let leecher2_addr = seeder_listener.local_addr().unwrap();
        let mock_tracker = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/announce"))
            .respond_with(move |_: &Request| {
                let response = Value::dictionary()
                    .with_entry("complete", Value::Integer(1))
                    .with_entry("incomplete", Value::Integer(2))
                    .with_entry("interval", Value::Integer(600))
                    .with_entry(
                        "peers",
                        Value::list()
                            .with_value(peer_entry(&seeder_addr))
                            .with_value(peer_entry(&leecher1_addr))
                            .with_value(peer_entry(&leecher2_addr)),
                    );

                let mut responses_bytes = Vec::new();
                response.encode(&mut responses_bytes).unwrap();

                ResponseTemplate::new(200).set_body_bytes(responses_bytes)
            })
            .mount(&mock_tracker)
            .await;

        let announce_url = Url::parse(&format!("{}/announce", mock_tracker.uri())).unwrap();

        let mut seeder = Peer::new(
            seeder_listener,
            Arc::new(Download {
                torrent: test_torrent_with_announce_url(announce_url.clone()),
                config: test_config("assets/alice_in_wonderland.txt")
                    .with_unchoking_interval(Duration::from_secs(2)),
            }),
            true,
        )
        .await;

        let mut leecher = Peer::new(
            leecher_listener,
            Arc::new(Download {
                torrent: test_torrent_with_announce_url(announce_url.clone()),
                config: test_config("/tmp/foo"),
            }),
            false,
        )
        .await;

        let mut leecher2 = Peer::new(
            leecher2_listener,
            Arc::new(Download {
                torrent: test_torrent_with_announce_url(announce_url),
                config: test_config("/tmp/bar"),
            }),
            false,
        )
        .await;

        let mut set = JoinSet::new();
        set.spawn(async move { seeder.start().await });
        set.spawn(async move { leecher.start().await });
        set.spawn(async move { leecher2.start().await });

        let result = timeout(Duration::from_secs(120), set.join_all()).await;

        dbg!(&result);

        // assert leecher has file
        assert!(result.is_ok());
    }
}
