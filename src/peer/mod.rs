mod blocks;
mod choke;
mod config;
mod connection;
mod event_loop;
mod peer_id;
mod piece;
mod sizes;
mod transfer_rate;

pub use config::*;
pub use peer_id::*;

use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::PathBuf;
use std::time::Duration;
use std::{io::Result, net::SocketAddr};

use bit_set::BitSet;
use log::{info, warn};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::time::{self, Instant, Interval};

use crate::message::Handshake;
use crate::message::Message;
use crate::peer::connection::Command;
use crate::peer::connection::Connection;
use crate::peer::event_loop::EventLoop;
use crate::peer::transfer_rate::TransferRate;
use crate::torrent::Info;

pub struct PeerEvent(pub SocketAddr, pub Event);

pub enum Event {
    Connect,
    Disconnected,
    Message(Message, TransferRate),
    //Uploaded(Block, TransferRate),
}

pub struct Peer {
    listener: TcpListener,
    torrent_info: Info,
    file_path: PathBuf,
    peers: HashMap<SocketAddr, Sender<Command>>,
    event_loop: EventLoop,
    tx: Sender<PeerEvent>,
    rx: Receiver<PeerEvent>,
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
    ) -> (Self, Sender<PeerEvent>) {
        let peer_id = PeerId::random();
        let handshake = Handshake::new(torrent_info.info_hash.clone(), peer_id.clone());
        let total_pieces = torrent_info.pieces.len();
        let has_pieces = if seeding {
            BitSet::from_iter(0..total_pieces)
        } else {
            BitSet::with_capacity(total_pieces)
        };
        let event_loop = EventLoop::new(torrent_info.clone(), config.clone(), has_pieces);

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
            event_loop,
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
                _ = keep_alive.tick() => event_loop::Event::KeepAliveTick,
                _ = choke.tick() => event_loop::Event::ChokeTick,
                Some(PeerEvent(addr, event)) = self.rx.recv() => {
                    match event {
                        Event::Message(message, _transfer_rate) => {
                            event_loop::Event::Message(addr, message)
                        }
                        Event::Connect => {
                            info!("connecting to {}", addr);
                            match TcpStream::connect(addr).await {
                                Ok(socket) => self.new_peer(addr, socket, true).await?,
                                Err(err) => warn!("error connecting to {}: {}", addr, err),
                            }
                            event_loop::Event::Connect(addr)
                        }
                        Event::Disconnected => event_loop::Event::Disconnect(addr),
                    }
                }
                Ok((socket, addr)) = self.listener.accept() => {
                    info!("got new connection from {}", &addr);
                    self.new_peer(addr, socket, false).await?;
                    event_loop::Event::Connect(addr)
                }
            };

            for action in self.event_loop.handle(event).0 {
                match action {
                    event_loop::Action::Send(addr, message) => {
                        let peer = self.peers.get(&addr).expect("invalid peer");
                        peer.send(Command::Send(message)).await?;
                    }
                    event_loop::Action::Broadcast(message) => {
                        for peer in self.peers.values() {
                            peer.send(Command::Send(message.clone())).await?;
                        }
                    }
                    event_loop::Action::Upload(addr, block) => {
                        let peer = self.peers.get(&addr).expect("invalid peer");
                        peer.send(Command::Upload(block)).await?;
                    }
                    event_loop::Action::IntegratePiece { offset, data } => {
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

    async fn new_peer(
        &mut self,
        addr: SocketAddr,
        socket: TcpStream,
        send_handshake_first: bool,
    ) -> Result<()> {
        let (rx, tx) = mpsc::channel(16);
        let mut connection = Connection::new(
            addr,
            socket,
            self.file_path.clone(),
            self.torrent_info.piece_length,
            self.tx.clone(),
            tx,
        );
        if send_handshake_first {
            connection.send(&self.handshake).await?;
            connection.wait_for_handshake().await?;
        } else {
            connection.wait_for_handshake().await?;
            connection.send(&self.handshake).await?;
        }
        self.peers.insert(addr, rx);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            connection.start().await?;
            info!("peer {} disconnected", addr);
            tx.send(PeerEvent(addr, Event::Disconnected)).await?;
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
            leecher_tx
                .send(PeerEvent(seeder_addr, Event::Connect))
                .await
                .unwrap();

            leecher.start().await
        });

        let result = timeout(Duration::from_secs(20), set.join_all()).await;

        // assert leecher has file
        assert!(result.is_ok());
    }
}
