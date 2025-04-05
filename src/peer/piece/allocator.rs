use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet, VecDeque};
use std::sync::Arc;
use std::{collections::HashMap, net::SocketAddr};

use bit_set::BitSet;
use priority_queue::PriorityQueue;

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

    pub fn peer_has_pieces(&mut self, addr: SocketAddr, pieces: &BitSet) -> (bool, Vec<Block>) {
        let peer = self.peers.entry(addr).or_default();
        peer.has_pieces(pieces);
        let became_interesting = peer.update_interest(&self.has_pieces);
        for piece in pieces {
            let piece_state = &mut self.pieces[piece];
            piece_state.peer_has_piece(addr);
            let piece_priority = PiecePriority::from(piece_state);
            for peer in self.peers.values_mut() {
                if peer.has_pieces.contains(piece) {
                    peer.peer_pieces.change_priority(&piece, piece_priority);
                }
            }
        }
        (became_interesting, self.assign(&addr))
    }

    /// Returns a block to request form peer who unchoked if possible
    pub fn peer_unchoked(&mut self, addr: SocketAddr) -> Vec<Block> {
        let peer = self.peers.entry(addr).or_default();
        if !peer.choking {
            // Client was already unchoked, do nothing
            return Vec::new();
        }
        peer.choking = false;
        self.assign(&addr)
    }

    pub fn peer_choked(&mut self, addr: SocketAddr) {
        let peer = self.peers.entry(addr).or_default();
        peer.choking = true;
        for block in peer.assigned_blocks.drain() {
            let piece = &mut self.pieces[block.piece];
            piece.release(&addr, &block);
        }
    }

    pub fn release(&mut self, addr: &SocketAddr, block: Block) -> Vec<Block> {
        let piece = &mut self.pieces[block.piece];
        piece.release(addr, &block);
        if let Some(peer) = self.peers.get_mut(addr) {
            if peer.assigned_blocks.remove(&block) {
                return self.assign(addr);
            }
        }
        Vec::new()
    }

    // TODO: test
    pub fn block_in_flight(&self, addr: &SocketAddr, block: &Block) -> bool {
        let peer = &self.peers[addr];
        peer.assigned_blocks.contains(block)
    }

    pub fn block_downloaded(&mut self, addr: &SocketAddr, block: &Block) -> Vec<Block> {
        let piece = &mut self.pieces[block.piece];
        piece.block_downloaded(block);
        let peer = self.peers.get_mut(addr).expect("invalid peer");
        if peer.assigned_blocks.remove(block) {
            return self.assign(addr);
        }
        Vec::new()
    }

    // TODO: test
    pub fn invalidate(&mut self, piece: usize) {
        let state = &mut self.pieces[piece];
        assert!(state.assigned_blocks.is_empty());
        let blocks = self.download.blocks(piece);
        state.unassigned_blocks = blocks.collect();
    }

    pub fn peer_disconnected(&mut self, addr: &SocketAddr) {
        if let Some(mut peer) = self.peers.remove(addr) {
            for block in peer.assigned_blocks.drain() {
                let piece = &mut self.pieces[block.piece];
                piece.release(addr, &block);
            }
        }
    }

    fn assign(&mut self, addr: &SocketAddr) -> Vec<Block> {
        let mut blocks = Vec::new();
        while let Some(block) = self.assign_one(addr) {
            blocks.push(block);
        }
        blocks
    }

    fn assign_one(&mut self, addr: &SocketAddr) -> Option<Block> {
        let peer = self.peers.get_mut(addr).expect("invalid peer");
        if peer.choking || peer.has_pieces.difference(&self.has_pieces).count() == 0 {
            return None;
        }
        if peer.assigned_blocks.len() >= self.download.config.max_concurrent_requests_per_peer {
            return None;
        }
        // TODO: change to heap
        let (piece, _) = peer
            .has_pieces
            .iter()
            .map(|piece| (piece, &self.pieces[piece]))
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
    peer_pieces: PriorityQueue<usize, PiecePriority>,
    /// Blocks assigned to be downloaded from the peer
    assigned_blocks: HashSet<Block>,
    /// If the peer is interesting to the client (has pieces the client doesn't)
    interesting: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PiecePriority {
    has_assigned_blocks: bool,
    peers_with_piece: Reverse<usize>,
}

impl PiecePriority {
    fn from(piece: &PieceState) -> Self {
        Self {
            has_assigned_blocks: !piece.assigned_blocks.is_empty(),
            peers_with_piece: Reverse(piece.peers_with_piece.len()),
        }
    }
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

    fn has_pieces(&mut self, pieces: &BitSet) {
        self.has_pieces.union_with(pieces);
    }
}

impl Default for PeerState {
    fn default() -> Self {
        Self {
            choking: true,
            has_pieces: BitSet::new(),
            peer_pieces: PriorityQueue::new(),
            assigned_blocks: HashSet::new(),
            interesting: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PieceState {
    unassigned_blocks: VecDeque<Block>,
    /// Blocks assigned from this piece
    assigned_blocks: HashSet<Block>,
    /// Number of peers that have this piece
    peers_with_piece: HashSet<SocketAddr>,
}

impl PieceState {
    fn new(blocks: Blocks) -> Self {
        let unassigned_blocks: VecDeque<_> = blocks.collect();
        let assigned_blocks = HashSet::with_capacity(unassigned_blocks.len());
        Self {
            unassigned_blocks,
            assigned_blocks,
            peers_with_piece: HashSet::new(),
        }
    }

    fn peer_has_piece(&mut self, addr: SocketAddr) {
        self.peers_with_piece.insert(addr);
    }

    fn select_block(&mut self) -> Option<Block> {
        let block = self.unassigned_blocks.pop_front()?;
        self.assigned_blocks.insert(block);
        Some(block)
    }

    fn release(&mut self, addr: &SocketAddr, block: &Block) {
        // TODO: assert block belongs to piece?
        self.peers_with_piece.remove(addr);
        if let Some(block) = self.assigned_blocks.take(block) {
            self.unassigned_blocks.push_front(block);
        }
    }

    fn priority(&self) -> (bool, Reverse<usize>) {
        (
            // Favor pieces which are already in flight in order to reduce fragmentation
            !self.assigned_blocks.is_empty(),
            // Otherwise, favor rarest piece first
            Reverse(self.peers_with_piece.len()),
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

impl Ord for PieceState {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        todo!()
    }
}

impl PartialOrd for PieceState {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use size::Size;

    use crate::peer::tests::{create_download, test_config, test_torrent};

    use super::*;

    #[test]
    fn peer_unchoked_but_has_no_pieces() {
        let download = Arc::new(create_download());
        let mut allocator = Allocator::new(download, BitSet::new());
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert!(allocator.peer_unchoked(addr).is_empty());
    }

    #[test]
    fn peer_unchoked_but_client_already_has_that_piece() {
        let download = Arc::new(create_download());
        let pieces = BitSet::from_iter([0]);
        let mut allocator = Allocator::new(download, pieces.clone());
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert_eq!(allocator.peer_has_pieces(addr, &pieces), (false, vec![]));
        assert!(allocator.peer_unchoked(addr).is_empty());
    }

    #[test]
    fn assign_a_block_to_request_from_peer() {
        let download = Arc::new(create_download());
        let mut allocator = Allocator::new(download, BitSet::new());
        let addr = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);

        assert_eq!(allocator.peer_has_pieces(addr, &pieces), (true, vec![]));
        assert_eq!(
            allocator.peer_unchoked(addr),
            vec![Block::new(0, 0, 16384), Block::new(0, 16384, 16384)]
        );
    }

    #[test]
    fn peer_unchoked_before_notifying_on_completed_piece() {
        let download = Arc::new(create_download());
        let mut allocator = Allocator::new(download, BitSet::new());
        let addr = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);

        assert!(allocator.peer_unchoked(addr).is_empty());
        assert_eq!(
            allocator.peer_has_pieces(addr, &pieces),
            (
                true,
                vec![Block::new(0, 0, 16384), Block::new(0, 16384, 16384)]
            )
        );
    }

    #[test]
    fn distribute_same_piece_between_two_peers() {
        let mut allocator = test_allocator();

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);
        assert!(allocator.peer_has_pieces(addr1, &pieces).1.is_empty());

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        assert!(allocator.peer_has_pieces(addr2, &pieces).1.is_empty());

        // Both peers have piece #0. Distribute its blocks among them.
        assert_eq!(allocator.peer_unchoked(addr1), vec![Block::new(0, 0, 8)]);
        assert_eq!(allocator.peer_unchoked(addr2), vec![Block::new(0, 8, 8)]);
    }

    #[test]
    fn select_rarest_pieces_first() {
        let mut allocator = test_allocator();

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);
        assert!(allocator.peer_has_pieces(addr1, &pieces).1.is_empty());

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0, 1]);
        assert!(allocator.peer_has_pieces(addr2, &pieces).1.is_empty());

        // Peer 2 has both piece #0 and #1. Since piece #1 is rarer, select it first.
        assert_eq!(allocator.peer_unchoked(addr2), vec![Block::new(1, 0, 8)]);

        // Peer 1 only has piece #0, select it.
        assert_eq!(allocator.peer_unchoked(addr1), vec![Block::new(0, 0, 8)]);
    }

    #[test]
    fn prioritize_pieces_that_already_started_downloading() {
        let mut allocator = test_allocator();

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);
        assert!(allocator.peer_has_pieces(addr1, &pieces).1.is_empty());

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0, 1]);
        assert!(allocator.peer_has_pieces(addr2, &pieces).1.is_empty());

        // Peer 1 only has piece #0, select it.
        assert_eq!(allocator.peer_unchoked(addr1), vec![Block::new(0, 0, 8)]);

        // Peer 2 has both piece #0 and #1. Piece #1 is rarer, but peer 1 already started
        // downloading piece #0, so prioritize it first.
        assert_eq!(allocator.peer_unchoked(addr2), vec![Block::new(0, 8, 8)]);
    }

    #[test]
    fn continue_to_next_block_after_previous_block_completed_downloading() {
        let mut allocator = test_allocator();

        let addr = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);
        let block1 = Block::new(0, 0, 8);
        let block2 = Block::new(0, 8, 8);

        assert!(allocator.peer_has_pieces(addr, &pieces).1.is_empty());
        assert_eq!(allocator.peer_unchoked(addr), vec![block1]);

        // Continue with next block once previous one completes
        assert_eq!(allocator.block_downloaded(&addr, &block1), vec![block2]);
    }

    #[test]
    fn release_abandoned_block() {
        let mut allocator = test_allocator();
        let addr = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);
        let block = Block::new(0, 0, 8);

        assert!(allocator.peer_has_pieces(addr, &pieces).1.is_empty());
        assert_eq!(allocator.peer_unchoked(addr), vec![block]);

        // Block abandoned, mark it as unassigned and re-assigns the same abandoned block
        assert_eq!(allocator.release(&addr, block), vec![block]);
    }

    #[test]
    fn release_block_after_peer_disconnected() {
        let mut allocator = test_allocator();
        let block = Block::new(0, 0, 8);

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);
        assert!(allocator.peer_has_pieces(addr1, &pieces).1.is_empty());
        assert_eq!(allocator.peer_unchoked(addr1), vec![block]);

        allocator.peer_disconnected(&addr1);
        assert!(allocator.release(&addr1, block).is_empty());

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);
        assert!(allocator.peer_has_pieces(addr2, &pieces).1.is_empty());
        assert_eq!(allocator.peer_unchoked(addr2), vec![block]);
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
        let torrent = test_torrent();
        let config = test_config("/tmp")
            .with_block_size(Size::from_bytes(8))
            .with_max_concurrent_requests_per_peer(1);
        let download = Arc::new(Download { torrent, config });
        let mut allocator = Allocator::new(download, BitSet::new());

        let pieces = BitSet::from_iter([0]);
        let block = Block::new(0, 0, 8);

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        assert!(allocator.peer_has_pieces(addr1, &pieces).1.is_empty());
        assert_eq!(allocator.peer_unchoked(addr1), vec![block]);

        // Peer 1 choked before completing the block, assign to peer 2
        allocator.peer_choked(addr1);

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        assert!(allocator.peer_has_pieces(addr2, &pieces).1.is_empty());
        assert_eq!(allocator.peer_unchoked(addr2), vec![block]);
    }

    #[test]
    fn do_not_reassign_downloaded_block() {
        let mut allocator = test_allocator();

        let pieces = BitSet::from_iter([0]);
        let block1 = Block::new(0, 0, 8);
        let block2 = Block::new(0, 8, 8);

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        assert!(allocator.peer_has_pieces(addr1, &pieces).1.is_empty());
        assert_eq!(allocator.peer_unchoked(addr1), vec![block1]);

        // Peer 1 completed downloading block #1 and choked. Assign next block to peer 2
        assert_eq!(allocator.block_downloaded(&addr1, &block1), vec![block2]);
        allocator.peer_choked(addr1);

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        assert!(allocator.peer_has_pieces(addr2, &pieces).1.is_empty());
        assert_eq!(allocator.peer_unchoked(addr2), vec![block2]);
    }

    #[test]
    fn find_all_peers_that_are_no_longer_interesting() {
        let mut allocator = test_allocator();

        // Peer 1 has pieces #0 and #1
        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0, 1]);
        assert!(allocator.peer_has_pieces(addr1, &pieces).1.is_empty());

        // Peer 2 has only piece #0
        let addr2 = "127.0.0.2:6881".parse().unwrap();
        let pieces = BitSet::from_iter([0]);
        assert!(allocator.peer_has_pieces(addr2, &pieces).1.is_empty());

        // Once piece #0 is downloaded, we're no longer interested in peer 2
        let not_interesting = allocator.client_has_piece(0);
        assert_eq!(not_interesting.len(), 1);
        assert!(not_interesting.contains(&addr2));
    }

    fn test_allocator() -> Allocator {
        let torrent = test_torrent();
        let config = test_config("/tmp")
            .with_block_size(Size::from_bytes(8))
            .with_max_concurrent_requests_per_peer(1);
        let download = Arc::new(Download { torrent, config });
        Allocator::new(download, BitSet::new())
    }
}
