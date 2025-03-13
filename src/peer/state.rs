use size::Size;
use std::path::PathBuf;
use std::time::Duration;
use std::{collections::HashMap, net::SocketAddr};

use anyhow::anyhow;
use bit_set::BitSet;
use tokio::sync::mpsc::Sender;

use crate::peer::message::Message;
use crate::peer::transfer_rate::TransferRate;

#[derive(Debug)]
pub struct PeerState {
    pub tx: Sender<Message>,
    pub has_pieces: BitSet,
    pub client_to_peer: PeerToPeer,
    pub peer_to_client: PeerToPeer,
}

impl PeerState {
    pub fn new(tx: Sender<Message>) -> Self {
        Self {
            tx,
            has_pieces: BitSet::new(),
            client_to_peer: PeerToPeer::new(),
            peer_to_client: PeerToPeer::new(),
        }
    }
}

#[derive(Debug)]
pub struct PeerToPeer {
    pub transfer_rate: TransferRate,
    pub choking: bool,
    pub interested: bool,
}

impl PeerToPeer {
    pub fn new() -> Self {
        Self {
            transfer_rate: TransferRate::EMPTY,
            choking: true,
            interested: false,
        }
    }
}

// OLD -------------------------------------------------------------------------

#[derive(Debug)]
pub enum PeerCommand {
    Send(Message),
}

struct PeerStateOld {
    tx: Sender<PeerCommand>,
    has_pieces: BitSet,
    client_to_peer: PeerToPeer,
    peer_to_client: PeerToPeer,
}

impl PeerStateOld {
    fn new(tx: Sender<PeerCommand>) -> Self {
        Self {
            tx,
            has_pieces: BitSet::new(),
            client_to_peer: PeerToPeer::new(),
            peer_to_client: PeerToPeer::new(),
        }
    }
}

pub struct SharedStateOld {
    peers: HashMap<SocketAddr, PeerStateOld>,
    completed_pieces: BitSet,
    temp_dir: PathBuf,
}

impl SharedStateOld {
    pub fn new(temp_dir: PathBuf) -> Self {
        Self {
            peers: HashMap::new(),
            completed_pieces: BitSet::new(),
            temp_dir,
        }
    }

    pub fn peer_connected(&mut self, addr: SocketAddr, tx: Sender<PeerCommand>) {
        self.peers.insert(addr, PeerStateOld::new(tx));
    }

    pub fn peer_choked(&mut self, addr: &SocketAddr) {
        if let Some(peer) = self.peers.get_mut(addr) {
            peer.peer_to_client.choking = true;
        }
    }

    pub fn peer_unchoked(&mut self, addr: &SocketAddr) {
        if let Some(peer) = self.peers.get_mut(addr) {
            peer.peer_to_client.choking = false;
            // TODO: send request
        }
    }

    pub fn peer_interested(&mut self, addr: &SocketAddr) {
        if let Some(peer) = self.peers.get_mut(addr) {
            peer.peer_to_client.interested = true;
        }
    }

    pub fn peer_not_interested(&mut self, addr: &SocketAddr) {
        if let Some(peer) = self.peers.get_mut(addr) {
            peer.peer_to_client.interested = false;
        }
    }

    pub fn peer_has_piece(&mut self, addr: SocketAddr, piece: usize) -> bool {
        if let Some(peer) = self.peers.get_mut(&addr) {
            peer.has_pieces.insert(piece);
            // Peer has a piece we want. We're unable to request the piece while being choked, let
            // the peer know we're interested.
            return peer.peer_to_client.choking && !self.completed_pieces.contains(piece);
        }
        false
    }

    pub fn peer_has_pieces(&mut self, addr: &SocketAddr, pieces: BitSet) -> bool {
        if let Some(peer) = self.peers.get_mut(addr) {
            peer.has_pieces.union_with(&pieces);
            if peer.peer_to_client.choking {
                pieces.difference(&self.completed_pieces);
                return !pieces.is_empty();
            }
        }
        false
    }

    pub fn update_upload_rate(&mut self, addr: &SocketAddr, size: Size, duration: Duration) {
        if let Some(peer) = self.peers.get_mut(addr) {
            peer.client_to_peer.transfer_rate += TransferRate(size, duration);
        }
    }

    pub fn update_download_rate(&mut self, addr: &SocketAddr, size: Size, duration: Duration) {
        if let Some(peer) = self.peers.get_mut(addr) {
            peer.peer_to_client.transfer_rate += TransferRate(size, duration);
        }
    }

    pub fn file_path(&self, piece: usize) -> PathBuf {
        let mut path = self.temp_dir.clone();
        path.push(format!("piece-{}", piece));
        path
    }

    pub fn file_path_for_upload(&self, addr: &SocketAddr, piece: usize) -> anyhow::Result<PathBuf> {
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
