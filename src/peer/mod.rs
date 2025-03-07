pub mod message;

use std::collections::HashSet;
use std::io::{Result, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::{collections::HashMap, net::SocketAddr};

use anyhow::anyhow;
use bit_set::BitSet;
use log::{info, warn};
use rand::RngCore;
use size::{KiB, Size};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::Instant;

use crate::codec::{AsyncDecoder, AsyncEncoder};
use crate::crypto::Sha1;
use crate::peer::message::Handshake;
use crate::peer::message::Message;
use crate::torrent::Torrent;

const BLOCK_SIZE: Size = Size::from_const(16 * KiB);

#[derive(PartialEq, Clone)]
pub struct PeerId(pub [u8; 20]);

struct TransferRate(Size, Duration);

impl TransferRate {
    fn empty() -> Self {
        TransferRate(Size::from_bytes(0), Duration::ZERO)
    }

    fn update(&mut self, size: Size, duration: Duration) {
        self.0 += size;
        self.1 += duration;
    }
}

impl PeerId {
    pub fn random() -> Self {
        let mut data = [0; 20];
        rand::rng().fill_bytes(&mut data);
        Self(data)
    }
}

#[derive(Debug)]
enum PeerCommand {
    Send(Message),
}

struct PieceInfo {
    sha1: Sha1,
    have: HashSet<SocketAddr>,
}

impl PieceInfo {
    fn add_peer(&mut self, peer: SocketAddr) {
        self.have.insert(peer);
    }
}

struct PeerToPeer {
    transfer_rate: TransferRate,
    choking: bool,
    interested: bool,
}

impl PeerToPeer {
    fn new() -> Self {
        Self {
            transfer_rate: TransferRate::empty(),
            choking: true,
            interested: false,
        }
    }
}

struct PeerState {
    tx: Sender<PeerCommand>,
    has_pieces: BitSet,
    client_to_peer: PeerToPeer,
    peer_to_client: PeerToPeer,
}

impl PeerState {
    fn new(tx: Sender<PeerCommand>) -> Self {
        Self {
            tx,
            has_pieces: BitSet::new(),
            client_to_peer: PeerToPeer::new(),
            peer_to_client: PeerToPeer::new(),
        }
    }
}

pub struct SharedState {
    peers: HashMap<SocketAddr, PeerState>,
    pieces: Vec<PieceInfo>,
    completed_pieces: BitSet,
    temp_dir: PathBuf,
}

impl SharedState {
    pub fn new(torrent: &Torrent, temp_dir: PathBuf) -> Self {
        let pieces = torrent
            .info
            .pieces
            .iter()
            .map(|sha1| PieceInfo {
                sha1: sha1.clone(),
                have: HashSet::new(),
            })
            .collect();

        Self {
            peers: HashMap::new(),
            pieces,
            completed_pieces: BitSet::new(),
            temp_dir,
        }
    }

    async fn broadcast(&mut self, message: Message) {
        for (addr, peer) in &self.peers {
            let command = PeerCommand::Send(message.clone());
            if let Err(err) = peer.tx.send(command).await {
                warn!("unable to send command to {}: {}", addr, err);
            }
        }
    }

    fn peer_connected(&mut self, addr: SocketAddr, tx: Sender<PeerCommand>) {
        self.peers.insert(addr, PeerState::new(tx));
    }

    fn peer_choked(&mut self, addr: &SocketAddr) {
        if let Some(peer) = self.peers.get_mut(addr) {
            peer.peer_to_client.choking = true;
        }
    }

    fn peer_unchoked(&mut self, addr: &SocketAddr) {
        if let Some(peer) = self.peers.get_mut(addr) {
            peer.peer_to_client.choking = false;
        }
    }

    fn peer_interested(&mut self, addr: &SocketAddr) {
        if let Some(peer) = self.peers.get_mut(addr) {
            peer.peer_to_client.interested = true;
        }
    }

    fn peer_not_interested(&mut self, addr: &SocketAddr) {
        if let Some(peer) = self.peers.get_mut(addr) {
            peer.peer_to_client.interested = false;
        }
    }

    fn peer_has_piece(&mut self, addr: SocketAddr, piece: usize) {
        if let Some(peer) = self.peers.get_mut(&addr) {
            peer.has_pieces.insert(piece);
        }
        let piece_info = self.pieces.get_mut(piece).unwrap();
        piece_info.add_peer(addr);
    }

    fn peer_has_pieces(&mut self, addr: &SocketAddr, pieces: BitSet) {
        if let Some(peer) = self.peers.get_mut(&addr) {
            peer.has_pieces.union_with(&pieces);
        }
    }

    fn update_upload_rate(&mut self, addr: &SocketAddr, size: Size, duration: Duration) {
        if let Some(peer) = self.peers.get_mut(&addr) {
            peer.client_to_peer.transfer_rate.update(size, duration);
        }
    }

    fn update_download_rate(&mut self, addr: &SocketAddr, size: Size, duration: Duration) {
        if let Some(peer) = self.peers.get_mut(&addr) {
            peer.peer_to_client.transfer_rate.update(size, duration);
        }
    }

    fn file_path(&self, piece: usize) -> PathBuf {
        let mut path = self.temp_dir.clone();
        path.push(format!("piece-{}", piece));
        path
    }

    fn file_path_for_upload(&self, addr: &SocketAddr, piece: usize) -> anyhow::Result<PathBuf> {
        let peer = match self.peers.get(addr) {
            Some(peer) => peer,
            None => {
                return Err(anyhow!("peer {} not found", addr));
            }
        };
        if peer.client_to_peer.choking {
            return Err(anyhow!(
                "peer {} requested piece {} while being choked",
                addr,
                piece
            ));
        }
        if !self.completed_pieces.contains(piece) {
            return Err(anyhow!(
                "peer {} requested uncompleted piece {}",
                addr,
                piece
            ));
        }
        Ok(self.file_path(piece))
    }
}

pub struct Peer {
    addr: SocketAddr,
    socket: TcpStream,
    state: Arc<RwLock<SharedState>>,
    rx: Receiver<PeerCommand>,
}

impl Peer {
    pub async fn new(addr: SocketAddr, socket: TcpStream, state: Arc<RwLock<SharedState>>) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        let peer = Self {
            addr,
            socket,
            state: Arc::clone(&state),
            rx,
        };
        state.write().await.peer_connected(addr, tx);
        peer
    }

    pub async fn wait_for_handshake(&mut self) -> Result<()> {
        let handshake = Handshake::decode(&mut self.socket).await?;
        info!("< got handshake {:?}", handshake);
        Ok(())
    }

    pub async fn send_handshake(&mut self, handshake: &Handshake) -> Result<()> {
        handshake.encode(&mut self.socket).await?;
        info!("> sent handshake");
        Ok(())
    }

    pub async fn process(&mut self) -> Result<()> {
        tokio::select! {
            Some(command) = self.rx.recv() => {
                info!("< got command {:?}", command);
                match command {
                    PeerCommand::Send(message) => {
                        message.encode(&mut self.socket).await?;
                    }
                }
            }
            message = Message::decode(&mut self.socket) => {
                info!("< got messsage {:?}", message);
                match message {
                    Ok(Message::KeepAlive) => {
                        // No-op
                    }
                    Ok(Message::Choke) => {
                        let mut state = self.state.write().await;
                        state.peer_choked(&self.addr);
                    }
                    Ok(Message::Unchoke) => {
                        let mut state = self.state.write().await;
                        state.peer_unchoked(&self.addr);
                    }
                    Ok(Message::Interested) => {
                        let mut state = self.state.write().await;
                        state.peer_interested(&self.addr);
                    }
                    Ok(Message::NotInterested) => {
                        let mut state = self.state.write().await;
                        state.peer_not_interested(&self.addr);
                    }
                    Ok(Message::Have { piece }) => {
                        let mut state = self.state.write().await;
                        state.peer_has_piece(self.addr, piece);
                    }
                    Ok(Message::Bitfield(bitset)) => {
                        let mut state = self.state.write().await;
                        state.peer_has_pieces(&self.addr, bitset);
                    }
                    Ok(Message::Request(block)) => {
                        // Upload to peer
                        let file_path = {
                            let state = self.state.read().await;
                            match state.file_path_for_upload(&self.addr, block.piece) {
                                Ok(path) => path,
                                Err(err) => {
                                    warn!("{}", err);
                                    return Ok(());
                                }
                            }
                        };
                        let mut file = File::open(file_path).await?;
                        file.seek(SeekFrom::Start(block.offset as u64)).await?;
                        let mut reader = file.take(block.length as u64);
                        let message = Message::Piece(block.clone());
                        let transfer_begin = Instant::now();
                        message.encode(&mut self.socket).await?;
                        let bytes = tokio::io::copy(&mut reader, &mut self.socket).await? as usize;
                        let duration = Instant::now() - transfer_begin;
                        let size = Size::from_bytes(bytes);
                        if bytes != block.length {
                            warn!("peer {} requested block {:?}, actually transferred {} bytes",
                                self.addr, block, bytes);
                        }

                        let mut state = self.state.write().await;
                        state.update_upload_rate(&self.addr, size, duration);
                    }
                    Ok(Message::Piece(block)) => {
                        // Download from peer
                        let file_path = {
                            let state = self.state.read().await;
                            state.file_path(block.piece)
                        };
                        // TODO: check block is in flight
                        let mut buf = vec![0; block.length];
                        let transfer_begin = Instant::now();
                        self.socket.read_exact(&mut buf).await?;
                        let duration = Instant::now() - transfer_begin;
                        let size = Size::from_bytes(block.length);
                        let mut file = OpenOptions::new().write(true).open(file_path).await?;
                        file.seek(SeekFrom::Start(block.offset as u64)).await?;
                        file.write_all(&buf).await?;

                        // TODO: update block was downloaded
                        let mut state = self.state.write().await;
                        state.update_download_rate(&self.addr, size, duration);
                    }
                    Ok(message) => warn!("unhandled message {:?}", message),
                    Err(err) => warn!("error decoding message: {}", err)
                }
            }
        };
        Ok(())
    }
}

/*
struct PeerSet {
    peer_to_piece: HashMap<SocketAddr, PieceInfo>,
    piece_to_peer: HashMap<usize, HashSet<SocketAddr>>,
}

impl PeerSet {
    fn new() -> Self {
        PeerSet {
            peer_to_piece: HashMap::new(),
            piece_to_peer: HashMap::new(),
        }
    }

    fn have(&mut self, peer: SocketAddr, piece: usize) {
        //
    }
}

struct PieceInfo {
    downloading: Option<SocketAddr>,
    have: HashSet<SocketAddr>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn have() {
        let mut peer_set = PeerSet::new();
        let peer = "0.0.0.1:6881".parse().unwrap();
        peer_set.have(peer, 1234);
    }
}
*/
