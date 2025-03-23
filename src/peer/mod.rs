mod blocks;
mod choke;
mod config;
mod connection;
mod event;
mod event_handler;
mod peer_id;
mod piece;
mod sizes;
mod transfer_rate;

pub use config::*;
pub use event::Event;
pub use peer_id::*;

use std::collections::HashMap;
use std::io::SeekFrom;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use bit_set::BitSet;
use log::{info, warn};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::task::JoinHandle;
use tokio::time::{self, Instant, Interval, timeout};

use crate::message::{Block, Handshake, Message};
use crate::peer::connection::Command;
use crate::peer::connection::Connection;
use crate::peer::event_handler::Action;
use crate::peer::event_handler::EventHandler;
use crate::torrent::Info;

pub struct Peer {
    listener: TcpListener,
    torrent_info: Info,
    file_path: PathBuf,
    peers: HashMap<SocketAddr, PeerHandle>,
    block_timeouts: HashMap<(SocketAddr, Block), JoinHandle<Result<()>>>,
    event_handler: EventHandler,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    handshake: Handshake,
    config: Config,
}

impl Peer {
    pub async fn new(
        listener: TcpListener,
        torrent_info: Info,
        file_path: PathBuf,
        config: Config,
        seeding: bool,
    ) -> (Self, Sender<Event>) {
        let peer_id = PeerId::random();
        let handshake = Handshake::new(torrent_info.info_hash.clone(), peer_id.clone());
        let total_pieces = torrent_info.pieces.len();
        let has_pieces = if seeding {
            BitSet::from_iter(0..total_pieces)
        } else {
            BitSet::with_capacity(total_pieces)
        };
        let event_handler = EventHandler::new(torrent_info.clone(), config.clone(), has_pieces);

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

        let (tx, rx) = mpsc::channel(128);
        let peer = Self {
            listener,
            torrent_info,
            file_path,
            peers: HashMap::new(),
            block_timeouts: HashMap::new(),
            event_handler,
            tx: tx.clone(),
            rx,
            handshake,
            config,
        };
        (peer, tx)
    }

