use std::{
    collections::{BTreeSet, HashMap, HashSet},
    net::SocketAddr,
};

use crate::peer::scheduler::piece_state::PieceState;

pub struct AvailablePieces {
    pieces: HashMap<usize, AvailablePiece>,
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

    pub fn next(&mut self, addr: &SocketAddr) -> Option<AvailablePiece> {
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

    fn priority(&self) -> (usize, usize) {
        (self.peers_with_piece.len(), self.index)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn foo() {
        todo!()
    }
}
