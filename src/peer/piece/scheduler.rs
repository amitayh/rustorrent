#![allow(dead_code)]
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
};

use bit_set::BitSet;

use crate::{
    message::Block,
    peer::{Download, blocks::Blocks},
};

enum PieceState<'a> {
    Orphan,
    Available(&'a mut AvailablePiece),
    Active(&'a mut ActivePiece),
}

/// The scheduler only keeps track of pieces the client still doesn't have
pub struct Scheduler {
    download: Arc<Download>,

    /// Maintains state for connected peers
    peers: HashMap<SocketAddr, Peer>,

    /// Pieces that no peer has announced to have yet (with Have / Bitfield messages)
    orphan_pieces: BitSet,

    /// Pieces that are known to be available by at least one peer
    available_pieces: AvailablePieces,

    /// Pieces that are currently selected for download
    active_pieces: ActivePieces,
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
            available_pieces: AvailablePieces::new(),
            active_pieces: ActivePieces::new(),
        }
    }

    pub fn peer_choked(&mut self, addr: SocketAddr) {
        let peer = self.peers.entry(addr).or_default();
        peer.choking = true;
        for block in peer.assigned_blocks.drain() {
            let piece = self.active_pieces.get_mut(&block.piece);
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
        self.try_assign(&addr)
    }

    pub fn block_downloaded(&mut self, addr: &SocketAddr, block: &Block) -> Vec<Block> {
        let peer = self.peers.get_mut(addr).expect("invalid peer");
        assert!(
            peer.assigned_blocks.remove(block),
            "peer should have the block assigned"
        );

        let piece = self.active_pieces.get_mut(&block.piece);

        piece.downloaded_blocks += 1;
        if piece.is_complete() {
            self.active_pieces.remove(&block.piece);
            //for addr in piece.peers_with_piece {
            //    let peer = self.peers.get_mut(&addr).expect("invalid peer");
            //    peer.has_pieces.remove(piece.index);
            //    if peer.has_pieces.is_empty() {
            //        // Remove peer
            //    }
            //}
        }

        self.try_assign(addr)
    }

    pub fn release(&mut self, addr: &SocketAddr, block: Block) -> Vec<Block> {
        self.active_pieces.get_mut(&block.piece).unassign(block);

        if let Some(peer) = self.peers.get_mut(addr) {
            assert!(
                peer.assigned_blocks.remove(&block),
                "peer should have the block assigned"
            );
            return self.try_assign(addr);
        }

        Vec::new()
    }

    pub fn peer_has_piece(&mut self, addr: SocketAddr, piece: usize) -> HaveResult {
        if self.orphan_pieces.remove(piece) {
            self.available_pieces
                .insert(AvailablePiece::new(piece, addr));
        } else if self.available_pieces.contains(&piece) {
            self.available_pieces.peer_has_piece(&piece, addr);
        } else if self.active_pieces.contains(&piece) {
            let state = self.active_pieces.get_mut(&piece);
            state.peers_with_piece.insert(addr);
        } else {
            // Ignore if client already has piece
            return HaveResult::None;
        }

        // Send "Interested" the first time the peer has piece we want
        let interested = !self.peers.contains_key(&addr);
        let peer = self.peers.entry(addr).or_default();
        peer.has_pieces.insert(piece);
        if peer.choking {
            return if interested {
                HaveResult::Interested
            } else {
                HaveResult::None
            };
        }

        HaveResult::InterestedAndRequest(self.try_assign(&addr))
    }

    /// Returns peers that are no longer interesting (don't have any piece we don't already have)
    pub fn client_has_piece(&mut self, piece: usize) -> HashSet<SocketAddr> {
        let piece = self.active_pieces.remove(&piece);
        for peer in piece.peers_with_piece {
            // Check if peer has any more pieces we want
        }
        HashSet::new()
    }

    pub fn peer_disconnected(&mut self, addr: &SocketAddr) {
        let mut peer = self.peers.remove(addr).expect("invalid peer");

        // Unassign all blocks assigned to peer
        for block in peer.assigned_blocks.drain() {
            let piece = self.active_pieces.get_mut(&block.piece);
            piece.unassign(block);
        }

        // Remove peer association form its pieces
        for piece in &peer.has_pieces {
            if self.available_pieces.contains(&piece) {
                if self.available_pieces.peer_disconnected(&piece, addr) {
                    self.orphan_pieces.insert(piece);
                }
            } else if self.active_pieces.contains(&piece) {
                let active_piece = self.active_pieces.get_mut(&piece);
                if active_piece.peer_disconnected(addr) {
                    self.orphan_pieces.insert(piece);
                }
            } else {
                panic!("peer piece must be either active or available");
            }
        }
    }

    fn try_assign(&mut self, addr: &SocketAddr) -> Vec<Block> {
        let peer = self.peers.get_mut(addr).expect("invalid peer");
        if peer.choking {
            return Vec::new();
        }

        let max_blocks = self.download.config.max_concurrent_requests_per_peer;
        let mut blocks_to_request = max_blocks - peer.assigned_blocks.len();
        let mut blocks = Vec::with_capacity(blocks_to_request);

        // Check if there's an active piece to assign from
        for piece in self.active_pieces.iter_mut() {
            if piece.peers_with_piece.contains(addr) {
                let assigned = piece.try_assign_n(blocks_to_request, &mut blocks);
                blocks_to_request -= assigned;
                if blocks_to_request == 0 {
                    break;
                }
            }
        }

        // Assign remaining blocks from available pieces the peer has
        while blocks_to_request > 0 {
            if let Some(available_piece) = self.available_pieces.next(addr) {
                let mut active_piece = ActivePiece::new(available_piece, &self.download);
                let assigned = active_piece.try_assign_n(blocks_to_request, &mut blocks);
                self.active_pieces.insert(active_piece.index, active_piece);
                blocks_to_request -= assigned;
            } else {
                break;
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

struct AvailablePieces {
    pieces: HashMap<usize, AvailablePiece>,
    priorities: BTreeSet<(usize, usize)>,
}

impl AvailablePieces {
    fn new() -> Self {
        Self {
            pieces: HashMap::new(),
            priorities: BTreeSet::new(),
        }
    }

    fn insert(&mut self, piece: AvailablePiece) {
        self.priorities.insert(piece.priority());
        self.pieces.insert(piece.index, piece);
    }

    fn contains(&self, piece: &usize) -> bool {
        self.pieces.contains_key(piece)
    }

    fn peer_has_piece(&mut self, index: &usize, addr: SocketAddr) {
        let piece = self.pieces.get_mut(index).expect("invalid piece");
        assert!(
            self.priorities.remove(&piece.priority()),
            "piece priority should be present"
        );
        piece.peers_with_piece.insert(addr);
        self.priorities.insert(piece.priority());
    }

    fn peer_disconnected(&mut self, index: &usize, addr: &SocketAddr) -> bool {
        let piece = self.pieces.get_mut(index).expect("invalid piece");
        assert!(
            self.priorities.remove(&piece.priority()),
            "piece priority should be present"
        );
        piece.peers_with_piece.remove(addr);
        if piece.peers_with_piece.is_empty() {
            // No more peers have this piece, it should no longer be considered "available"
            self.pieces.remove(index).unwrap();
            true
        } else {
            // Update priority after peer-set change
            self.priorities.insert(piece.priority());
            false
        }
    }

    fn next(&mut self, addr: &SocketAddr) -> Option<AvailablePiece> {
        let piece = self
            .priorities
            .iter()
            .map(|(_, piece)| self.pieces.get(piece).expect("invalid piece"))
            .find(|piece| piece.peers_with_piece.contains(addr))
            .map(|piece| piece.index)?;

        Some(self.remove(&piece))
    }

    fn remove(&mut self, index: &usize) -> AvailablePiece {
        let piece = self.pieces.remove(index).expect("invalid piece");
        assert!(
            self.priorities.remove(&piece.priority()),
            "piece priority should be present"
        );
        piece
    }
}

struct AvailablePiece {
    index: usize,
    peers_with_piece: HashSet<SocketAddr>,
}

impl AvailablePiece {
    fn new(index: usize, addr: SocketAddr) -> Self {
        Self {
            index,
            peers_with_piece: HashSet::from_iter([addr]),
        }
    }

    fn priority(&self) -> (usize, usize) {
        (self.peers_with_piece.len(), self.index)
    }

    fn peer_disconnected(&mut self, addr: &SocketAddr) {
        assert!(self.peers_with_piece.remove(addr), "peer should have piece");
    }
}

// -------------------------------------------------------------------------------------------------

struct ActivePieces(HashMap<usize, ActivePiece>);

impl ActivePieces {
    fn new() -> Self {
        Self(HashMap::new())
    }

    fn insert(&mut self, index: usize, piece: ActivePiece) {
        self.0.insert(index, piece);
    }

    fn get_mut<'a>(&'a mut self, piece: &usize) -> &'a mut ActivePiece {
        self.0.get_mut(piece).expect("invalid piece")
    }

    fn contains(&self, piece: &usize) -> bool {
        self.0.contains_key(piece)
    }

    fn remove(&mut self, piece: &usize) -> ActivePiece {
        self.0.remove(piece).expect("invalid piece")
    }

    fn iter_mut<'a>(&'a mut self) -> impl Iterator<Item = &'a mut ActivePiece> {
        self.0.values_mut()
    }
}

