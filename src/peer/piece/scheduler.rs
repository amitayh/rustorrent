#![allow(dead_code)]
use std::{
    collections::{BTreeSet, HashMap, HashSet, hash_map::Entry},
    net::SocketAddr,
    sync::Arc,
};

use bit_set::BitSet;

use crate::{
    message::Block,
    peer::{Download, blocks::Blocks},
};

/// The scheduler only keeps track of pieces the client still doesn't have
pub struct Scheduler {
    download: Arc<Download>,
    peers: HashMap<SocketAddr, Peer>,

    /// Pieces that no peer has announced to have yet (with Have / Bitfield messages)
    orphan_pieces: BitSet,

    /// Pieces that are known to be available by at least one peer
    available_pieces: PiecePriorityMap,

    /// Pieces that are currently selected for download
    active_pieces: HashMap<usize, Piece>,
}

impl Scheduler {
    pub fn new(download: Arc<Download>, has_pieces: BitSet) -> Self {
        let total_pieces = download.torrent.info.total_pieces();
        let missing_pieces = (0..total_pieces).filter(|piece| !has_pieces.contains(*piece));
        let orphan_pieces = BitSet::from_iter(missing_pieces);
        Self {
            download,
            peers: HashMap::new(),
            orphan_pieces,
            available_pieces: PiecePriorityMap::new(),
            active_pieces: HashMap::new(),
        }
    }

