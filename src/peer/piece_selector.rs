use std::cmp::{Ordering, Reverse};
use std::collections::HashSet;
use std::iter::{Cycle, Zip};
use std::time::Duration;
use std::vec::IntoIter;
use std::{collections::HashMap, net::SocketAddr};

use bit_set::BitSet;
use priority_queue::PriorityQueue;
use size::Size;
use tokio::sync::Notify;

use crate::peer::blocks::Blocks;
use crate::peer::message::Block;
use crate::peer::transfer_rate::TransferRate;

#[allow(dead_code)]
pub struct PieceSelector {
    peer_transfer_rate: HashMap<SocketAddr, TransferRate>,
    rarest_piece: PriorityQueue<usize, PeerSet>,
    current_piece: Option<Zip<Cycle<IntoIter<SocketAddr>>, Blocks>>,
    piece_size: Size,
    total_size: Size,
    block_size: Size,
    completed_pieces: BitSet,
    notify: Notify,
}

#[allow(dead_code)]
impl PieceSelector {
    pub fn new(piece_size: Size, total_size: Size, block_size: Size) -> Self {
        Self {
            peer_transfer_rate: HashMap::new(),
            rarest_piece: PriorityQueue::new(),
            current_piece: None,
            piece_size,
            total_size,
            block_size,
            completed_pieces: BitSet::new(),
            notify: Notify::new(),
        }
    }

    pub async fn next(&mut self) -> Option<(SocketAddr, Block)> {
        if let Some(blocks) = &mut self.current_piece {
            if let result @ Some(_) = blocks.next() {
                return result;
            } else {
                self.current_piece = None;
            }
        }
        // mark blocks as "in-flight"
        // give up on blocks after configured duration
        let total_pieces =
            ((self.total_size.bytes() as f64) / (self.piece_size.bytes() as f64)).ceil() as usize;
        while self.rarest_piece.is_empty() && self.completed_pieces.len() < total_pieces {
            self.notify.notified().await;
        }

        if let Some((piece, PeerSet(peers))) = self.rarest_piece.pop() {
            let mut peers: Vec<_> = peers.into_iter().collect();
            peers.sort_by_key(|peer| Reverse(self.transfer_rate_of(peer)));
            let peers_cycle = peers.into_iter().cycle();
            let blocks = Blocks::new(self.piece_size, self.total_size, self.block_size, piece);
            let mut zipped = peers_cycle.zip(blocks);
            if let Some((addr, block)) = zipped.next() {
                self.current_piece = Some(zipped);
                return Some((addr, block));
            }
        }
        None
    }

    fn transfer_rate_of(&self, peer: &SocketAddr) -> &TransferRate {
        self.peer_transfer_rate
            .get(peer)
            .unwrap_or(&TransferRate::EMPTY)
    }

    fn piece_complete(&mut self, piece: usize) {
        self.completed_pieces.insert(piece);
        self.notify.notify_one();
    }

    // TODO: test
    fn peer_disconnected(&mut self, addr: &SocketAddr) {
        for (_, PeerSet(peers)) in self.rarest_piece.iter_mut() {
            peers.remove(addr);
        }
    }

    fn peer_has_pieces(&mut self, addr: SocketAddr, pieces: BitSet) {
        for piece in &pieces {
            if self.completed_pieces.contains(piece) {
                // Ignore pieces we already have
                continue;
            }

            let piece_exists = self
                .rarest_piece
                .change_priority_by(&piece, |PeerSet(peers)| {
                    peers.insert(addr);
                });

            if !piece_exists {
                self.rarest_piece.push(piece, PeerSet::singleton(addr));
            }
        }
        self.notify.notify_one();
    }

    fn update_transfer_rate(&mut self, addr: SocketAddr, size: Size, duration: Duration) {
        let entry = self
            .peer_transfer_rate
            .entry(addr)
            .or_insert(TransferRate::EMPTY);

        *entry += TransferRate(size, duration);
    }
}

#[derive(PartialEq, Eq)]
struct PeerSet(HashSet<SocketAddr>);

impl PeerSet {
    fn singleton(addr: SocketAddr) -> Self {
        Self(HashSet::from([addr]))
    }
}

