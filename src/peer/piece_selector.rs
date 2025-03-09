use std::cmp::Reverse;
use std::collections::HashSet;
use std::{collections::HashMap, net::SocketAddr};

use bit_set::BitSet;
use priority_queue::PriorityQueue;
use size::Size;
use tokio::sync::Notify;

use crate::peer::message::Block;

#[allow(dead_code)]
pub struct PieceSelector {
    piece_to_peers: HashMap<usize, HashSet<SocketAddr>>,
    rarest_piece: PriorityQueue<usize, Reverse<usize>>,
    current_piece: Option<usize>,
    total_pieces: usize,
    block_size: Size,
    completed_pieces: BitSet,
    notify: Notify,
}

#[allow(dead_code)]
impl PieceSelector {
    fn new(total_pieces: usize, block_size: Size) -> Self {
        Self {
            piece_to_peers: HashMap::new(),
            rarest_piece: PriorityQueue::new(),
            current_piece: None,
            total_pieces,
            block_size,
            completed_pieces: BitSet::new(),
            notify: Notify::new(),
        }
    }

    async fn next(&mut self) -> Option<(&SocketAddr, Block)> {
        // get rarest pieces that i don't have
        // sort peers by transfer rate
        // assign blocks to available peers
        // mark blocks as "in-flight"
        // give up on blocks after configured duration
        while self.rarest_piece.is_empty() && self.completed_pieces.len() < self.total_pieces {
            self.notify.notified().await;
        }
        while let Some((piece, _)) = self.rarest_piece.pop() {
            if self.completed_pieces.contains(piece) {
                continue;
            }
            let peer = self
                .piece_to_peers
                .get(&piece)
                .and_then(|peers| peers.iter().next())
                .expect("at least 1 peer should have the piece");
            return Some((peer, Block::new(piece, 0, self.block_size.bytes() as usize)));
        }
        None
    }

    fn piece_complete(&mut self, piece: usize) {
        self.completed_pieces.insert(piece);
        self.notify.notify_one();
    }

    fn peer_has_pieces(&mut self, addr: SocketAddr, pieces: BitSet) {
        for piece in &pieces {
            let peers = self.piece_to_peers.entry(piece).or_default();
            peers.insert(addr);
            self.rarest_piece.push(piece, Reverse(peers.len()));
        }
        self.notify.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use size::KiB;

    const BLOCK_SIZE: Size = Size::from_const(KiB);
    const BLOCK_SIZE_BYTES: usize = BLOCK_SIZE.bytes() as usize;

    #[tokio::test]
    async fn one_available_peer() {
        let addr = "127.0.0.1:6881".parse().unwrap();
        let mut state = PieceSelector::new(1, BLOCK_SIZE);
        state.peer_has_pieces(addr, BitSet::from_bytes(&[0b10000000]));

        assert_eq!(
            state.next().await,
            Some((&addr, Block::new(0, 0, BLOCK_SIZE_BYTES)))
        );
    }

    #[tokio::test]
    async fn one_available_peer_with_different_piece() {
        let addr = "127.0.0.1:6881".parse().unwrap();
        let mut state = PieceSelector::new(1, BLOCK_SIZE);
        state.peer_has_pieces(addr, BitSet::from_bytes(&[0b01000000]));

        assert_eq!(
            state.next().await,
            Some((&addr, Block::new(1, 0, BLOCK_SIZE_BYTES)))
        );
    }

    #[tokio::test]
    async fn ignore_pieces_clinet_already_has() {
        let addr = "127.0.0.1:6881".parse().unwrap();
        let mut state = PieceSelector::new(2, BLOCK_SIZE);
        state.piece_complete(0);
        state.peer_has_pieces(addr, BitSet::from_bytes(&[0b11000000]));

        assert_eq!(
            state.next().await,
            Some((&addr, Block::new(1, 0, BLOCK_SIZE_BYTES)))
        );
    }

    // TODO: blocks

    #[tokio::test]
    async fn get_rarest_pieces_first() {
        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let addr2 = "127.0.0.2:6881".parse().unwrap();

        let mut state = PieceSelector::new(2, BLOCK_SIZE);
        // Piece #0 is rarest - only peer 2 has it
        state.peer_has_pieces(addr1, BitSet::from_bytes(&[0b10000000]));
        state.peer_has_pieces(addr2, BitSet::from_bytes(&[0b11000000]));

        assert_eq!(
            state.next().await,
            Some((&addr2, Block::new(1, 0, BLOCK_SIZE_BYTES)))
        );
    }
}
