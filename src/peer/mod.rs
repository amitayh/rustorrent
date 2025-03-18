pub mod block_assigner;
pub mod blocks;
pub mod choke;
pub mod config;
pub mod connection;
pub mod handler;
pub mod message;
pub mod peer_id;
pub mod piece_assembler;
pub mod transfer_rate;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::{io::Result, net::SocketAddr};

use bit_set::BitSet;
use block_assigner::BlockAssignerConfig;
use log::{info, warn};
use message::Block;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::time::{self, Instant, Interval};
use transfer_rate::TransferRate;

use crate::peer::block_assigner::BlockAssigner;
use crate::peer::choke::Choker;
use crate::peer::config::Config;
use crate::peer::connection::Command;
use crate::peer::connection::Connection;
use crate::peer::message::Handshake;
use crate::peer::message::Message;
use crate::peer::peer_id::PeerId;
use crate::torrent::Info;

pub struct PeerEvent(pub SocketAddr, pub Event);

pub enum Event {
    Connect,
    Disconnected,
    Message(Message),
    Uploaded(Block, TransferRate),
    Downloaded(Block, Vec<u8>, TransferRate),
}

pub struct Peer {
    listener: TcpListener,
    torrent_info: Info,
    file_path: PathBuf,
    peers: HashMap<SocketAddr, Sender<Command>>,
    choker: Choker,
    block_assigner: BlockAssigner,
    tx: Sender<PeerEvent>,
    rx: Receiver<PeerEvent>,
    handshake: Handshake,
    has_pieces: BitSet,
    interested: HashSet<SocketAddr>,
    config: Config,
}

impl Peer {
    pub fn new(
        listener: TcpListener,
        torrent_info: Info,
        file_path: PathBuf,
        config: Config,
    ) -> (Self, Sender<PeerEvent>) {
        let peer_id = PeerId::random();
        let handshake = Handshake::new(torrent_info.info_hash.clone(), peer_id.clone());
        let total_pieces = torrent_info.pieces.len();
        let choker = Choker::new(config.optimistic_choking_cycle);
        let block_assigner = BlockAssigner::new(
            torrent_info.piece_length,
            torrent_info.download_type.length(),
            config.block_size,
            config.block_assigner_config.clone(),
        );
        let (tx, rx) = mpsc::channel(128);
        let peer = Self {
            listener,
            torrent_info,
            file_path,
            peers: HashMap::new(),
            choker,
            block_assigner,
            tx: tx.clone(),
            rx,
            handshake,
            has_pieces: BitSet::with_capacity(total_pieces),
            interested: HashSet::new(),
            config,
        };
        (peer, tx)
    }

    #[allow(dead_code)]
    fn temp_has_file(&mut self) {
        for piece in 0..self.torrent_info.pieces.len() {
            self.has_pieces.insert(piece);
        }
    }

