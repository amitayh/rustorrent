mod blocks;
mod choke;
pub mod config;
mod connection;
mod handler;
pub mod peer_id;
mod piece;
mod sizes;
mod transfer_rate;

use std::collections::{HashMap, HashSet};
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

use crate::message::Block;
use crate::message::Handshake;
use crate::message::Message;
use crate::peer::choke::Choker;
use crate::peer::config::Config;
use crate::peer::connection::Command;
use crate::peer::connection::Connection;
use crate::peer::peer_id::PeerId;
use crate::peer::piece::Distributor;
use crate::peer::piece::{Joiner, Status};
use crate::peer::sizes::Sizes;
use crate::peer::transfer_rate::TransferRate;
use crate::torrent::Info;

pub struct PeerEvent(pub SocketAddr, pub Event);

pub enum Event {
    Connect,
    Disconnected,
    Message(Message, TransferRate),
    Uploaded(Block, TransferRate),
}

pub struct Peer {
    listener: TcpListener,
    torrent_info: Info,
    file_path: PathBuf,
    peers: HashMap<SocketAddr, Sender<Command>>,
    choker: Choker,
    distributor: Distributor,
    joiner: Joiner,
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
        let sizes = Sizes::new(
            torrent_info.piece_length,
            torrent_info.download_type.length(),
            config.block_size,
        );
        let distributor = Distributor::new(&sizes);
        let joiner = Joiner::new(&sizes, torrent_info.pieces.clone());

        let (tx, rx) = mpsc::channel(128);
        let peer = Self {
            listener,
            torrent_info,
            file_path,
            peers: HashMap::new(),
            choker,
            distributor,
            joiner,
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
        self.distributor.client_has_pieces(&self.has_pieces);
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
                Some(PeerEvent(addr, event)) = self.rx.recv() => {
                    match event {
                        Event::Message(message, transfer_rate) => {
                            self.choker.update_peer_transfer_rate(addr, transfer_rate);
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
                            self.peer_disconnected(&addr);
                        }
                        Event::Uploaded(_block, _transfer_rate) => {
                            //info!("sent block data! {:?}", transfer_rate);
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

    fn peer_disconnected(&mut self, addr: &SocketAddr) {
        self.peers.remove(addr);
        self.choker.peer_disconnected(addr);
        self.distributor.peer_disconnected(addr);
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
                Message::Choke => self.distributor.peer_choked(addr),
                Message::Unchoke => {
                    if let Some(block) = self.distributor.peer_unchoked(addr) {
                        peer_state
                            .send(Command::Send(Message::Request(block)))
                            .await?;
                    }
                }
                Message::Interested => self.choker.peer_interested(addr),
                Message::NotInterested => self.choker.peer_not_interested(&addr),
                Message::Have { piece } => {
                    let mut pieces = BitSet::new();
                    pieces.insert(piece);
                    self.peer_has_pieces(addr, pieces).await?;
                }
                Message::Bitfield(pieces) => self.peer_has_pieces(addr, pieces).await?,
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
                Message::Piece {
                    piece,
                    offset,
                    data,
                } => {
                    // TODO: verify block was in flight
                    let block = Block::new(piece, offset, data.len());
                    if let Some(block) = self.distributor.block_downloaded(&addr, &block) {
                        peer_state
                            .send(Command::Send(Message::Request(block)))
                            .await?;
                    }
                    match self.joiner.add(piece, offset, data) {
                        Status::Incomplete => (),
                        Status::Invalid => self.distributor.invalidate(piece),
                        Status::Valid { offset, data } => {
                            let mut file = OpenOptions::new()
                                .create(true)
                                .write(true)
                                .truncate(true)
                                .open(&self.file_path)
                                .await?;
                            file.seek(SeekFrom::Start(offset)).await?;
                            file.write_all(&data).await?;
                            self.has_pieces.insert(piece);
                            self.distributor.client_has_piece(piece);
                            self.broadcast(Message::Have { piece }).await?;
                        }
                    }
                }
                Message::Cancel(_) | Message::Port(_) => todo!(),
            },
            None => panic!("invalid peer"),
        }
        Ok(())
    }

    async fn peer_has_pieces(&mut self, addr: SocketAddr, pieces: BitSet) -> anyhow::Result<()> {
        let peer = self.peers.get_mut(&addr).expect("invalid peer");
        let want = pieces.difference(&self.has_pieces);
        if !self.interested.contains(&addr) && want.count() > 0 {
            peer.send(Command::Send(Message::Interested)).await?;
            self.interested.insert(addr);
        }
        if let Some(block) = self.distributor.peer_has_pieces(addr, &pieces) {
            peer.send(Command::Send(Message::Request(block))).await?;
        }
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
            Peer::new(listener, torrent_info, "/tmp/foo".into(), Config::default())
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
