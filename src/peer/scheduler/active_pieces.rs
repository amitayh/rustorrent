use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
};

use crate::peer::scheduler::available_pieces::AvailablePiece;
use crate::peer::scheduler::piece_state::PieceState;
use crate::{message::Block, peer::blocks::Blocks};

pub struct ActivePieces(HashMap<usize, ActivePiece>);

impl ActivePieces {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    pub fn insert(&mut self, index: usize, piece: ActivePiece) {
        self.0.insert(index, piece);
    }

    pub fn get_mut_safe(&mut self, piece: usize) -> Option<&mut ActivePiece> {
        self.0.get_mut(&piece)
    }

    pub fn get_mut(&mut self, piece: usize) -> &mut ActivePiece {
        self.get_mut_safe(piece).expect("invalid piece")
    }

    pub fn peer_has_piece(&mut self, piece: usize, addr: SocketAddr) {
        self.get_mut(piece).peers_with_piece.insert(addr);
    }

    pub fn peer_disconnected(&mut self, index: usize, addr: &SocketAddr) -> PieceState {
        let piece = self.get_mut(index);
        assert!(
            piece.peers_with_piece.remove(addr),
            "peer should have piece"
        );
        if piece.peers_with_piece.is_empty() {
            PieceState::Orphan
        } else {
            PieceState::Active
        }
    }

    pub fn contains(&self, piece: usize) -> bool {
        self.0.contains_key(&piece)
    }

    pub fn remove(&mut self, piece: usize) -> ActivePiece {
        self.0.remove(&piece).expect("invalid piece")
    }

    pub fn try_assign_n(&mut self, addr: &SocketAddr, n: usize, blocks: &mut Vec<Block>) -> usize {
        let mut assigned = 0;
        for piece in self.peer_pieces(addr) {
            assigned += piece.try_assign_n(n - assigned, blocks);
            if assigned == n {
                break;
            }
        }
        assigned
    }

    fn peer_pieces(&mut self, addr: &SocketAddr) -> impl Iterator<Item = &mut ActivePiece> {
        self.0
            .values_mut()
            .filter(|piece| piece.peers_with_piece.contains(addr))
    }
}

#[derive(Debug)]
pub struct ActivePiece {
    pub index: usize,
    pub peers_with_piece: HashSet<SocketAddr>,
    unassigned_blocks: Blocks,
    released_blocks: Vec<Block>,
}

impl ActivePiece {
    pub fn new(piece: AvailablePiece, blocks: Blocks) -> Self {
        Self {
            index: piece.index,
            unassigned_blocks: blocks,
            released_blocks: Vec::new(),
            peers_with_piece: piece.peers_with_piece,
        }
    }

    pub fn peers(&self) -> impl Iterator<Item = &SocketAddr> {
        self.peers_with_piece.iter()
    }

    pub fn unassign(&mut self, block: Block) {
        assert_eq!(block.piece, self.index, "piece mismatch");
        self.released_blocks.push(block);
    }

    pub fn try_assign_n(&mut self, n: usize, dest: &mut Vec<Block>) -> usize {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_pieces_to_assign_from() {
        let mut pieces = ActivePieces::new();
        let addr = "127.0.0.1:6881".parse().unwrap();
        let mut blocks = Vec::new();

        let assigned = pieces.try_assign_n(&addr, 1, &mut blocks);

        assert_eq!(assigned, 0);
        assert!(blocks.is_empty());
    }

    #[test]
    fn assign_one_piece() {
        let mut pieces = ActivePieces::new();
        let addr = "127.0.0.1:6881".parse().unwrap();
        let mut blocks = Vec::new();

        let piece = AvailablePiece::new(0, addr);
        let piece_blocks = Blocks::new(0, 8, 8);
        pieces.insert(0, ActivePiece::new(piece, piece_blocks));

        let assigned = pieces.try_assign_n(&addr, 1, &mut blocks);

        assert_eq!(assigned, 1);
        assert_eq!(blocks, vec![Block::new(0, 0, 8)]);
    }

    #[test]
    fn assign_multiple_pieces() {
        let mut pieces = ActivePieces::new();
        let addr = "127.0.0.1:6881".parse().unwrap();
        let mut blocks = Vec::new();

        let piece = AvailablePiece::new(0, addr);
        let piece_blocks = Blocks::new(0, 16, 8);
        pieces.insert(0, ActivePiece::new(piece, piece_blocks));

        let assigned = pieces.try_assign_n(&addr, 2, &mut blocks);

        assert_eq!(assigned, 2);
        assert_eq!(blocks, vec![Block::new(0, 0, 8), Block::new(0, 8, 8)]);
    }

    #[test]
    fn assign_less_blocks_than_requested() {
        let mut pieces = ActivePieces::new();
        let addr = "127.0.0.1:6881".parse().unwrap();
        let mut blocks = Vec::new();

        let piece = AvailablePiece::new(0, addr);
        let piece_blocks = Blocks::new(0, 16, 8);
        pieces.insert(0, ActivePiece::new(piece, piece_blocks));

        let assigned = pieces.try_assign_n(&addr, 3, &mut blocks);

        assert_eq!(assigned, 2);
        assert_eq!(blocks, vec![Block::new(0, 0, 8), Block::new(0, 8, 8)]);
    }

    #[test]
    fn assign_blocks_from_multiple_pieces_if_needed() {
        let mut pieces = ActivePieces::new();
        let addr = "127.0.0.1:6881".parse().unwrap();
        let mut blocks = Vec::new();

        let piece0 = AvailablePiece::new(0, addr);
        let piece0_blocks = Blocks::new(0, 16, 8);
        pieces.insert(0, ActivePiece::new(piece0, piece0_blocks));

        let piece1 = AvailablePiece::new(1, addr);
        let piece1_blocks = Blocks::new(1, 16, 8);
        pieces.insert(1, ActivePiece::new(piece1, piece1_blocks));

        let assigned = pieces.try_assign_n(&addr, 3, &mut blocks);

        assert_eq!(assigned, 3);
    }

    #[test]
    fn assign_released_blocks_first() {
        let addr = "127.0.0.1:6881".parse().unwrap();

        let available_piece = AvailablePiece::new(0, addr);
        let piece_blocks = Blocks::new(0, 16, 8);
        let mut piece = ActivePiece::new(available_piece, piece_blocks);

        let mut blocks = Vec::new();
        assert_eq!(piece.try_assign_n(1, &mut blocks), 1);
        assert_eq!(blocks, vec![Block::new(0, 0, 8)]);

        piece.unassign(Block::new(0, 0, 8));

        let mut blocks = Vec::new();
        assert_eq!(piece.try_assign_n(1, &mut blocks), 1);
        assert_eq!(blocks, vec![Block::new(0, 0, 8)]);

        let mut blocks = Vec::new();
        assert_eq!(piece.try_assign_n(1, &mut blocks), 1);
        assert_eq!(blocks, vec![Block::new(0, 8, 8)]);
    }

    #[test]
    fn ignore_pieces_peer_does_not_have() {
        let mut pieces = ActivePieces::new();
        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let addr2 = "127.0.0.2:6881".parse().unwrap();
        let mut blocks = Vec::new();

        let piece = AvailablePiece::new(0, addr1);
        let piece_blocks = Blocks::new(0, 16, 8);
        pieces.insert(0, ActivePiece::new(piece, piece_blocks));

        let assigned = pieces.try_assign_n(&addr2, 1, &mut blocks);

        assert_eq!(assigned, 0);
        assert!(blocks.is_empty());
    }
}