    pub fn peer_choked(&mut self, addr: SocketAddr) {
        let peer = self.peers.entry(addr).or_default();
        peer.choking = true;
        for block in peer.assigned_blocks.drain() {
            //let piece = self.pieces.get_mut(&block.piece).expect("invalid piece");
            //piece.unassign(block);
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

    pub fn block_downloaded(&mut self, addr: &SocketAddr, block: &Block) -> Vec<Block> {
        let peer = self.peers.get_mut(addr).expect("invalid peer");
        assert!(
            peer.assigned_blocks.remove(block),
            "peer should have the block assigned"
        );

        let piece = self
            .active_pieces
            .get_mut(&block.piece)
            .expect("invalid piece");

        piece.downloaded_blocks += 1;
        if piece.is_complete() {
            self.active_pieces.remove(&block.piece);
        }

        self.assign(addr)
    }

    pub fn release(&mut self, addr: &SocketAddr, block: Block) -> Vec<Block> {
        self.active_pieces
            .get_mut(&block.piece)
            .expect("invalid piece")
            .unassign(block);

        if let Some(peer) = self.peers.get_mut(addr) {
            assert!(
                peer.assigned_blocks.remove(&block),
                "peer should have the block assigned"
            );
            return self.assign(addr);
        }

        Vec::new()
    }

    pub fn peer_has_piece(&mut self, addr: SocketAddr, piece: usize) -> HaveResult {
        if self.orphan_pieces.remove(piece) {
            let state = Piece::new(piece, self.download.blocks(piece), addr);
            self.available_pieces.insert(state);
        } else if self.available_pieces.contains(&piece) {
            self.available_pieces.peer_has_piece(&piece, addr);
        } else if self.active_pieces.contains_key(&piece) {
            let state = self.active_pieces.get_mut(&piece).expect("invalid piece");
            state.peers_with_piece.insert(addr);
        } else {
            // Ignore if client already has piece
            return HaveResult::None;
        }

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
        HashSet::new()
    }

    pub fn peer_disconnected(&mut self, addr: &SocketAddr) {
        if let Some(mut peer) = self.peers.remove(addr) {
            for block in peer.assigned_blocks.drain() {
                //let piece = self.pieces.get_mut(&block.piece).expect("invalid piece");
                //piece.peer_disconnected(addr);
                //piece.unassign(block);
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

        // Check if there's an active piece to assign from
        for (piece, state) in &mut self.active_pieces {
            if peer.peer_pieces.contains(*piece) {
                let assigned = state.try_assign_n(blocks_to_request, &mut blocks);
                blocks_to_request -= assigned;
                assigned_pieces.insert(*piece);
                if blocks_to_request == 0 {
                    break;
                }
            }
        }

        if blocks_to_request > 0 {
            // Check if there's an available piece to assign from
            let mut priorities_to_remove = Vec::new();
            for (_, piece) in &self.available_pieces.priorities {
                if peer.peer_pieces.contains(*piece) {
                    let mut state = self
                        .available_pieces
                        .pieces
                        .remove(piece)
                        .expect("invalud piece");
                    priorities_to_remove.push(state.priority());
                    let assigned = state.try_assign_n(blocks_to_request, &mut blocks);
                    self.active_pieces.insert(*piece, state);
                    blocks_to_request -= assigned;
                    assigned_pieces.insert(*piece);
                    if blocks_to_request == 0 {
                        break;
                    }
                }
            }
            for priority in &priorities_to_remove {
                assert!(
                    self.available_pieces.priorities.remove(priority),
                    "piece priority should be present"
                );
            }
        }

        peer.assigned_blocks.extend(&blocks);

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

struct PiecePriorityMap {
    pieces: HashMap<usize, Piece>,
    priorities: BTreeSet<(usize, usize)>,
}

impl PiecePriorityMap {
    fn new() -> Self {
        Self {
            pieces: HashMap::new(),
            priorities: BTreeSet::new(),
        }
    }

    fn insert(&mut self, piece: Piece) {
        self.priorities.insert(piece.priority());
        self.pieces.insert(piece.index, piece);
    }

    fn contains(&self, piece: &usize) -> bool {
        self.pieces.contains_key(piece)
    }

    fn peer_has_piece(&mut self, piece: &usize, addr: SocketAddr) {
        let state = self.pieces.get_mut(piece).expect("invalid piece");
        assert!(
            self.priorities.remove(&state.priority()),
            "piece priority should be present"
        );
        state.peers_with_piece.insert(addr);
        self.priorities.insert(state.priority());
    }

    fn remove(&mut self, piece: &usize) -> Piece {
        let state = self.pieces.remove(piece).expect("invalid piece");
        assert!(
            self.priorities.remove(&state.priority()),
            "piece priority should be present"
        );
        state
    }
}

// -------------------------------------------------------------------------------------------------

#[derive(Debug)]
struct Piece {
    index: usize,
    total_blocks: usize,
    downloaded_blocks: usize,
    unassigned_blocks: Blocks,
    released_blocks: Vec<Block>,
    peers_with_piece: HashSet<SocketAddr>,
}

impl Piece {
    fn new(piece: usize, blocks: Blocks, addr: SocketAddr) -> Self {
        Self {
            index: piece,
            total_blocks: blocks.len(),
            downloaded_blocks: 0,
            unassigned_blocks: blocks,
            released_blocks: Vec::new(),
            peers_with_piece: HashSet::from_iter([addr]),
        }
    }

    fn priority(&self) -> (usize, usize) {
        (self.peers_with_piece.len(), self.index)
    }

    fn peer_disconnected(&mut self, addr: &SocketAddr) {
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

    fn is_complete(&self) -> bool {
        self.downloaded_blocks == self.total_blocks
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

// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use size::Size;

    use crate::peer::tests::{test_config, test_torrent};

    use super::*;

    #[test]
    fn peer_unchoked_but_has_no_pieces() {
        let mut scheduler = test_scheduler(&[]);
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert!(scheduler.peer_unchoked(addr).is_empty());
    }

    #[test]
    fn peer_unchoked_but_client_already_has_that_piece() {
        let mut scheduler = test_scheduler(&[0]);
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert_eq!(scheduler.peer_has_piece(addr, 0), HaveResult::None);
        assert!(scheduler.peer_unchoked(addr).is_empty());
    }

    #[test]
    fn assign_a_block_to_request_from_peer() {
        let mut scheduler = test_scheduler(&[]);
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert_eq!(scheduler.peer_has_piece(addr, 0), HaveResult::Interested);
        assert_eq!(scheduler.peer_unchoked(addr), vec![Block::new(0, 0, 8)]);
    }

    #[test]
    fn peer_unchoked_before_notifying_on_completed_piece() {
        let mut scheduler = test_scheduler(&[]);
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert!(scheduler.peer_unchoked(addr).is_empty());
        assert_eq!(
            scheduler.peer_has_piece(addr, 0),
            HaveResult::InterestedAndRequest(vec![Block::new(0, 0, 8)])
        );
    }

    #[test]
    fn distribute_same_piece_between_two_peers() {
        let mut scheduler = test_scheduler(&[]);

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
        let mut scheduler = test_scheduler(&[]);

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
        let mut scheduler = test_scheduler(&[]);

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

    #[test]
    fn continue_to_next_block_after_previous_block_completed_downloading() {
        let mut scheduler = test_scheduler(&[]);

        let addr = "127.0.0.1:6881".parse().unwrap();
        let block1 = Block::new(0, 0, 8);
        let block2 = Block::new(0, 8, 8);

        assert_eq!(scheduler.peer_has_piece(addr, 0), HaveResult::Interested);
        assert_eq!(scheduler.peer_unchoked(addr), vec![block1]);

        // Continue with next block once previous one completes
        assert_eq!(scheduler.block_downloaded(&addr, &block1), vec![block2]);
    }

    #[test]
    fn release_abandoned_block() {
        let mut scheduler = test_scheduler(&[]);
        let addr = "127.0.0.1:6881".parse().unwrap();
        let block = Block::new(0, 0, 8);

        assert_eq!(scheduler.peer_has_piece(addr, 0), HaveResult::Interested);
        assert_eq!(scheduler.peer_unchoked(addr), vec![block]);

        // Block abandoned, mark it as unassigned and re-assigns the same abandoned block
        assert_eq!(scheduler.release(&addr, block), vec![block]);
    }

    #[test]
    fn release_block_after_peer_disconnected() {
        let mut scheduler = test_scheduler(&[]);
        let block = Block::new(0, 0, 8);

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        assert_eq!(scheduler.peer_has_piece(addr1, 0), HaveResult::Interested);
        assert_eq!(scheduler.peer_unchoked(addr1), vec![block]);

        scheduler.peer_disconnected(&addr1);
        assert!(scheduler.release(&addr1, block).is_empty());

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        assert_eq!(scheduler.peer_has_piece(addr2, 0), HaveResult::Interested);
        assert_eq!(scheduler.peer_unchoked(addr2), vec![block]);
    }

    fn test_scheduler(has_pieces: &[usize]) -> Scheduler {
        let torrent = test_torrent();
        let config = test_config("/tmp")
            .with_block_size(Size::from_bytes(8))
            .with_max_concurrent_requests_per_peer(1);
        let download = Arc::new(Download { torrent, config });
        Scheduler::new(download, BitSet::from_iter(has_pieces.iter().copied()))
    }
}
