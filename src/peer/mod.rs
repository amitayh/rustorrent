mod blocks;
mod choke;
mod config;
mod connection;
mod event;
mod event_handler;
mod fs;
mod peer_id;
mod piece;
mod sizes;
mod stats;
mod sweeper;
mod transfer_rate;

pub use config::*;
pub use event::Event;
use fs::FileReaderWriter;
pub use peer_id::*;
use sizes::Sizes;
use tokio::sync::Mutex;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use bit_set::BitSet;
use log::{info, warn};
use tokio::fs::OpenOptions;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::task::JoinSet;
use tokio::time::{self, Instant, Interval, timeout};

use crate::peer::connection::Connection;
use crate::peer::event_handler::Action;
use crate::peer::event_handler::EventHandler;
use crate::torrent::Torrent;
use crate::tracker::{self, Tracker};

pub struct Peer {
    listener: TcpListener,
    torrent: Torrent,
    peers: HashMap<SocketAddr, Connection>,
    event_handler: EventHandler,
    file_reader_writer: Arc<Mutex<FileReaderWriter>>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    config: Config,
}

impl Peer {
    pub async fn new(
        listener: TcpListener,
        torrent: Torrent,
        file_path: PathBuf,
        config: Config,
        seeding: bool,
    ) -> Self {
        let torrent_info = &torrent.info;
        let total_pieces = torrent_info.pieces.len();
        let has_pieces = if seeding {
            BitSet::from_iter(0..total_pieces)
        } else {
            BitSet::with_capacity(total_pieces)
        };
        let event_handler = EventHandler::new(torrent_info.clone(), config.clone(), has_pieces);
        let file_reader_writer = {
            let sizes = Sizes::new(
                torrent_info.piece_length,
                torrent_info.download_type.length(),
                config.block_size,
            );
            Arc::new(Mutex::new(FileReaderWriter::new(
                file_path.clone(),
                &sizes,
                torrent_info.clone(),
            )))
        };

        if !seeding {
            let file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&file_path)
                .await
                .unwrap();

            file.set_len(torrent_info.download_type.length().bytes() as u64)
                .await
                .unwrap();
        }

        let (tx, rx) = mpsc::channel(1024);
        Self {
            listener,
            torrent,
            peers: HashMap::new(),
            event_handler,
            file_reader_writer,
            tx: tx.clone(),
            rx,
            config,
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        let mut keep_alive = interval_with_delay(self.config.keep_alive_interval);
        let mut choke = interval_with_delay(self.config.choking_interval);
        let mut sweep = interval_with_delay(self.config.sweep_interval);

        let tracker = {
            let self_addr = self.listener.local_addr()?;
            let client_id = self.config.clinet_id.clone();
            let config = tracker::Config {
                client_id,
                port: self_addr.port(),
            };
            Tracker::spawn(&self.torrent, &config, self.tx.clone())
        };

        let mut running = true;
        while running {
            let event = tokio::select! {
                _ = tokio::signal::ctrl_c() => Event::Shutdown,
                _ = keep_alive.tick() => Event::KeepAliveTick,
                _ = choke.tick() => Event::ChokeTick,
                now = sweep.tick() => Event::SweepTick(now),
                Some(event) = self.rx.recv() => event,
                Ok((socket, addr)) = self.listener.accept() => {
                    Event::AcceptConnection(addr, socket)
                }
            };

            for action in self.event_handler.handle(event) {
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
                                self.torrent.info.info_hash.clone(),
                                &self.config,
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
                        tracker.update_progress(stats.download_rate.0, stats.upload_rate.0)?;
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
        if timeout(self.config.shutdown_timeout, self.shutdown())
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

    use size::Size;
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

    fn test_config() -> Config {
        Config::default()
            .with_keep_alive_interval(Duration::from_secs(5))
            .with_unchoking_interval(Duration::from_secs(1))
            .with_optimistic_unchoking_cycle(2)
    }

    fn test_torrent(announce_url: Url) -> Torrent {
        Torrent {
            announce: announce_url,
            info: Info {
                info_hash: Sha1::from_hex("e90cf5ec83e174d7dcb94821560dac201ae1f663").unwrap(),
                piece_length: Size::from_kibibytes(32),
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
                    length: Size::from_bytes(174357),
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
            test_torrent(announce_url.clone()),
            "assets/alice_in_wonderland.txt".into(),
            test_config().with_unchoking_interval(Duration::from_secs(2)),
            true,
        )
        .await;

        let mut leecher = Peer::new(
            leecher_listener,
            test_torrent(announce_url.clone()),
            "/tmp/foo".into(),
            test_config(),
            false,
        )
        .await;

        let mut leecher2 = Peer::new(
            leecher2_listener,
            test_torrent(announce_url),
            "/tmp/bar".into(),
            test_config(),
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