    #[allow(dead_code)]
    fn address(&self) -> Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        let mut keep_alive = interval_with_delay(self.config.keep_alive_interval);
        let mut choke = interval_with_delay(self.config.choking_interval);
        loop {
            tokio::select! {
                // TODO: disconnect idle peers
                _ = keep_alive.tick() => {
                    self.broadcast(Message::KeepAlive).await?;
                }
                _ = choke.tick() => {
                    self.choke().await?;
                }
                (addr, block) = self.block_assigner.next() => {
                    let peer = self.peers.get_mut(&addr).expect("invalid peer");
                    peer.send(Command::Send(Message::Request(block)))
                        .await
                        .expect("unable to send");
                }
                Some(PeerEvent(addr, event)) = self.rx.recv() => {
                    match event {
                        Event::Message(message) => {
                            self.handle_message(addr, message).await?;
                        }
                        Event::Connect => {
                            info!("connecting to {}", addr);
                            match TcpStream::connect(addr).await {
                                Ok(socket) => self.new_peer(addr, socket, true).await?,
                                Err(err) => {
                                    warn!("error connecting to {}: {}", addr, err);
                                }
                            }
                        }
                        Event::Disconnected => {
                            self.peer_disconnected(&addr).await;
                        }
                        Event::Uploaded(_block, transfer_rate) => {
                            info!("sent block data! {:?}", transfer_rate);
                        }
                        Event::Downloaded(block, _data, transfer_rate) => {
                            // TODO: verify block was in flight
                            self.block_assigner.block_downloaded(&addr, &block).await;
                            info!("got block data! {:?}", transfer_rate);
                            self.choker.update_peer_transfer_rate(addr, transfer_rate);
                            // TODO: add piece to piece assembler
                            // if piece is complete and valid = broadcast Have
                            // if invalid - invalidate piece
                        }
                    }
                }
                Ok((socket, addr)) = self.listener.accept() => {
                    info!("got new connection from {}", &addr);
                    self.new_peer(addr, socket, false).await?;
                }
            }
        }
    }

    async fn broadcast(&self, message: Message) -> anyhow::Result<()> {
        for peer in self.peers.values() {
            peer.send(Command::Send(message.clone())).await?;
        }
        Ok(())
    }

    async fn new_peer(
        &mut self,
        addr: SocketAddr,
        socket: TcpStream,
        send_handshake_first: bool,
    ) -> Result<()> {
        let (rx, tx) = mpsc::channel(16);
        let mut connection =
            Connection::new(addr, socket, self.file_path.clone(), self.tx.clone(), tx);
        if send_handshake_first {
            connection.send(&self.handshake).await?;
            connection.wait_for_handshake().await?;
        } else {
            connection.wait_for_handshake().await?;
            connection.send(&self.handshake).await?;
        }
        if !self.has_pieces.is_empty() {
            let bitfield = Message::Bitfield(self.has_pieces.clone());
            connection.send(&bitfield).await?;
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

    async fn peer_disconnected(&mut self, addr: &SocketAddr) {
        self.peers.remove(addr);
        self.choker.peer_disconnected(addr);
        self.block_assigner.peer_choked(addr).await;
    }

    async fn choke(&mut self) -> anyhow::Result<()> {
        let decision = self.choker.run();
        for addr in &decision.peers_to_choke {
            info!("choking {}", addr);
            let peer = self.peers.get_mut(addr).expect("invalid peer");
            peer.send(Command::Send(Message::Choke)).await?;
        }
        for addr in &decision.peers_to_unchoke {
            info!("unchoking {}", addr);
            let peer = self.peers.get_mut(addr).expect("invalid peer");
            peer.send(Command::Send(Message::Unchoke)).await?;
        }
        Ok(())
    }

    async fn handle_message(&mut self, addr: SocketAddr, message: Message) -> anyhow::Result<()> {
        match self.peers.get_mut(&addr) {
            Some(peer_state) => match message {
                Message::KeepAlive => (), // No-op
                Message::Choke => {
                    self.block_assigner.peer_choked(&addr).await;
                }
                Message::Unchoke => {
                    self.block_assigner.peer_unchoked(addr);
                }
                Message::Interested => {
                    self.choker.peer_interested(addr);
                }
                Message::NotInterested => {
                    self.choker.peer_not_interested(&addr);
                }
                Message::Have { piece } => {
                    let mut pieces = BitSet::new();
                    pieces.insert(piece);
                    self.peer_has_pieces(addr, pieces).await?;
                }
                Message::Bitfield(pieces) => {
                    self.peer_has_pieces(addr, pieces).await?;
                }
                Message::Request(block) => {
                    if !self.choker.is_unchoked(&addr) {
                        warn!(
                            "peer {} requested block {:?} while being choked",
                            addr, block
                        );
                        return Ok(());
                    }
                    if !self.has_pieces.contains(block.piece) {
                        warn!(
                            "peer {} requested piece {} which is not available",
                            addr, block.piece
                        );
                        return Ok(());
                    }
                    peer_state.send(Command::Upload(block)).await?;
                }
                _ => todo!(),
            },
            None => panic!("invalid peer"),
        }
        Ok(())
    }

    async fn peer_has_pieces(&mut self, addr: SocketAddr, pieces: BitSet) -> anyhow::Result<()> {
        let want = pieces.difference(&self.has_pieces);
        if !self.interested.contains(&addr) && want.count() > 0 {
            let peer = self.peers.get_mut(&addr).expect("invalid peer");
            peer.send(Command::Send(Message::Interested)).await?;
            self.interested.insert(addr);
        }
        self.block_assigner.peer_has_pieces(addr, &pieces).await;
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
            let (mut peer, _) = Peer::new(listener, torrent_info.clone(), file_path.into(), config);
            peer.temp_has_file();
            peer
        };
        let seeder_addr = seeder.address().unwrap();
        let (mut leecher, leecher_tx) = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            Peer::new(listener, torrent_info, file_path.into(), Config::default())
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
