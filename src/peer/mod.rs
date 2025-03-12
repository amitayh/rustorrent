#![allow(dead_code)]
pub mod blocks;
pub mod config;
pub mod connection;
pub mod message;
pub mod peer;
pub mod peer_id;
pub mod piece_selector;
pub mod transfer_rate;

use std::path::PathBuf;
use std::time::Duration;
use std::{collections::HashMap, net::SocketAddr};

use anyhow::anyhow;
use bit_set::BitSet;
use size::Size;
use tokio::sync::mpsc::Sender;

use crate::peer::message::Block;
use crate::peer::message::Message;
use crate::peer::transfer_rate::TransferRate;

//const BLOCK_SIZE: Size = Size::from_const(16 * KiB);
const BLOCK_SIZE: usize = 16 * 1024;

#[derive(Debug)]
enum PeerCommand {
    Send(Message),
}

#[derive(Debug)]
struct PeerToPeer {
    transfer_rate: TransferRate,
    choking: bool,
    interested: bool,
}

impl PeerToPeer {
    fn new() -> Self {
        Self {
            transfer_rate: TransferRate::EMPTY,
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
    completed_pieces: BitSet,
    temp_dir: PathBuf,
}

impl SharedState {
    pub fn new(temp_dir: PathBuf) -> Self {
        Self {
            peers: HashMap::new(),
            completed_pieces: BitSet::new(),
            temp_dir,
        }
    }

    pub async fn select_pieces_to_request(&self) {
        let mut result: HashMap<usize, Vec<SocketAddr>> = HashMap::new();
        for (addr, peer) in &self.peers {
            if peer.peer_to_client.choking {
                continue;
            }
            for piece in peer.has_pieces.iter() {
                if !self.completed_pieces.contains(piece) {
                    let entry = result.entry(piece).or_default();
                    entry.push(*addr);
                }
            }
        }
        let mut entries: Vec<_> = result.into_iter().collect();
        // Select rarest piece
        entries.sort_by(|(_, peers_a), (_, peers_b)| peers_b.len().cmp(&peers_a.len()));
        for (piece, peers) in entries {
            let message = Message::Request(Block::new(piece, 0, BLOCK_SIZE));
            self.peers
                .get(peers.first().unwrap())
                .unwrap()
                .tx
                .send(PeerCommand::Send(message))
                .await
                .unwrap();
        }
    }

    //async fn broadcast(&self, message: Message) {
    //    for (addr, peer) in &self.peers {
    //        let command = PeerCommand::Send(message.clone());
    //        if let Err(err) = peer.tx.send(command).await {
    //            warn!("unable to send command to {}: {}", addr, err);
    //        }
    //    }
    //}

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
            // TODO: send request
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

    fn peer_has_piece(&mut self, addr: SocketAddr, piece: usize) -> bool {
        if let Some(peer) = self.peers.get_mut(&addr) {
            peer.has_pieces.insert(piece);
            // Peer has a piece we want. We're unable to request the piece while being choked, let
            // the peer know we're interested.
            return peer.peer_to_client.choking && !self.completed_pieces.contains(piece);
        }
        false
    }

    fn peer_has_pieces(&mut self, addr: &SocketAddr, pieces: BitSet) -> bool {
        if let Some(peer) = self.peers.get_mut(addr) {
            peer.has_pieces.union_with(&pieces);
            if peer.peer_to_client.choking {
                pieces.difference(&self.completed_pieces);
                return !pieces.is_empty();
            }
        }
        false
    }

    fn update_upload_rate(&mut self, addr: &SocketAddr, size: Size, duration: Duration) {
        if let Some(peer) = self.peers.get_mut(addr) {
            peer.client_to_peer.transfer_rate += TransferRate(size, duration);
        }
    }

    fn update_download_rate(&mut self, addr: &SocketAddr, size: Size, duration: Duration) {
        if let Some(peer) = self.peers.get_mut(addr) {
            peer.peer_to_client.transfer_rate += TransferRate(size, duration);
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
