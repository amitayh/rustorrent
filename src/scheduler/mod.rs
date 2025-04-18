use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
};

use bit_set::BitSet;
use piece_state::PieceState;

use crate::{client::Download, message::Block};
use active_pieces::*;
use available_pieces::*;
use blocks::*;

mod active_pieces;
mod available_pieces;
mod blocks;
mod piece_state;

/// Manages piece selection and block assignment for downloading pieces from peers.
///
/// The scheduler is responsible for:
/// - Tracking which pieces are available from which peers
/// - Selecting pieces to download based on availability and rarity
/// - Assigning blocks to peers for downloading
/// - Handling peer choke/unchoke events and connection state
/// - Maintaining the state of active piece downloads
///
/// It uses a rarest-first piece selection strategy to prioritize downloading pieces
/// that are less available in the swarm. This helps ensure rare pieces get downloaded
/// and improves overall piece availability.
pub struct Scheduler {
    /// Download metadata and configuration
    download: Arc<Download>,

    /// Maintains state for connected peers
    peers: HashMap<SocketAddr, Peer>,

    /// Pieces that no peer has announced to have yet (with a Have / Bitfield messages)
    orphan_pieces: BitSet,

    /// Pieces that are known to be available by at least one peer
    available_pieces: AvailablePieces,

    /// Pieces that are currently selected for download
    active_pieces: ActivePieces,
}

impl Scheduler {
    pub fn new(download: Arc<Download>, has_pieces: &BitSet) -> Self {
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
            let piece = self.active_pieces.get_mut(block.piece);
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
        self.try_assign(addr)
    }

    pub fn release(&mut self, addr: &SocketAddr, block: Block) -> Vec<Block> {
        if let Some(piece) = self.active_pieces.get_mut_safe(block.piece) {
            piece.unassign(block);
        }

        if let Some(peer) = self.peers.get_mut(addr) {
            peer.assigned_blocks.remove(&block);
            return self.try_assign(addr);
        }

        Vec::new()
    }

    pub fn peer_has_piece(&mut self, addr: SocketAddr, piece: usize) -> PeerPieceResponse {
        if self.orphan_pieces.remove(piece) {
            self.available_pieces
                .insert(AvailablePiece::new(piece, addr));
        } else if self.available_pieces.contains(piece) {
            self.available_pieces.peer_has_piece(piece, addr);
        } else if self.active_pieces.contains(piece) {
            self.active_pieces.peer_has_piece(piece, addr);
        } else {
            // Ignore if client already has piece
            return PeerPieceResponse::NoAction;
        }

        let peer = self.peers.entry(addr).or_default();
        // Send "Interested" the first time the peer has piece we want
        let interested = peer.has_pieces.is_empty();
        peer.has_pieces.insert(piece);

        match (peer.choking, interested) {
            (true, false) => PeerPieceResponse::NoAction,
            (true, true) => PeerPieceResponse::ExpressInterest,
            (false, false) => PeerPieceResponse::RequestBlocks(self.try_assign(&addr)),
            (false, true) => PeerPieceResponse::ExpressInterestAndRequest(self.try_assign(&addr)),
        }
    }

    /// Returns peers that are no longer interesting (don't have any piece we don't already have)
    pub fn client_has_piece(&mut self, piece: usize) -> HashSet<SocketAddr> {
        let mut not_interested = HashSet::new();
        // If a piece is completed it means it was active
        let piece = self.active_pieces.remove(piece);
        for addr in piece.peers() {
            let peer = self.peers.get_mut(addr).expect("invalid peer");
            assert!(
                peer.has_pieces.remove(piece.index),
                "peer should have piece"
            );
            if peer.has_pieces.is_empty() {
                // Peer doesn't have any more piece we need, send NotInterested
                not_interested.insert(*addr);
            }
        }
        not_interested
    }

    pub fn invalidate(&mut self, piece: usize) {
        let active_piece = self.active_pieces.remove(piece);
        let available_piece = AvailablePiece::from(active_piece);
        self.available_pieces.insert(available_piece);
    }

    pub fn block_in_flight(&self, addr: &SocketAddr, block: &Block) -> bool {
        let peer = self.peers.get(addr).expect("invalid peer");
        peer.assigned_blocks.contains(block)
    }

