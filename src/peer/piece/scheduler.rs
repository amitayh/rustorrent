#![allow(dead_code, unused)]
use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    sync::Arc,
};

use bit_set::BitSet;
use futures::SinkExt;
use priority_queue::PriorityQueue;

use crate::{
    message::Block,
    peer::{Download, blocks::Blocks},
};

pub struct Scheduler {
    download: Arc<Download>,
    /// Contains only pieces that the client still doesn't have
    pieces: HashMap<usize, Piece>,
    peers: HashMap<SocketAddr, Peer>,
}

impl Scheduler {
    pub fn new(download: Arc<Download>, has_pieces: BitSet) -> Self {
        let total_pieces = download.torrent.info.total_pieces();
        let mut pieces = HashMap::with_capacity(total_pieces);
        let missing_pieces = (0..total_pieces).filter(|piece| !has_pieces.contains(*piece));
        for piece in missing_pieces {
            let blocks = download.blocks(piece);
            pieces.insert(piece, Piece::new(blocks));
        }
        Self {
            download,
            pieces,
            peers: HashMap::new(),
        }
    }

    pub fn peer_choked(&mut self, addr: SocketAddr) {
        let peer = self.peers.entry(addr).or_default();
        peer.choking = true;
        for block in peer.assigned_blocks.drain() {
            let piece = self.pieces.get_mut(&block.piece).expect("invalid piece");
            piece.unassign(block);
        }
    }

    pub fn peer_unchoked(&mut self, addr: SocketAddr) -> Vec<Block> {
        let peer = self.peers.entry(addr).or_default();
        if !peer.choking {
            // Client was already unchoked, do nothing
            return Vec::new();
        }
        peer.choking = false;
        self.assign(&addr)
    }

    pub fn peer_has_piece(&mut self, addr: SocketAddr, piece: usize) -> HaveResult {
        if !self.pieces.contains_key(&piece) {
            // Ignore if client already has piece
            return HaveResult::NotInterested;
        }

        let priority = {
            let state = self.pieces.get_mut(&piece).expect("invalid piece");
            state.peers_with_piece.insert(addr);
            state.priority()
        };

        let blocks_to_request = {
            // Add piece to peer's queue
            let peer = self.peers.entry(addr).or_default();
            peer.peer_pieces.push(piece, priority);
            self.assign(&addr)
        };

        // Update piece priority for all peers
        let state = self.pieces.get(&piece).expect("invalid piece");
        for addr in &state.peers_with_piece {
            let peer = self.peers.get_mut(addr).expect("invalid peer");
            peer.peer_pieces.change_priority(&piece, priority);
        }

        HaveResult::Interested { blocks_to_request }
    }

    /// Returns peers that are no longer interesting (don't have any piece we don't already have)
    pub fn client_has_piece(&mut self, piece: usize) -> HashSet<SocketAddr> {
        if let Some(state) = self.pieces.remove(&piece) {
            //
        }
        HashSet::new()
    }

    pub fn peer_disconnected(&mut self, addr: &SocketAddr) {
        if let Some(mut peer) = self.peers.remove(addr) {
            for block in peer.assigned_blocks.drain() {
                let piece = self.pieces.get_mut(&block.piece).expect("invalid piece");
                piece.peer_disconnected(addr);
                piece.unassign(block);
            }
        }
    }

    fn assign(&mut self, addr: &SocketAddr) -> Vec<Block> {
        let peer = self.peers.get_mut(addr).expect("invalid peer");
        if peer.choking {
            return Vec::new();
        }

        let max_blocks = self.download.config.max_concurrent_requests_per_peer;
        let mut blocks_to_request = max_blocks - peer.assigned_blocks.len();
        let mut blocks = Vec::with_capacity(blocks_to_request);

        // Update priority for pieces
        let mut pieces_assigned_from = BitSet::new();

        while blocks_to_request > 0 && !peer.peer_pieces.is_empty() {
            let (piece, _) = peer.peer_pieces.peek().expect("queue is not empty");
            let state = self.pieces.get_mut(piece).expect("invalid piece");
            pieces_assigned_from.insert(*piece);
            while blocks_to_request > 0 && !state.unassigned_blocks.is_empty() {
                let block = state.unassigned_blocks.pop_front();
                blocks.push(block.expect("unassigned blocks not empty"));
                blocks_to_request -= 1;
            }
            if state.unassigned_blocks.is_empty() {
                peer.peer_pieces.pop();
            }
        }

        for piece in &pieces_assigned_from {
            let state = self.pieces.get(&piece).expect("invalid piece");
            let priority = state.priority();
            for addr in &state.peers_with_piece {
                let peer = self.peers.get_mut(addr).expect("invalid peer");
                peer.peer_pieces.change_priority(&piece, priority);
            }
        }

        blocks
    }
}