    #[allow(dead_code)]
    fn address(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub async fn start(&mut self) -> Result<()> {
        let mut keep_alive = interval_with_delay(self.config.keep_alive_interval);
        let mut choke = interval_with_delay(self.config.choking_interval);
        loop {
            let event = tokio::select! {
                // TODO: disconnect idle peers
                _ = tokio::signal::ctrl_c() => break,
                _ = keep_alive.tick() => Event::KeepAliveTick,
                _ = choke.tick() => Event::ChokeTick,
                Some(event) = self.rx.recv() => event,
                Ok((socket, addr)) = self.listener.accept() => {
                    Event::AcceptConnection(addr, socket)
                }
            };

            if let Event::Message(addr, Message::Piece(block_data)) = &event {
                let key = (*addr, block_data.into());
                if let Some(timeout) = self.block_timeouts.remove(&key) {
                    timeout.abort();
                }
            }

            for action in self.event_handler.handle(event) {
                match action {
                    Action::EstablishConnection(addr, socket) => {
                        self.establish_connection(addr, socket);
                    }
                    Action::Send(addr, message) => {
                        let peer = self.peers.get(&addr).expect("invalid peer");
                        if let Message::Request(block) = &message {
                            let timeout = self.config.block_timeout;
                            let tx = self.tx.clone();
                            let block = *block;
                            let join_handle = tokio::spawn(async move {
                                tokio::time::sleep(timeout).await;
                                tx.send(Event::BlockTimeout(addr, block)).await?;
                                Ok(())
                            });
                            self.block_timeouts.insert((addr, block), join_handle);
                        }
                        peer.send(Command::Send(message)).await;
                    }
                    Action::Broadcast(message) => {
                        for peer in self.peers.values() {
                            peer.send(Command::Send(message.clone())).await;
                        }
                    }
                    Action::Upload(addr, block) => {
                        let peer = self.peers.get(&addr).expect("invalid peer");
                        peer.send(Command::Upload(block)).await;
                    }
                    Action::IntegratePiece { offset, data } => {
                        // TODO: run this in a blocking task
                        let mut file = OpenOptions::new()
                            .write(true)
                            .truncate(false)
                            .open(&self.file_path)
                            .await?;
                        file.seek(SeekFrom::Start(offset)).await?;
                        file.write_all(&data).await?;
                    }
                    Action::RemovePeer(addr) => {
                        self.peers.remove(&addr);
                    }
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

        Ok(())
    }

    fn establish_connection(&mut self, addr: SocketAddr, socket: Option<TcpStream>) {
        let (rx, tx) = mpsc::channel(16);

        let event_channel = self.tx.clone();
        let file_path = self.file_path.clone();
        let piece_length = self.torrent_info.piece_length;
        let handshake = self.handshake.clone();
        let join_handle = tokio::spawn(async move {
            let (socket, send_handshake_first) = match socket {
                Some(socket) => (socket, false),
                None => match TcpStream::connect(addr).await {
                    Ok(socket) => (socket, true),
                    Err(err) => {
                        warn!("unable to connect to {}: {}", &addr, err);
                        event_channel.send(Event::Disconnect(addr)).await?;
                        return Ok(());
                    }
                },
            };
            let mut connection = Connection::new(
                addr,
                socket,
                file_path,
                piece_length,
                event_channel.clone(),
                tx,
            );
            if send_handshake_first {
                connection.send(&handshake).await?;
                connection.wait_for_handshake().await?;
            } else {
                connection.wait_for_handshake().await?;
                connection.send(&handshake).await?;
            }
            connection.start().await?;
            info!("peer {} disconnected", addr);
            event_channel.send(Event::Disconnect(addr)).await?;
            Ok(())
        });
        self.peers.insert(addr, PeerHandle { rx, join_handle });
    }

    async fn shutdown(&mut self) -> Result<()> {
        for peer in self.peers.values() {
            let rx = peer.rx.clone();
            tokio::spawn(async move { rx.send(Command::Shutdown).await });
        }
        for (addr, peer) in self.peers.drain() {
            if let Err(err) = peer.join_handle.await? {
                warn!("peer {} existed with error: {}", addr, err);
            }
        }
        Ok(())
    }
}

struct PeerHandle {
    rx: Sender<Command>,
    join_handle: JoinHandle<Result<()>>,
}

impl PeerHandle {
    async fn send(&self, command: Command) {
        if let Err(err) = self.rx.send(command.clone()).await {
            warn!("unable to send command {:?}: {}", command, err);
        }
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

    use super::*;

    #[tokio::test]
    async fn one_seeder_one_leecher() {
        env_logger::init();

        let file_path = "assets/alice_in_wonderland.txt";
        let torrent_info = Info::load(&file_path).await.unwrap();

        let mut seeder = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let config = Config::default()
                .with_keep_alive_interval(Duration::from_secs(5))
                .with_unchoking_interval(Duration::from_secs(1))
                .with_optimistic_unchoking_cycle(2);
            let (peer, _) = Peer::new(
                listener,
                torrent_info.clone(),
                file_path.into(),
                config,
                true,
            )
            .await;
            peer
        };
        let seeder_addr = seeder.address().unwrap();
        let (mut leecher, leecher_tx) = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            // lecher unchokes seeder???
            Peer::new(
                listener,
                torrent_info.clone(),
                "/tmp/foo".into(),
                Config::default(),
                false,
            )
            .await
        };

        //let leecher_addr = leecher.address().unwrap();
        //let (mut leecher2, leecher_tx2) = {
        //    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        //    // lecher unchokes seeder???
        //    Peer::new(
        //        listener,
        //        torrent_info,
        //        "/tmp/bar".into(),
        //        Config::default(),
        //        false,
        //    )
        //    .await
        //};

        let mut set = JoinSet::new();
        set.spawn(async move { seeder.start().await });
        set.spawn(async move {
            leecher_tx.send(Event::Connect(seeder_addr)).await.unwrap();
            leecher.start().await
        });
        //set.spawn(async move {
        //    leecher_tx2.send(Event::Connect(seeder_addr)).await.unwrap();
        //    leecher_tx2
        //        .send(Event::Connect(leecher_addr))
        //        .await
        //        .unwrap();
        //    leecher2.start().await
        //});

        let result = timeout(Duration::from_secs(60), set.join_all()).await;

        // assert leecher has file
        assert!(result.is_ok());
    }
}
