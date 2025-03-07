pub mod message;

use std::collections::HashSet;
use std::io::{Result, SeekFrom};
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::{collections::HashMap, net::SocketAddr};

use log::{info, warn};
use rand::RngCore;
use rangemap::RangeSet;
use size::{KiB, Size};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::sync::mpsc::{Receiver, Sender};

use crate::codec::{AsyncDecoder, AsyncEncoder};
use crate::crypto::Sha1;
use crate::peer::message::Message;
use crate::peer::message::{Block, Handshake};
use crate::torrent::Torrent;

const BLOCK_SIZE: Size = Size::from_const(16 * KiB);

#[derive(PartialEq, Clone)]
pub struct PeerId(pub [u8; 20]);

pub struct TransferRate(Size, Duration);

impl PeerId {
    pub fn random() -> Self {
        let mut data = [0; 20];
        rand::rng().fill_bytes(&mut data);
        Self(data)
    }
}

type PeerCommand = u32;

struct PieceInfo {
    sha1: Sha1,
    have: HashSet<SocketAddr>,
    completed: RangeSet<usize>,
}

impl PieceInfo {
    fn add_peer(&mut self, peer: SocketAddr) {
        self.have.insert(peer);
    }

    fn have_bytes(&self, wanted: &RangeInclusive<usize>) -> bool {
        self.completed
            .iter()
            .any(|have| have.contains(wanted.start()) && have.contains(wanted.end()))
    }
}

pub struct SharedState {
    peers: HashMap<SocketAddr, Sender<PeerCommand>>,
    interested: HashSet<SocketAddr>,
    unchoked: HashSet<SocketAddr>,
    pieces: Vec<PieceInfo>,
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
                completed: RangeSet::new(),
            })
            .collect();

        Self {
            peers: HashMap::new(),
            interested: HashSet::new(),
            unchoked: HashSet::new(),
            pieces,
            temp_dir,
        }
    }

    fn peer_connected(&mut self, addr: SocketAddr, tx: Sender<PeerCommand>) {
        self.peers.insert(addr, tx);
    }

    fn peer_choked(&mut self, addr: &SocketAddr) {
        self.unchoked.remove(addr);
    }

    fn peer_unchoked(&mut self, addr: SocketAddr) {
        self.unchoked.insert(addr);
    }

    fn peer_interested(&mut self, addr: SocketAddr) {
        self.interested.insert(addr);
    }

    fn peer_not_interested(&mut self, addr: &SocketAddr) {
        self.interested.remove(addr);
    }

    fn peer_has_piece(&mut self, addr: SocketAddr, piece: usize) {
        let piece_info = self.pieces.get_mut(piece).unwrap();
        piece_info.add_peer(addr);
    }

    fn get_file_path(&self, block: &Block) -> PathBuf {
        let mut path = self.temp_dir.clone();
        path.push(format!("piece-{}", block.piece));
        path
    }

    fn request_allowed(&self, addr: &SocketAddr, block: &Block) -> bool {
        let piece_info = self.pieces.get(block.piece).unwrap();
        self.unchoked.contains(addr) && piece_info.have_bytes(&block.bytes_range())
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
                        state.peer_unchoked(self.addr);
                    }
                    Ok(Message::Interested) => {
                        let mut state = self.state.write().await;
                        state.peer_interested(self.addr);
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
                        for piece in bitset.iter() {
                            state.peer_has_piece(self.addr, piece);
                        }
                    }
                    Ok(Message::Request(block)) => {
                        let (file_path, allowed) = {
                            let state = self.state.read().await;
                            let file = state.get_file_path(&block);
                            let allowed = state.request_allowed(&self.addr, &block);
                            (file, allowed)
                        };
                        if !allowed {
                            warn!("peer {} requested {:?} while not allowed", &self.addr, &block);
                            return Ok(());
                        }
                        let mut file = File::open(file_path).await?;
                        file.seek(SeekFrom::Start(block.offset as u64)).await?;
                        let mut reader = file.take(block.length as u64);
                        tokio::io::copy(&mut reader, &mut self.socket).await?;
                    }
                    Ok(Message::Piece(block)) => {
                        let file_path = {
                            let state = self.state.read().await;
                            state.get_file_path(&block)
                        };
                        let mut file = OpenOptions::new().write(true).open(file_path).await?;
                        file.seek(SeekFrom::Start(block.offset as u64)).await?;
                        // let mut reader = self.socket.take(block.length as u64);
                        tokio::io::copy(&mut self.socket, &mut file).await?;
                    }
                    _ => warn!("unhandled")
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