// TODO: rename
#[derive(Debug, PartialEq, Eq)]
pub enum HaveResult {
    NotInterested,
    Interested { blocks_to_request: Vec<Block> },
}

// -------------------------------------------------------------------------------------------------

struct Piece {
    total_blocks: usize,
    unassigned_blocks: VecDeque<Block>,
    peers_with_piece: HashSet<SocketAddr>,
}

impl Piece {
    fn new(blocks: Blocks) -> Self {
        let unassigned_blocks: VecDeque<_> = blocks.collect();
        Self {
            total_blocks: unassigned_blocks.len(),
            unassigned_blocks,
            peers_with_piece: HashSet::new(),
        }
    }

    pub fn peer_disconnected(&mut self, addr: &SocketAddr) {
        self.peers_with_piece.remove(addr);
    }

    fn unassign(&mut self, block: Block) {
        self.unassigned_blocks.push_front(block);
    }

    fn priority(&self) -> DownloadPriority {
        DownloadPriority::new(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct DownloadPriority {
    has_assigned_blocks: bool,
    num_peers_with_piece: Reverse<usize>,
}

impl DownloadPriority {
    fn new(piece: &Piece) -> Self {
        Self {
            has_assigned_blocks: piece.unassigned_blocks.len() < piece.total_blocks,
            num_peers_with_piece: Reverse(piece.peers_with_piece.len()),
        }
    }
}

// -------------------------------------------------------------------------------------------------

#[derive(Debug)]
struct Peer {
    /// If the peer is choking the clinet
    choking: bool,
    /// Pieces the peer has and the client doesn't, by download priority
    peer_pieces: PriorityQueue<usize, DownloadPriority>,
    /// Blocks assigned to be downloaded from the peer
    assigned_blocks: HashSet<Block>,
}

impl Default for Peer {
    fn default() -> Self {
        Self {
            choking: true,
            peer_pieces: PriorityQueue::default(),
            assigned_blocks: HashSet::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use size::Size;

    use crate::peer::{
        piece::scheduler,
        tests::{test_config, test_torrent},
    };

    use super::*;

    #[test]
    fn peer_unchoked_but_has_no_pieces() {
        let mut scheduler = test_scheduler();
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert!(scheduler.peer_unchoked(addr).is_empty());
    }

    #[test]
    fn peer_unchoked_but_client_already_has_that_piece() {
        let mut scheduler = test_scheduler();
        scheduler.client_has_piece(0);
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert_eq!(scheduler.peer_has_piece(addr, 0), HaveResult::NotInterested);
        assert!(scheduler.peer_unchoked(addr).is_empty());
    }

    #[test]
    fn assign_a_block_to_request_from_peer() {
        let mut scheduler = test_scheduler();
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert_eq!(
            scheduler.peer_has_piece(addr, 0),
            HaveResult::Interested {
                blocks_to_request: vec![]
            }
        );
        assert_eq!(scheduler.peer_unchoked(addr), vec![Block::new(0, 0, 8)]);
    }

    fn test_scheduler() -> Scheduler {
        let torrent = test_torrent();
        let config = test_config("/tmp")
            .with_block_size(Size::from_bytes(8))
            .with_max_concurrent_requests_per_peer(1);
        let download = Arc::new(Download { torrent, config });
        Scheduler::new(download, BitSet::new())
    }
}