impl PartialOrd for PeerSet {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PeerSet {
    fn cmp(&self, other: &Self) -> Ordering {
        // Compare based on peer set size in reverse ordering
        other.0.len().cmp(&self.0.len())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    use size::KiB;

    const PIECE_SIZE: Size = Size::from_const(8 * KiB);
    const TOTAL_SIZE: Size = Size::from_const(24 * KiB);
    const BLOCK_SIZE: Size = Size::from_const(KiB);
    const BLOCK_SIZE_BYTES: usize = BLOCK_SIZE.bytes() as usize;

    #[tokio::test]
    async fn one_available_peer() {
        let addr = "127.0.0.1:6881".parse().unwrap();
        let mut state = PieceSelector::new(PIECE_SIZE, TOTAL_SIZE, BLOCK_SIZE);
        state.peer_has_pieces(addr, BitSet::from_bytes(&[0b10000000]));

        assert_eq!(
            state.next().await,
            Some((addr, Block::new(0, 0, BLOCK_SIZE_BYTES)))
        );
    }

    #[tokio::test]
    async fn one_available_peer_with_different_piece() {
        let addr = "127.0.0.1:6881".parse().unwrap();
        let mut state = PieceSelector::new(PIECE_SIZE, TOTAL_SIZE, BLOCK_SIZE);
        state.peer_has_pieces(addr, BitSet::from_bytes(&[0b01000000]));

        assert_eq!(
            state.next().await,
            Some((addr, Block::new(1, 0, BLOCK_SIZE_BYTES)))
        );
    }

    #[tokio::test]
    async fn ignore_pieces_clinet_already_has() {
        let addr = "127.0.0.1:6881".parse().unwrap();
        let mut state = PieceSelector::new(PIECE_SIZE, TOTAL_SIZE, BLOCK_SIZE);
        state.piece_complete(0);
        state.peer_has_pieces(addr, BitSet::from_bytes(&[0b11000000]));

        assert_eq!(
            state.next().await,
            Some((addr, Block::new(1, 0, BLOCK_SIZE_BYTES)))
        );
    }

    #[tokio::test]
    async fn get_rarest_pieces_first() {
        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let addr2 = "127.0.0.2:6881".parse().unwrap();

        let mut state = PieceSelector::new(PIECE_SIZE, TOTAL_SIZE, BLOCK_SIZE);
        // Piece #0 is rarest - only peer 2 has it
        state.peer_has_pieces(addr1, BitSet::from_bytes(&[0b10000000]));
        state.peer_has_pieces(addr2, BitSet::from_bytes(&[0b11000000]));

        assert_eq!(
            state.next().await,
            Some((addr2, Block::new(1, 0, BLOCK_SIZE_BYTES)))
        );
    }

    #[tokio::test]
    async fn get_piece_blocks_in_order() {
        let addr = "127.0.0.1:6881".parse().unwrap();

        let mut state = PieceSelector::new(PIECE_SIZE, TOTAL_SIZE, BLOCK_SIZE);
        state.peer_has_pieces(addr, BitSet::from_bytes(&[0b10000000]));

        assert_eq!(state.next().await, Some((addr, Block::new(0, 0, 1024))));
        assert_eq!(state.next().await, Some((addr, Block::new(0, 1024, 1024))));
    }

    #[tokio::test]
    async fn assign_each_block_to_a_different_peer_ordered_by_transfer_rate() {
        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let addr2 = "127.0.0.2:6881".parse().unwrap();
        let addr3 = "127.0.0.3:6881".parse().unwrap();
        let addr4 = "127.0.0.4:6881".parse().unwrap();

        let mut state = PieceSelector::new(PIECE_SIZE, TOTAL_SIZE, BLOCK_SIZE);

        state.peer_has_pieces(addr1, BitSet::from_bytes(&[0b10000000]));
        state.update_transfer_rate(addr1, Size::from_kibibytes(10), Duration::from_secs(1));

        state.peer_has_pieces(addr2, BitSet::from_bytes(&[0b10000000]));
        state.update_transfer_rate(addr2, Size::from_kibibytes(20), Duration::from_secs(1));

        state.peer_has_pieces(addr3, BitSet::from_bytes(&[0b10000000]));
        state.update_transfer_rate(addr3, Size::from_kibibytes(30), Duration::from_secs(1));

        state.peer_has_pieces(addr4, BitSet::from_bytes(&[0b10000000]));
        state.update_transfer_rate(addr4, Size::from_kibibytes(40), Duration::from_secs(1));

        assert_eq!(state.next().await, Some((addr4, Block::new(0, 0, 1024))));
        assert_eq!(state.next().await, Some((addr3, Block::new(0, 1024, 1024))));
        assert_eq!(state.next().await, Some((addr2, Block::new(0, 2048, 1024))));
        assert_eq!(state.next().await, Some((addr1, Block::new(0, 3072, 1024))));
    }

    // TODO: distribute blocks between peers, by upload rate
}
