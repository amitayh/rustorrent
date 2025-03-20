use std::cmp::Reverse;
use std::collections::{HashSet, VecDeque};
use std::{collections::HashMap, net::SocketAddr};

use bit_set::BitSet;
use size::Size;

use crate::peer::{blocks::Blocks, message::Block};

pub struct Assignment {
    has_pieces: BitSet,
    peers: HashMap<SocketAddr, PeerState>,
    pieces: Vec<PieceState>,
}

impl Assignment {
    pub fn new(piece_size: Size, total_size: Size, block_size: Size) -> Self {
        let total_pieces =
            ((total_size.bytes() as f64) / (piece_size.bytes() as f64)).ceil() as usize;

        let mut pieces = Vec::with_capacity(total_pieces);
        for piece in 0..total_pieces {
            let blocks = Blocks::new(piece_size, total_size, block_size, piece);
            pieces.push(PieceState::new(blocks));
        }

        Self {
            has_pieces: BitSet::with_capacity(total_pieces),
            peers: HashMap::new(),
            pieces,
        }
    }

    pub fn client_has_pieces(&mut self, pieces: &BitSet) {
        self.has_pieces.union_with(pieces);
    }

    pub fn peer_has_pieces(&mut self, addr: SocketAddr, pieces: &BitSet) -> Option<Block> {
        let peer = self.peers.entry(addr).or_default();
        peer.has_pieces.union_with(pieces);
        for piece in pieces {
            let piece = self.pieces.get_mut(piece).expect("invalid piece");
            piece.peer_has_piece();
        }
        self.assign(&addr)
    }

    pub fn peer_disconnected(&mut self, addr: &SocketAddr) {
        if let Some(peer) = self.peers.remove(addr) {
            for block in peer.assigned_blocks {
                let piece = self.pieces.get_mut(block.piece).expect("invalid piece");
                piece.release(block);
            }
        }
    }

    pub fn peer_unchoked(&mut self, addr: SocketAddr) -> Option<Block> {
        let peer = self.peers.entry(addr).or_default();
        peer.choking = false;
        self.assign(&addr)
    }

    // TODO: make private
    pub fn assign(&mut self, addr: &SocketAddr) -> Option<Block> {
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

    pub fn peer_choked(&mut self, addr: &SocketAddr) {
        if let Some(peer) = self.peers.remove(addr) {
            for block in peer.assigned_blocks {
                let piece = self.pieces.get_mut(block.piece).expect("invalid piece");
                piece.release(block);
            }
        }
    }

    pub fn block_downloaded(&mut self, addr: &SocketAddr, block: &Block) {
        let peer = self.peers.get_mut(addr).expect("invalid peer");
        peer.assigned_blocks.remove(&block);

        let piece = self.pieces.get_mut(block.piece).expect("invalid piece");
        piece.block_downloaded();
    }

    pub fn release(&mut self, addr: &SocketAddr, block: Block) {
        let peer = self.peers.get_mut(addr).expect("invalid peer");
        let piece = self.pieces.get_mut(block.piece).expect("invalid piece");
        peer.assigned_blocks.remove(&block);
        piece.release(block);
    }
}

#[derive(Debug)]
struct PeerState {
    choking: bool,
    has_pieces: BitSet,
    assigned_blocks: HashSet<Block>,
}

impl Default for PeerState {
    fn default() -> Self {
        Self {
            choking: true,
            // TODO: add capacity
            has_pieces: BitSet::new(),
            assigned_blocks: HashSet::new(),
        }
    }
}

#[derive(Debug)]
struct PieceState {
    unassigned_blocks: VecDeque<Block>,
    assigned_blocks: usize,
    peers_with_piece: usize,
}

impl PieceState {
    fn new(blocks: Blocks) -> Self {
        Self {
            unassigned_blocks: blocks.collect(),
            assigned_blocks: 0,
            peers_with_piece: 0,
        }
    }

    fn peer_has_piece(&mut self) {
        self.peers_with_piece += 1;
    }

    fn select_block(&mut self) -> Option<Block> {
        let block = self.unassigned_blocks.pop_front()?;
        self.assigned_blocks += 1;
        Some(block)
    }

    fn release(&mut self, block: Block) {
        // TODO: assert block belongs to piece?
        self.unassigned_blocks.push_front(block);
        self.assigned_blocks -= 1;
    }

    fn priority(&self) -> (bool, Reverse<usize>) {
        (
            // Favor pieces which are already in flight in order to reduce fragmentation
            self.assigned_blocks > 0,
            // Otherwise, favor rarest piece first
            Reverse(self.peers_with_piece),
        )
    }

    // TODO: test
    fn is_eligible(&self) -> bool {
        !self.unassigned_blocks.is_empty()
    }

    fn block_downloaded(&mut self) {
        self.assigned_blocks -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIECE_SIZE: Size = Size::from_const(24);
    const TOTAL_SIZE: Size = Size::from_const(32);
    const BLOCK_SIZE: Size = Size::from_const(8);

    #[test]
    fn peer_unchoked_but_has_no_pieces() {
        let mut assignment = Assignment::new(PIECE_SIZE, TOTAL_SIZE, BLOCK_SIZE);
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert!(assignment.peer_unchoked(addr).is_none());
    }

    #[test]
    fn peer_unchoked_but_client_already_has_that_piece() {
        let mut assignment = Assignment::new(PIECE_SIZE, TOTAL_SIZE, BLOCK_SIZE);
        let addr = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_bytes(&[0b10000000]);
        assignment.client_has_pieces(&pieces);

        assert!(assignment.peer_has_pieces(addr, &pieces).is_none());
        assert!(assignment.peer_unchoked(addr).is_none());
    }

    #[test]
    fn assign_a_block_to_request_from_peer() {
        let mut assignment = Assignment::new(PIECE_SIZE, TOTAL_SIZE, BLOCK_SIZE);
        let addr = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_bytes(&[0b10000000]);

        assert!(assignment.peer_has_pieces(addr, &pieces).is_none());
        assert_eq!(assignment.peer_unchoked(addr), Some(Block::new(0, 0, 8)));
    }

    #[test]
    fn peer_unchoked_before_notifying_on_completed_piece() {
        let mut assignment = Assignment::new(PIECE_SIZE, TOTAL_SIZE, BLOCK_SIZE);
        let addr = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_bytes(&[0b10000000]);

        assert!(assignment.peer_unchoked(addr).is_none());
        assert_eq!(
            assignment.peer_has_pieces(addr, &pieces),
            Some(Block::new(0, 0, 8))
        );
    }
}
