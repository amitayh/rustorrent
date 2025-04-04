use std::cmp::Reverse;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::{collections::HashMap, net::SocketAddr};

use bit_set::BitSet;

use crate::message::Block;
use crate::peer::Download;
use crate::peer::blocks::Blocks;

/// Allocates blocks to request from peers. Optimizes rarest piece first while trying to ditribute
/// the blocks between peers who have them.
#[derive(Debug)]
pub struct Allocator {
    download: Arc<Download>,
    peers: HashMap<SocketAddr, PeerState>,
    pieces: Vec<PieceState>,
    pub has_pieces: BitSet,
}

impl Allocator {
    pub fn new(download: Arc<Download>, has_pieces: BitSet) -> Self {
        let total_pieces = download.torrent.info.total_pieces();
        let mut pieces = Vec::with_capacity(total_pieces);
        for piece in 0..total_pieces {
            let blocks = download.blocks(piece);
            pieces.push(PieceState::new(blocks));
        }

        Self {
            download,
            peers: HashMap::new(),
            pieces,
            has_pieces,
        }
    }

    pub fn is_available(&self, piece: usize) -> bool {
        self.has_pieces.contains(piece)
    }

    /// Returns peers that are no longer interesting (don't have any piece we don't already have)
    pub fn client_has_piece(&mut self, piece: usize) -> HashSet<SocketAddr> {
        self.has_pieces.insert(piece);
        self.peers
            .iter_mut()
            .filter_map(|(addr, peer)| {
                if peer.update_interest(&self.has_pieces) {
                    Some(*addr)
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn peer_has_pieces(&mut self, addr: SocketAddr, pieces: &BitSet) -> (bool, Option<Block>) {
        let peer = self.peers.entry(addr).or_default();
        peer.has_pieces.union_with(pieces);
        let became_interesting = peer.update_interest(&self.has_pieces);
        for piece in pieces {
            let piece = self.pieces.get_mut(piece).expect("invalid piece");
            piece.peer_has_piece();
        }
        (became_interesting, self.assign(&addr))
    }

    /// Returns a block to request form peer who unchoked if possible
    pub fn peer_unchoked(&mut self, addr: SocketAddr) -> Option<Block> {
        let peer = self.peers.entry(addr).or_default();
        if !peer.choking {
            return None;
        }
        peer.choking = false;
        self.assign(&addr)
    }

    pub fn peer_choked(&mut self, addr: SocketAddr) {
        let peer = self.peers.entry(addr).or_default();
        peer.choking = true;
        for block in peer.assigned_blocks.drain() {
            let piece = self.pieces.get_mut(block.piece).expect("invalid piece");
            piece.release(&block);
        }
    }

    pub fn release(&mut self, addr: &SocketAddr, block: Block) -> Option<Block> {
        let piece = self.pieces.get_mut(block.piece).expect("invalid piece");
        piece.release(&block);
        if let Some(peer) = self.peers.get_mut(addr) {
            if peer.assigned_blocks.remove(&block) {
                return self.assign(addr);
            }
        }
        None
    }

    // TODO: test
    pub fn block_in_flight(&self, addr: &SocketAddr, block: &Block) -> bool {
        let peer = self.peers.get(addr).expect("invalid peer");
        peer.assigned_blocks.contains(block)
    }

    pub fn block_downloaded(&mut self, addr: &SocketAddr, block: &Block) -> Option<Block> {
        let piece = self.pieces.get_mut(block.piece).expect("invalid piece");
        piece.block_downloaded(block);
        let peer = self.peers.get_mut(addr)?;
        if peer.assigned_blocks.remove(block) {
            return self.assign(addr);
        }
        None
    }

    // TODO: test
    pub fn invalidate(&mut self, piece: usize) {
        let state = self.pieces.get_mut(piece).expect("invalid piece");
        assert!(state.assigned_blocks.is_empty());
        let blocks = self.download.blocks(piece);
        state.unassigned_blocks = blocks.collect();
    }

    pub fn peer_disconnected(&mut self, addr: &SocketAddr) {
        if let Some(mut peer) = self.peers.remove(addr) {
            for block in peer.assigned_blocks.drain() {
                let piece = self.pieces.get_mut(block.piece).expect("invalid piece");
                piece.release(&block);
            }
        }
    }

    fn assign(&mut self, addr: &SocketAddr) -> Option<Block> {
        let peer = self.peers.get_mut(addr).expect("invalid peer");
        if peer.choking || peer.has_pieces.difference(&self.has_pieces).count() == 0 {
            return None;
        }
        let (piece, _) = peer
            .has_pieces
            .iter()
            .map(|piece| (piece, self.pieces.get(piece).expect("invalid piece")))
            .filter(|(_, piece)| piece.is_eligible())
            .max_by_key(|(_, piece)| piece.priority())?;

        let block = self.pieces.get_mut(piece)?.select_block()?;
        peer.assigned_blocks.insert(block);
        Some(block)
    }
}

#[derive(Debug)]
struct PeerState {
    /// If the peer is choking the clinet
    choking: bool,
    /// Available pieces peer has
    has_pieces: BitSet,
    /// Blocks assigned to be downloaded from the peer
    assigned_blocks: HashSet<Block>,
    /// If the peer is interesting to the client (has pieces the client doesn't)
    interesting: bool,
}

impl PeerState {
    /// Returns whether there was a change in interest. i.e:
    /// changed from interesting -> to not interesting, or
    /// change from not interesting -> to interesting.
    fn update_interest(&mut self, client_pieces: &BitSet) -> bool {
        let mut remaining_pieces = self.has_pieces.difference(client_pieces);
        let was_interesting = self.interesting;
        self.interesting = remaining_pieces.next().is_some();
        was_interesting ^ self.interesting
    }
}

impl Default for PeerState {
    fn default() -> Self {
        Self {
            choking: true,
            has_pieces: BitSet::new(),
            assigned_blocks: HashSet::new(),
            interesting: false,
        }
    }
}

#[derive(Debug)]
struct PieceState {
    unassigned_blocks: VecDeque<Block>,
    /// Blocks assigned from this piece
    assigned_blocks: HashSet<Block>,
    /// Number of peers that have this piece
    peers_with_piece: usize,
}

impl PieceState {
    fn new(blocks: Blocks) -> Self {
        let unassigned_blocks: VecDeque<_> = blocks.collect();
        let assigned_blocks = HashSet::with_capacity(unassigned_blocks.len());
        Self {
            unassigned_blocks,
            assigned_blocks,
            peers_with_piece: 0,
        }
    }

    fn peer_has_piece(&mut self) {
        self.peers_with_piece += 1;
    }

    fn select_block(&mut self) -> Option<Block> {
        let block = self.unassigned_blocks.pop_front()?;
        self.assigned_blocks.insert(block);
        Some(block)
    }

    fn release(&mut self, block: &Block) {
        // TODO: assert block belongs to piece?
        if let Some(block) = self.assigned_blocks.take(block) {
            self.unassigned_blocks.push_front(block);
        }
    }

    fn priority(&self) -> (bool, Reverse<usize>) {
        (
            // Favor pieces which are already in flight in order to reduce fragmentation
            !self.assigned_blocks.is_empty(),
            // Otherwise, favor rarest piece first
            Reverse(self.peers_with_piece),
        )
    }

    // TODO: test
    fn is_eligible(&self) -> bool {
        !self.unassigned_blocks.is_empty()
    }

    fn block_downloaded(&mut self, block: &Block) {
        self.assigned_blocks.remove(block);
    }
}

#[cfg(test)]
mod tests {
    /*
    use super::*;

    use size::Size;

    fn sizes() -> Sizes {
        Sizes::new(
            Size::from_bytes(24),
            Size::from_bytes(32),
            Size::from_bytes(8),
        )
    }

    #[test]
    fn peer_unchoked_but_has_no_pieces() {
        let mut allocator = Allocator::new(sizes(), BitSet::new());
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert!(allocator.peer_unchoked(addr).is_none());
    }

    #[test]
    fn peer_unchoked_but_client_already_has_that_piece() {
        let pieces = BitSet::from_iter([0]);
        let mut allocator = Allocator::new(sizes(), pieces.clone());
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert_eq!(allocator.peer_has_pieces(addr, &pieces), (false, None));
        assert!(allocator.peer_unchoked(addr).is_none());
    }

    #[test]
    fn assign_a_block_to_request_from_peer() {
        let mut allocator = Allocator::new(sizes(), BitSet::new());
        let addr = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);

        assert_eq!(allocator.peer_has_pieces(addr, &pieces), (true, None));
        assert_eq!(allocator.peer_unchoked(addr), Some(Block::new(0, 0, 8)));
    }

    #[test]
    fn peer_unchoked_before_notifying_on_completed_piece() {
        let mut allocator = Allocator::new(sizes(), BitSet::new());
        let addr = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);

        assert!(allocator.peer_unchoked(addr).is_none());
        assert_eq!(
            allocator.peer_has_pieces(addr, &pieces),
            (true, Some(Block::new(0, 0, 8)))
        );
    }

    #[test]
    fn distribute_same_piece_between_two_peers() {
        let mut allocator = Allocator::new(sizes(), BitSet::new());

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);
        assert!(allocator.peer_has_pieces(addr1, &pieces).1.is_none());

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        assert!(allocator.peer_has_pieces(addr2, &pieces).1.is_none());

        // Both peers have piece #0. Distribute its blocks among them.
        assert_eq!(allocator.peer_unchoked(addr1), Some(Block::new(0, 0, 8)));
        assert_eq!(allocator.peer_unchoked(addr2), Some(Block::new(0, 8, 8)));
    }

    #[test]
    fn select_rarest_pieces_first() {
        let mut allocator = Allocator::new(sizes(), BitSet::new());

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);
        assert!(allocator.peer_has_pieces(addr1, &pieces).1.is_none());

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0, 1]);
        assert!(allocator.peer_has_pieces(addr2, &pieces).1.is_none());

        // Peer 2 has both piece #0 and #1. Since piece #1 is rarer, select it first.
        assert_eq!(allocator.peer_unchoked(addr2), Some(Block::new(1, 0, 8)));

        // Peer 1 only has piece #0, select it.
        assert_eq!(allocator.peer_unchoked(addr1), Some(Block::new(0, 0, 8)));
    }

    #[test]
    fn prioritize_pieces_that_already_started_downloading() {
        let mut allocator = Allocator::new(sizes(), BitSet::new());

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);
        assert!(allocator.peer_has_pieces(addr1, &pieces).1.is_none());

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0, 1]);
        assert!(allocator.peer_has_pieces(addr2, &pieces).1.is_none());

        // Peer 1 only has piece #0, select it.
        assert_eq!(allocator.peer_unchoked(addr1), Some(Block::new(0, 0, 8)));

        // Peer 2 has both piece #0 and #1. Piece #1 is rarer, but peer 1 already started
        // downloading piece #0, so prioritize it first.
        assert_eq!(allocator.peer_unchoked(addr2), Some(Block::new(0, 8, 8)));
    }

    #[test]
    fn continue_to_next_block_after_previous_block_completed_downloading() {
        let mut allocator = Allocator::new(sizes(), BitSet::new());

        let addr = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);
        let block1 = Block::new(0, 0, 8);
        let block2 = Block::new(0, 8, 8);

        assert!(allocator.peer_has_pieces(addr, &pieces).1.is_none());
        assert_eq!(allocator.peer_unchoked(addr), Some(block1));

        // Continue with next block once previous one completes
        assert_eq!(allocator.block_downloaded(&addr, &block1), Some(block2));
    }

    #[test]
    fn release_abandoned_block() {
        let mut allocator = Allocator::new(sizes(), BitSet::new());
        let addr = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);
        let block = Block::new(0, 0, 8);

        assert!(allocator.peer_has_pieces(addr, &pieces).1.is_none());
        assert_eq!(allocator.peer_unchoked(addr), Some(block));

        // Block abandoned, mark it as unassigned and re-assigns the same abandoned block
        assert_eq!(allocator.release(&addr, block), Some(block));
    }

    #[test]
    fn release_block_after_peer_disconnected() {
        let mut allocator = Allocator::new(sizes(), BitSet::new());
        let block = Block::new(0, 0, 8);

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);
        assert!(allocator.peer_has_pieces(addr1, &pieces).1.is_none());
        assert_eq!(allocator.peer_unchoked(addr1), Some(block));

        allocator.peer_disconnected(&addr1);
        assert!(allocator.release(&addr1, block).is_none());

        let addr2 = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);
        assert!(allocator.peer_has_pieces(addr2, &pieces).1.is_none());
        assert_eq!(allocator.peer_unchoked(addr2), Some(block));
        //// Peer 2 has only piece #0
        //let addr2 = "127.0.0.2:6881".parse().unwrap();
        //let pieces = BitSet::from_iter([0]);
        //assert!(allocator.peer_has_pieces(addr2, &pieces).1.is_none());

        //// Once piece #0 is downloaded, we're no longer interested in peer 2
        //let not_interesting = allocator.client_has_piece(0);
        //assert_eq!(not_interesting.len(), 1);
        //assert!(not_interesting.contains(&addr2));
    }

    #[test]
    fn assign_abandoned_block_to_other_peer_if_needed() {
        let mut allocator = Allocator::new(sizes(), BitSet::new());

        let pieces = BitSet::from_iter([0]);
        let block = Block::new(0, 0, 8);

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        assert!(allocator.peer_has_pieces(addr1, &pieces).1.is_none());
        assert_eq!(allocator.peer_unchoked(addr1), Some(block));

        // Peer 1 choked before completing the block, assign to peer 2
        allocator.peer_choked(addr1);

        let addr2 = "127.0.0.1:6881".parse().unwrap();
        assert!(allocator.peer_has_pieces(addr2, &pieces).1.is_none());
        assert_eq!(allocator.peer_unchoked(addr2), Some(block));
    }

    #[test]
    fn do_not_reassign_downloaded_block() {
        let mut allocator = Allocator::new(sizes(), BitSet::new());

        let pieces = BitSet::from_iter([0]);
        let block1 = Block::new(0, 0, 8);
        let block2 = Block::new(0, 8, 8);

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        assert!(allocator.peer_has_pieces(addr1, &pieces).1.is_none());
        assert_eq!(allocator.peer_unchoked(addr1), Some(block1));

        // Peer 1 completed downloading block #1 and choked. Assign next block to peer 2
        assert_eq!(allocator.block_downloaded(&addr1, &block1), Some(block2));
        allocator.peer_choked(addr1);

        let addr2 = "127.0.0.1:6881".parse().unwrap();
        assert!(allocator.peer_has_pieces(addr2, &pieces).1.is_none());
        assert_eq!(allocator.peer_unchoked(addr2), Some(block2));
    }

    #[test]
    fn find_all_peers_that_are_no_longer_interesting() {
        let mut allocator = Allocator::new(sizes(), BitSet::new());

        // Peer 1 has pieces #0 and #1
        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0, 1]);
        assert!(allocator.peer_has_pieces(addr1, &pieces).1.is_none());

        // Peer 2 has only piece #0
        let addr2 = "127.0.0.2:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);
        assert!(allocator.peer_has_pieces(addr2, &pieces).1.is_none());

        // Once piece #0 is downloaded, we're no longer interested in peer 2
        let not_interesting = allocator.client_has_piece(0);
        assert_eq!(not_interesting.len(), 1);
        assert!(not_interesting.contains(&addr2));
    }
    */
}