#[derive(Debug)]
struct ActivePiece {
    index: usize,
    total_blocks: usize,
    downloaded_blocks: usize,
    unassigned_blocks: Blocks,
    released_blocks: Vec<Block>,
    peers_with_piece: HashSet<SocketAddr>,
}

impl ActivePiece {
    fn new(piece: AvailablePiece, download: &Download) -> Self {
        let blocks = download.blocks(piece.index);
        Self {
            index: piece.index,
            total_blocks: blocks.len(),
            downloaded_blocks: 0,
            unassigned_blocks: blocks,
            released_blocks: Vec::new(),
            peers_with_piece: piece.peers_with_piece,
        }
    }

    /// Returns `true` is there are no more connected peers with this piece
    fn peer_disconnected(&mut self, addr: &SocketAddr) -> bool {
        self.peers_with_piece.remove(addr);
        self.peers_with_piece.is_empty()
    }

    fn unassign(&mut self, block: Block) {
        self.released_blocks.push(block);
    }

    fn try_assign_n(&mut self, n: usize, dest: &mut Vec<Block>) -> usize {
        let mut assigned = 0;

        // Use released blocks first
        let released_n = n.min(self.released_blocks.len());
        dest.extend(self.released_blocks.drain(0..released_n));
        assigned += released_n;

        // Assign remaining from unassigned blocks
        while assigned < n {
            if let Some(block) = self.unassigned_blocks.next() {
                dest.push(block);
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
    /// Blocks assigned to be downloaded from the peer
    assigned_blocks: HashSet<Block>,
    /// Pieces the peer has and client doesn't
    has_pieces: BitSet,
}

impl Default for Peer {
    fn default() -> Self {
        Self {
            choking: true,
            assigned_blocks: HashSet::new(),
            has_pieces: BitSet::new(),
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
