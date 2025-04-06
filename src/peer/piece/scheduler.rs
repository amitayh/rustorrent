#![allow(dead_code)]
use std::{
    cmp::{Ordering, Reverse},
    collections::{BTreeSet, HashMap, HashSet, hash_map::Entry},
    net::SocketAddr,
    sync::Arc,
};

use bit_set::BitSet;

use crate::{
    message::Block,
    peer::{Download, blocks::Blocks},
};

pub struct Scheduler {
    download: Arc<Download>,
    /// Contains only pieces that the client still doesn't have
    pieces: HashMap<usize, Piece>,
    peers: HashMap<SocketAddr, Peer>,

    pieces_by_priority: BTreeSet<Reverse<PiecePriority>>,
}

impl Scheduler {
    pub fn new(download: Arc<Download>, has_pieces: BitSet) -> Self {
        let total_pieces = download.torrent.info.total_pieces();
        let mut pieces = HashMap::with_capacity(total_pieces);

        let mut pieces_by_priority = BTreeSet::new();

        let missing_pieces = (0..total_pieces).filter(|piece| !has_pieces.contains(*piece));
        for piece in missing_pieces {
            let blocks = download.blocks(piece);
            let state = Piece::new(piece, blocks);
            pieces_by_priority.insert(Reverse(state.priority));
            pieces.insert(piece, state);
        }
        Self {
            download,
            pieces,
            peers: HashMap::new(),
            pieces_by_priority,
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
            return HaveResult::None;
        }

        let state = self.pieces.get_mut(&piece).expect("invalid piece");
        assert!(
            self.pieces_by_priority.remove(&Reverse(state.priority)),
            "piece priority should be present"
        );
        state.priority.num_peers_with_piece += 1;
        self.pieces_by_priority.insert(Reverse(state.priority));

        let (peer, interested) = match self.peers.entry(addr) {
            Entry::Occupied(entry) => (entry.into_mut(), false),
            Entry::Vacant(entry) => (entry.insert(Peer::default()), true),
        };
        peer.peer_pieces.insert(piece);
        if peer.choking {
            return if interested {
                HaveResult::Interested
            } else {
                HaveResult::None
            };
        }

        HaveResult::InterestedAndRequest(self.assign(&addr))
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
        let mut assigned_pieces = BitSet::new();

        for Reverse(PiecePriority { piece, .. }) in &self.pieces_by_priority {
            if peer.peer_pieces.contains(*piece) {
                let state = self.pieces.get_mut(piece).expect("invalid piece");
                let assigned = state.try_assign_n(blocks_to_request, &mut blocks);
                blocks_to_request -= assigned;
                assigned_pieces.insert(*piece);
                if blocks_to_request == 0 {
                    break;
                }
            }
        }

        for piece in &assigned_pieces {
            let state = self.pieces.get_mut(&piece).expect("invalid piece");
            assert!(
                self.pieces_by_priority.remove(&Reverse(state.priority)),
                "piece priority should be present"
            );
            state.priority.has_assigned_blocks = true;
            self.pieces_by_priority.insert(Reverse(state.priority));
        }

        blocks
    }
}

// TODO: rename
#[derive(Debug, PartialEq, Eq)]
pub enum HaveResult {
    None,
    Interested,
    InterestedAndRequest(Vec<Block>),
    Request(Vec<Block>),
}

// -------------------------------------------------------------------------------------------------

#[derive(Debug)]
struct Piece {
    total_blocks: usize,
    unassigned_blocks: Blocks,
    released_blocks: Vec<Block>,
    peers_with_piece: HashSet<SocketAddr>,
    priority: PiecePriority,
}

impl Piece {
    fn new(piece: usize, blocks: Blocks) -> Self {
        Self {
            total_blocks: blocks.len(),
            unassigned_blocks: blocks,
            released_blocks: Vec::new(),
            peers_with_piece: HashSet::new(),
            priority: PiecePriority::new(piece),
        }
    }

    pub fn peer_disconnected(&mut self, addr: &SocketAddr) {
        self.peers_with_piece.remove(addr);
    }

    fn unassign(&mut self, block: Block) {
        self.released_blocks.push(block);
    }

    fn try_assign_n(&mut self, n: usize, blocks: &mut Vec<Block>) -> usize {
        let mut assigned = 0;
        // Use released blocks first
        let released_n = n.min(self.released_blocks.len());
        blocks.extend(self.released_blocks.drain(0..released_n));
        assigned += released_n;

        // Assign remaining from unassigned blocks
        while assigned < n {
            if let Some(block) = self.unassigned_blocks.next() {
                blocks.push(block);
                assigned += 1;
            } else {
                break;
            }
        }

        assigned
    }
}

