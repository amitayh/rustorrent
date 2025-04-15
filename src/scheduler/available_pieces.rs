use std::{
    collections::{BTreeSet, HashMap, HashSet},
    net::SocketAddr,
};

use crate::scheduler::active_pieces::ActivePiece;
use crate::scheduler::piece_state::PieceState;

/// Tracks pieces that are available from peers but not yet selected for download.
///
/// This struct maintains two key pieces of information:
/// - A mapping of piece indices to their availability state (`AvailablePiece`)
/// - A priority queue of pieces ordered by rarity (number of peers that have the piece)
///
/// The priority queue is used to implement the rarest-first piece selection strategy,
/// where pieces that are available from fewer peers are prioritized for download.
pub struct AvailablePieces {
    /// Maps piece indices to their availability state
    pieces: HashMap<usize, AvailablePiece>,
    /// Priority queue of (peer_count, piece_index) tuples ordered by rarity
    priorities: BTreeSet<(usize, usize)>,
}

impl AvailablePieces {
    pub fn new() -> Self {
        Self {
            pieces: HashMap::new(),
            priorities: BTreeSet::new(),
        }
    }

    pub fn insert(&mut self, piece: AvailablePiece) {
        self.priorities.insert(piece.priority());
        self.pieces.insert(piece.index, piece);
    }

    pub fn contains(&self, piece: usize) -> bool {
        self.pieces.contains_key(&piece)
    }

    pub fn peer_has_piece(&mut self, index: usize, addr: SocketAddr) {
        let piece = self.pieces.get_mut(&index).expect("invalid piece");
        assert!(
            self.priorities.remove(&piece.priority()),
            "piece priority should be present"
        );
        piece.peers_with_piece.insert(addr);
        self.priorities.insert(piece.priority());
    }

    pub fn peer_disconnected(&mut self, index: usize, addr: &SocketAddr) -> PieceState {
        let piece = self.pieces.get_mut(&index).expect("invalid piece");
        assert!(
            self.priorities.remove(&piece.priority()),
            "piece priority should be present"
        );
        assert!(
            piece.peers_with_piece.remove(addr),
            "peer should have piece"
        );
        if piece.peers_with_piece.is_empty() {
            // No more peers have this piece, it should no longer be considered "available"
            self.pieces.remove(&index).expect("piece should be present");
            PieceState::Orphan
        } else {
            // Update priority after peer-set change
            self.priorities.insert(piece.priority());
            PieceState::Available
        }
    }

    pub fn take_next(&mut self, addr: &SocketAddr) -> Option<AvailablePiece> {
        let piece = self
            .priorities
            .iter()
            .filter_map(|(_, index)| {
                let piece = self.pieces.get(index).expect("invalid piece");
                if piece.peers_with_piece.contains(addr) {
                    Some(*index)
                } else {
                    None
                }
            })
            .next()?;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailablePiece {
    pub index: usize,
    pub peers_with_piece: HashSet<SocketAddr>,
}

impl AvailablePiece {
    pub fn new(index: usize, addr: SocketAddr) -> Self {
        Self {
            index,
            peers_with_piece: HashSet::from_iter([addr]),
        }
    }

    pub fn from(piece: ActivePiece) -> Self {
        Self {
            index: piece.index,
            peers_with_piece: piece.peers_with_piece,
        }
    }

    fn priority(&self) -> (usize, usize) {
        (self.peers_with_piece.len(), self.index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_available_pieces() {
        let mut available_pieces = AvailablePieces::new();
        let addr = "127.0.0.1:6881".parse().unwrap();

        assert_eq!(available_pieces.take_next(&addr), None);
    }

    #[test]
    fn no_available_pieces_for_peer() {
        let mut available_pieces = AvailablePieces::new();
        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let addr2 = "127.0.0.2:6881".parse().unwrap();

        available_pieces.insert(AvailablePiece::new(0, addr1));

        assert_eq!(available_pieces.take_next(&addr2), None);
    }

    #[test]
    fn take_next_piece_for_peer() {
        let mut available_pieces = AvailablePieces::new();
        let addr = "127.0.0.1:6881".parse().unwrap();

        let piece = AvailablePiece::new(0, addr);
        available_pieces.insert(piece.clone());

        assert_eq!(available_pieces.take_next(&addr), Some(piece));
        assert_eq!(available_pieces.take_next(&addr), None);
    }

    #[test]
    fn select_rarest_pieces_first() {
        let mut available_pieces = AvailablePieces::new();
        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let addr2 = "127.0.0.2:6881".parse().unwrap();

        // Peer #1 has both pieces 0 and 1
        let piece0 = AvailablePiece::new(0, addr1);
        available_pieces.insert(piece0.clone());

        let piece1 = AvailablePiece::new(1, addr1);
        available_pieces.insert(piece1.clone());

        // Peer #2 has piece 0
        available_pieces.peer_has_piece(0, addr2);

        // Piece 1 is rarest, select it first
        assert_eq!(available_pieces.take_next(&addr1).unwrap().index, 1);
        assert_eq!(available_pieces.take_next(&addr1).unwrap().index, 0);
        assert_eq!(available_pieces.take_next(&addr1), None);
        assert_eq!(available_pieces.take_next(&addr2), None);
    }

    #[test]
    fn peer_disconnection_updates_priority() {
        let mut available_pieces = AvailablePieces::new();
        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let addr2 = "127.0.0.2:6881".parse().unwrap();
        let addr3 = "127.0.0.3:6881".parse().unwrap();

        // Peer #1 has pieces 0, 1 and 2
        let piece0 = AvailablePiece::new(0, addr1);
        available_pieces.insert(piece0.clone());

        let piece1 = AvailablePiece::new(1, addr1);
        available_pieces.insert(piece1.clone());

        let piece2 = AvailablePiece::new(2, addr1);
        available_pieces.insert(piece2.clone());

        // Peer #2 has pieces 0 and 1
        available_pieces.peer_has_piece(0, addr2);
        available_pieces.peer_has_piece(1, addr2);

        // Peer #3 has piece 1
        available_pieces.peer_has_piece(1, addr3);

        // Peer #1 disconnected
        available_pieces.peer_disconnected(0, &addr1);
        available_pieces.peer_disconnected(1, &addr1);
        available_pieces.peer_disconnected(2, &addr1);

        // Now piece 1 is rarest
        assert_eq!(available_pieces.take_next(&addr2).unwrap().index, 0);
        assert_eq!(available_pieces.take_next(&addr2).unwrap().index, 1);
        assert_eq!(available_pieces.take_next(&addr2), None);
        assert_eq!(available_pieces.take_next(&addr3), None);
    }

    #[test]
    fn peer_without_pieces_returns_none() {
        let mut available_pieces = AvailablePieces::new();
        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let addr2 = "127.0.0.2:6881".parse().unwrap();

        let piece = AvailablePiece::new(0, addr1);
        available_pieces.insert(piece);

        assert_eq!(available_pieces.take_next(&addr2), None);
    }

    #[test]
    fn last_peer_disconnection_makes_piece_orphan() {
        let mut available_pieces = AvailablePieces::new();
        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let addr2 = "127.0.0.2:6881".parse().unwrap();

        let piece = AvailablePiece::new(0, addr1);
        available_pieces.insert(piece);
        available_pieces.peer_has_piece(0, addr2);

        assert_eq!(
            available_pieces.peer_disconnected(0, &addr1),
            PieceState::Available
        );
        assert_eq!(
            available_pieces.peer_disconnected(0, &addr2),
            PieceState::Orphan
        );
        assert!(!available_pieces.contains(0));
    }
}
