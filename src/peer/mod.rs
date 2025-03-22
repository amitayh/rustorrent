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
use event_handler::HandshakeOrder;
pub use peer_id::*;

use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::PathBuf;
use std::time::Duration;
use std::{io::Result, net::SocketAddr};

use bit_set::BitSet;
use log::info;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::time::{self, Instant, Interval};

use crate::message::Handshake;
use crate::peer::connection::Command;
use crate::peer::connection::Connection;
pub use crate::peer::event::Event;
use crate::peer::event_handler::Action;
use crate::peer::event_handler::EventHandler;
use crate::torrent::Info;

pub struct Peer {
    listener: TcpListener,
    torrent_info: Info,
    file_path: PathBuf,
    peers: HashMap<SocketAddr, Sender<Command>>,
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
            event_handler,
            tx: tx.clone(),
            rx,
            handshake,
            config,
        };
        (peer, tx)
    }

    #[allow(dead_code)]
    fn address(&self) -> Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        let mut keep_alive = interval_with_delay(self.config.keep_alive_interval);
        let mut choke = interval_with_delay(self.config.choking_interval);
        loop {
            let event = tokio::select! {
                // TODO: disconnect idle peers
                _ = keep_alive.tick() => Event::KeepAliveTick,
                _ = choke.tick() => Event::ChokeTick,
                Some(event) = self.rx.recv() => event,
                Ok((socket, addr)) = self.listener.accept() => {
                    info!("got new connection from {}", &addr);
                    self.accept_connection(addr, socket).await?;
                    Event::AcceptConnection(addr)
                }
            };

            for action in self.event_handler.handle(event) {
                match action {
                    Action::Connect(addr) => {
                        self.connect(addr).await?;
                    }
                    Action::Handshake(addr, order) => {
                        //let peer = self.peers.get(&addr).expect("invalid peer");
                        //peer.send(Command::Handshake).await?;
                    }
                    Action::Send(addr, message) => {
                        let peer = self.peers.get(&addr).expect("invalid peer");
                        peer.send(Command::Send(message)).await?;
                    }
                    Action::Broadcast(message) => {
                        for peer in self.peers.values() {
                            peer.send(Command::Send(message.clone())).await?;
                        }
                    }
                    Action::Upload(addr, block) => {
                        let peer = self.peers.get(&addr).expect("invalid peer");
                        peer.send(Command::Upload(block)).await?;
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
                }
            }
        }
    }

    async fn connect(&mut self, addr: SocketAddr) -> Result<()> {
        let (rx, tx) = mpsc::channel(16);
        self.peers.insert(addr, rx);
        let handshake = self.handshake.clone();
        let tx2 = self.tx.clone();
        let file_path = self.file_path.clone();
        let piece_length = self.torrent_info.piece_length;
        tokio::spawn(async move {
            let socket = TcpStream::connect(addr).await?;
            let mut connection =
                Connection::new(addr, socket, file_path, piece_length, tx2.clone(), tx);
            connection.send(&handshake).await?;
            connection.wait_for_handshake().await?;
            connection.start().await?;
            info!("peer {} disconnected", addr);
            tx2.send(Event::Disconnect(addr)).await?;
            Ok::<(), anyhow::Error>(())
        });
        Ok(())
    }

    async fn accept_connection(&mut self, addr: SocketAddr, socket: TcpStream) -> Result<()> {
        let (rx, tx) = mpsc::channel(16);
        let mut connection = Connection::new(
            addr,
            socket,
            self.file_path.clone(),
            self.torrent_info.piece_length,
            self.tx.clone(),
            tx,
        );
        self.peers.insert(addr, rx);
        let handshake = self.handshake.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            connection.wait_for_handshake().await?;
            connection.send(&handshake).await?;
            connection.start().await?;
            info!("peer {} disconnected", addr);
            tx.send(Event::Disconnect(addr)).await?;
            Ok::<(), anyhow::Error>(())
        });
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
            Peer::new(
                listener,
                torrent_info,
                "/tmp/foo".into(),
                Config::default(),
                false,
            )
            .await
        };

        let mut set = JoinSet::new();
        set.spawn(async move { seeder.start().await });
        set.spawn(async move {
            leecher_tx.send(Event::Connect(seeder_addr)).await.unwrap();
            leecher.start().await
        });

        let result = timeout(Duration::from_secs(20), set.join_all()).await;

        // assert leecher has file
        assert!(result.is_ok());
    }
}