// -------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PiecePriority {
    piece: usize,
    has_assigned_blocks: bool,
    // TODO: this can probably be a u16 or even u8
    num_peers_with_piece: usize,
}

impl Ord for PiecePriority {
    fn cmp(&self, other: &Self) -> Ordering {
        // Favor pieces which are already in flight in order to reduce fragmentation
        self.has_assigned_blocks
            .cmp(&other.has_assigned_blocks)
            // Otherwise, favor rarest piece first
            .then(other.num_peers_with_piece.cmp(&self.num_peers_with_piece))
            .then(self.piece.cmp(&other.piece))
    }
}

impl PartialOrd for PiecePriority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PiecePriority {
    fn new(piece: usize) -> Self {
        Self {
            piece,
            has_assigned_blocks: false,
            num_peers_with_piece: 0,
        }
    }
}

// -------------------------------------------------------------------------------------------------

#[derive(Debug)]
struct Peer {
    /// If the peer is choking the clinet
    choking: bool,
    /// Pieces the peer has and the client doesn't
    peer_pieces: BitSet,
    /// Blocks assigned to be downloaded from the peer
    assigned_blocks: HashSet<Block>,
}

impl Default for Peer {
    fn default() -> Self {
        Self {
            choking: true,
            peer_pieces: BitSet::default(),
            assigned_blocks: HashSet::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use size::Size;

    use crate::peer::tests::{test_config, test_torrent};

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

        assert_eq!(scheduler.peer_has_piece(addr, 0), HaveResult::None);
        assert!(scheduler.peer_unchoked(addr).is_empty());
    }

    #[test]
    fn assign_a_block_to_request_from_peer() {
        let mut scheduler = test_scheduler();
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert_eq!(scheduler.peer_has_piece(addr, 0), HaveResult::Interested);
        assert_eq!(scheduler.peer_unchoked(addr), vec![Block::new(0, 0, 8)]);
    }

    #[test]
    fn peer_unchoked_before_notifying_on_completed_piece() {
        let mut scheduler = test_scheduler();
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert!(scheduler.peer_unchoked(addr).is_empty());
        assert_eq!(
            scheduler.peer_has_piece(addr, 0),
            HaveResult::InterestedAndRequest(vec![Block::new(0, 0, 8)])
        );
    }

    #[test]
    fn distribute_same_piece_between_two_peers() {
        let mut scheduler = test_scheduler();

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        assert_eq!(scheduler.peer_has_piece(addr1, 0), HaveResult::Interested);

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        assert_eq!(scheduler.peer_has_piece(addr2, 0), HaveResult::Interested);

        // Both peers have piece #0. Distribute its blocks among them.
        assert_eq!(scheduler.peer_unchoked(addr1), vec![Block::new(0, 0, 8)]);
        assert_eq!(scheduler.peer_unchoked(addr2), vec![Block::new(0, 8, 8)]);
    }

    #[test]
    fn select_rarest_pieces_first() {
        let mut scheduler = test_scheduler();

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        assert_eq!(scheduler.peer_has_piece(addr1, 0), HaveResult::Interested);

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        assert_eq!(scheduler.peer_has_piece(addr2, 0), HaveResult::Interested);
        assert_eq!(scheduler.peer_has_piece(addr2, 1), HaveResult::None);

        // Peer 2 has both piece #0 and #1. Since piece #1 is rarer, select it first.
        assert_eq!(scheduler.peer_unchoked(addr2), vec![Block::new(1, 0, 8)]);

        // Peer 1 only has piece #0, select it.
        assert_eq!(scheduler.peer_unchoked(addr1), vec![Block::new(0, 0, 8)]);
    }

    #[test]
    fn prioritize_pieces_that_already_started_downloading() {
        let mut scheduler = test_scheduler();

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        assert_eq!(scheduler.peer_has_piece(addr1, 0), HaveResult::Interested);

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        assert_eq!(scheduler.peer_has_piece(addr2, 0), HaveResult::Interested);
        assert_eq!(scheduler.peer_has_piece(addr2, 1), HaveResult::None);

        // Peer 1 only has piece #0, select it.
        assert_eq!(scheduler.peer_unchoked(addr1), vec![Block::new(0, 0, 8)]);

        // Peer 2 has both piece #0 and #1. Piece #1 is rarer, but peer 1 already started
        // downloading piece #0, so prioritize it first.
        assert_eq!(scheduler.peer_unchoked(addr2), vec![Block::new(0, 8, 8)]);
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