    pub fn peer_disconnected(&mut self, addr: &SocketAddr) {
        if let Some(mut peer) = self.peers.remove(addr) {
            // Unassign all blocks assigned to peer
            for block in peer.assigned_blocks.drain() {
                let piece = self.active_pieces.get_mut(block.piece);
                piece.unassign(block);
            }

            // Remove peer association form its pieces
            for piece in &peer.has_pieces {
                if self.available_pieces.contains(piece) {
                    if self.available_pieces.peer_disconnected(piece, addr) == PieceState::Orphan {
                        self.orphan_pieces.insert(piece);
                    }
                } else if self.active_pieces.contains(piece) {
                    if self.active_pieces.peer_disconnected(piece, addr) == PieceState::Orphan {
                        self.orphan_pieces.insert(piece);
                    }
                } else {
                    panic!("peer piece must be either active or available");
                }
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

        // Try to assign from active pieces first.
        // This is meant to reduce fragmentation.
        blocks_to_request -= self
            .active_pieces
            .try_assign_n(addr, blocks_to_request, &mut blocks);

        // Assign remaining blocks from available pieces the peer has
        while blocks_to_request > 0 {
            if let Some(available_piece) = self.available_pieces.take_next(addr) {
                let piece_blocks = Blocks::for_piece(&self.download, available_piece.index);
                let mut active_piece = ActivePiece::new(available_piece, piece_blocks);
                blocks_to_request -= active_piece.try_assign_n(blocks_to_request, &mut blocks);
                self.active_pieces.insert(active_piece.index, active_piece);
            } else {
                break;
            }
        }

        peer.assigned_blocks.extend(&blocks);

        blocks
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PeerPieceResponse {
    /// No action needed (either we already have the piece or peer is choking)
    NoAction,
    /// Need to express interest in the peer's pieces
    ExpressInterest,
    /// Need to express interest and request blocks from the peer
    ExpressInterestAndRequest(Vec<Block>),
    /// Only need to request blocks from the peer (already interested)
    RequestBlocks(Vec<Block>),
}

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

#[cfg(test)]
mod tests {
    use size::Size;

    use crate::client::Config;
    use crate::client::tests::{test_config, test_torrent};

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

        assert_eq!(
            scheduler.peer_has_piece(addr, 0),
            PeerPieceResponse::NoAction
        );
        assert!(scheduler.peer_unchoked(addr).is_empty());
    }

    #[test]
    fn assign_a_block_to_request_from_peer() {
        let mut scheduler = test_scheduler(&[]);
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert_eq!(
            scheduler.peer_has_piece(addr, 0),
            PeerPieceResponse::ExpressInterest
        );
        assert_eq!(scheduler.peer_unchoked(addr), vec![Block::new(0, 0, 8)]);
    }

    #[test]
    fn peer_unchoked_before_notifying_on_completed_piece() {
        let mut scheduler = test_scheduler(&[]);
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert!(scheduler.peer_unchoked(addr).is_empty());
        assert_eq!(
            scheduler.peer_has_piece(addr, 0),
            PeerPieceResponse::ExpressInterestAndRequest(vec![Block::new(0, 0, 8)])
        );
    }

    #[test]
    fn show_interest_only_once() {
        let config = test_config("/tmp")
            .with_block_size(Size::from_kib(16))
            .with_max_concurrent_requests_per_peer(3);
        let mut scheduler = test_scheduler_with_config(config, &[]);
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert!(scheduler.peer_unchoked(addr).is_empty());
        assert_eq!(
            scheduler.peer_has_piece(addr, 0),
            PeerPieceResponse::ExpressInterestAndRequest(vec![
                Block::new(0, 0, 16384),
                Block::new(0, 16384, 16384)
            ])
        );
        assert_eq!(
            scheduler.peer_has_piece(addr, 1),
            PeerPieceResponse::RequestBlocks(vec![Block::new(1, 0, 16384)])
        );
    }

    #[test]
    fn distribute_same_piece_between_two_peers() {
        let mut scheduler = test_scheduler(&[]);

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        assert_eq!(
            scheduler.peer_has_piece(addr1, 0),
            PeerPieceResponse::ExpressInterest
        );

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        assert_eq!(
            scheduler.peer_has_piece(addr2, 0),
            PeerPieceResponse::ExpressInterest
        );

        // Both peers have piece #0. Distribute its blocks among them.
        assert_eq!(scheduler.peer_unchoked(addr1), vec![Block::new(0, 0, 8)]);
        assert_eq!(scheduler.peer_unchoked(addr2), vec![Block::new(0, 8, 8)]);
    }

    #[test]
    fn select_rarest_pieces_first() {
        let mut scheduler = test_scheduler(&[]);

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        assert_eq!(
            scheduler.peer_has_piece(addr1, 0),
            PeerPieceResponse::ExpressInterest
        );

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        assert_eq!(
            scheduler.peer_has_piece(addr2, 0),
            PeerPieceResponse::ExpressInterest
        );
        assert_eq!(
            scheduler.peer_has_piece(addr2, 1),
            PeerPieceResponse::NoAction
        );

        // Peer 2 has both piece #0 and #1. Since piece #1 is rarer, select it first.
        assert_eq!(scheduler.peer_unchoked(addr2), vec![Block::new(1, 0, 8)]);

        // Peer 1 only has piece #0, select it.
        assert_eq!(scheduler.peer_unchoked(addr1), vec![Block::new(0, 0, 8)]);
    }

    #[test]
    fn prioritize_pieces_that_already_started_downloading() {
        let mut scheduler = test_scheduler(&[]);

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        assert_eq!(
            scheduler.peer_has_piece(addr1, 0),
            PeerPieceResponse::ExpressInterest
        );

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        assert_eq!(
            scheduler.peer_has_piece(addr2, 0),
            PeerPieceResponse::ExpressInterest
        );
        assert_eq!(
            scheduler.peer_has_piece(addr2, 1),
            PeerPieceResponse::NoAction
        );

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

        assert_eq!(
            scheduler.peer_has_piece(addr, 0),
            PeerPieceResponse::ExpressInterest
        );
        assert_eq!(scheduler.peer_unchoked(addr), vec![block1]);

        // Continue with next block once previous one completes
        assert_eq!(scheduler.block_downloaded(&addr, &block1), vec![block2]);
    }

    #[test]
    fn release_abandoned_block() {
        let mut scheduler = test_scheduler(&[]);
        let addr = "127.0.0.1:6881".parse().unwrap();
        let block = Block::new(0, 0, 8);

        assert_eq!(
            scheduler.peer_has_piece(addr, 0),
            PeerPieceResponse::ExpressInterest
        );
        assert_eq!(scheduler.peer_unchoked(addr), vec![block]);

        // Block abandoned, mark it as unassigned and re-assigns the same abandoned block
        assert_eq!(scheduler.release(&addr, block), vec![block]);
    }

    #[test]
    fn release_block_after_peer_disconnected() {
        let mut scheduler = test_scheduler(&[]);
        let block = Block::new(0, 0, 8);

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        assert_eq!(
            scheduler.peer_has_piece(addr1, 0),
            PeerPieceResponse::ExpressInterest
        );
        assert_eq!(scheduler.peer_unchoked(addr1), vec![block]);

        scheduler.peer_disconnected(&addr1);
        assert!(scheduler.release(&addr1, block).is_empty());

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        assert_eq!(
            scheduler.peer_has_piece(addr2, 0),
            PeerPieceResponse::ExpressInterest
        );
        assert_eq!(scheduler.peer_unchoked(addr2), vec![block]);
    }

    #[test]
    fn assign_abandoned_block_to_other_peer_if_needed() {
        let mut scheduler = test_scheduler(&[]);

        let block = Block::new(0, 0, 8);
        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let addr2 = "127.0.0.2:6881".parse().unwrap();

        assert_eq!(
            scheduler.peer_has_piece(addr1, 0),
            PeerPieceResponse::ExpressInterest
        );
        assert_eq!(scheduler.peer_unchoked(addr1), vec![block]);

        // Peer 1 choked before completing the block, assign to peer 2
        scheduler.peer_choked(addr1);

        assert_eq!(
            scheduler.peer_has_piece(addr2, 0),
            PeerPieceResponse::ExpressInterest
        );
        assert_eq!(scheduler.peer_unchoked(addr2), vec![block]);
    }

    #[test]
    fn do_not_reassign_downloaded_block() {
        let mut scheduler = test_scheduler(&[]);

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let addr2 = "127.0.0.2:6881".parse().unwrap();
        let block1 = Block::new(0, 0, 8);
        let block2 = Block::new(0, 8, 8);

        assert_eq!(
            scheduler.peer_has_piece(addr1, 0),
            PeerPieceResponse::ExpressInterest
        );
        assert_eq!(scheduler.peer_unchoked(addr1), vec![block1]);

        // Peer 1 completed downloading block #1 and choked. Assign next block to peer 2
        assert_eq!(scheduler.block_downloaded(&addr1, &block1), vec![block2]);
        scheduler.peer_choked(addr1);

        assert_eq!(
            scheduler.peer_has_piece(addr2, 0),
            PeerPieceResponse::ExpressInterest
        );
        assert_eq!(scheduler.peer_unchoked(addr2), vec![block2]);
    }

    #[test]
    fn find_all_peers_that_are_no_longer_interesting() {
        let config = test_config("/tmp").with_block_size(Size::from_kib(16));
        let mut scheduler = test_scheduler_with_config(config, &[]);

        // Peer 1 has pieces #0 and #1
        let addr1 = "127.0.0.1:6881".parse().unwrap();
        assert_eq!(
            scheduler.peer_has_piece(addr1, 0),
            PeerPieceResponse::ExpressInterest
        );
        assert_eq!(
            scheduler.peer_has_piece(addr1, 1),
            PeerPieceResponse::NoAction
        );

        // Peer 2 has only piece #0
        let addr2 = "127.0.0.2:6881".parse().unwrap();
        assert_eq!(
            scheduler.peer_has_piece(addr2, 0),
            PeerPieceResponse::ExpressInterest
        );

        // Peer #1 downloads all blocks
        for block in scheduler.peer_unchoked(addr1) {
            scheduler.block_downloaded(&addr1, &block);
        }

        // Once piece #0 is downloaded, we're no longer interested in peer 2
        let not_interesting = scheduler.client_has_piece(0);
        assert_eq!(not_interesting.len(), 1);
        assert!(not_interesting.contains(&addr2));
    }

    fn test_scheduler_with_config(config: Config, has_pieces: &[usize]) -> Scheduler {
        let torrent = test_torrent();
        let download = Arc::new(Download { torrent, config });
        let has_pieces = BitSet::from_iter(has_pieces.iter().copied());
        Scheduler::new(download, &has_pieces)
    }

    #[test]
    fn peer_disconnection_releases_blocks() {
        let mut scheduler = test_scheduler(&[]);
        let addr = "127.0.0.1:6881".parse().unwrap();

        // Peer gets a block assigned
        assert_eq!(
            scheduler.peer_has_piece(addr, 0),
            PeerPieceResponse::ExpressInterest
        );
        let blocks = scheduler.peer_unchoked(addr);
        assert_eq!(blocks.len(), 1);

        // When peer disconnects, block should be available again
        scheduler.peer_disconnected(&addr);

        // New peer should get the same block
        let addr2 = "127.0.0.2:6881".parse().unwrap();
        assert_eq!(
            scheduler.peer_has_piece(addr2, 0),
            PeerPieceResponse::ExpressInterest
        );
        assert_eq!(scheduler.peer_unchoked(addr2), blocks);
    }

    #[test]
    fn multiple_peers_get_different_blocks() {
        let config = test_config("/tmp")
            .with_block_size(Size::from_bytes(8))
            .with_max_concurrent_requests_per_peer(2);
        let mut scheduler = test_scheduler_with_config(config, &[]);

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let addr2 = "127.0.0.2:6881".parse().unwrap();

        assert_eq!(
            scheduler.peer_has_piece(addr1, 0),
            PeerPieceResponse::ExpressInterest
        );
        let blocks1 = scheduler.peer_unchoked(addr1);
        assert_eq!(blocks1.len(), 2);

        assert_eq!(
            scheduler.peer_has_piece(addr2, 0),
            PeerPieceResponse::ExpressInterest
        );
        let blocks2 = scheduler.peer_unchoked(addr2);
        assert_eq!(blocks2.len(), 2);

        // Verify blocks are different
        for b1 in &blocks1 {
            assert!(!blocks2.contains(b1));
        }
    }

    fn test_scheduler(has_pieces: &[usize]) -> Scheduler {
        let config = test_config("/tmp")
            .with_block_size(Size::from_bytes(8))
            .with_max_concurrent_requests_per_peer(1);
        test_scheduler_with_config(config, has_pieces)
    }
}
