use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    sync::Arc,
};

use bit_set::BitSet;
use log::info;
use size::Size;
use tokio::{
    sync::{
        Mutex,
        mpsc::{self, Receiver, Sender},
    },
    task::JoinHandle,
};

use super::{blocks::Blocks, message::Block};

struct BlockAssigner {
    unchoked_peers: HashMap<SocketAddr, JoinHandle<()>>,
    assignment: Arc<Mutex<AssignmentState>>,
    tx: Sender<(SocketAddr, Block)>,
    rx: Receiver<(SocketAddr, Block)>,
}

impl BlockAssigner {
    pub fn new(piece_size: Size, total_size: Size, block_size: Size) -> Self {
        let (tx, rx) = mpsc::channel(16);
        Self {
            unchoked_peers: HashMap::new(),
            assignment: Arc::new(Mutex::new(AssignmentState::new(
                piece_size, total_size, block_size,
            ))),
            tx,
            rx,
        }
    }

    pub async fn peer_has_pieces(&mut self, addr: SocketAddr, pieces: &BitSet) {
        self.assignment.lock().await.peer_has_pieces(addr, pieces)
    }

    pub fn peer_unchoked(&mut self, addr: SocketAddr) {
        let tx = self.tx.clone();
        let assignment = Arc::clone(&self.assignment);
        let join_handle = tokio::spawn(async move {
            if let Some(block) = assignment.lock().await.assign_block_for(&addr) {
                tx.send((addr, block)).await.unwrap();
            }
        });
        self.unchoked_peers.insert(addr, join_handle);
    }

    pub async fn next(&mut self) -> (SocketAddr, Block) {
        self.rx.recv().await.unwrap()
    }
}

struct AssignmentState {
    piece_size: Size,
    total_size: Size,
    block_size: Size,
    peer_pieces: HashMap<SocketAddr, BitSet>,
    unassigned_blocks: HashMap<usize, VecDeque<Block>>,
}

impl AssignmentState {
    fn new(piece_size: Size, total_size: Size, block_size: Size) -> Self {
        Self {
            piece_size,
            total_size,
            block_size,
            peer_pieces: HashMap::new(),
            unassigned_blocks: HashMap::new(),
        }
    }

    fn peer_has_pieces(&mut self, addr: SocketAddr, pieces: &BitSet) {
        let peer_pieces = self.peer_pieces.entry(addr).or_default();
        peer_pieces.union_with(pieces);
    }

    fn assign_block_for(&mut self, addr: &SocketAddr) -> Option<Block> {
        let piece = self
            .peer_pieces
            .get(addr)
            .and_then(|pieces| pieces.iter().next())?;

        let piece_blocks = self.unassigned_blocks.entry(piece).or_insert_with(|| {
            Blocks::new(self.piece_size, self.total_size, self.block_size, piece).collect()
        });

        piece_blocks.pop_front()
    }
}

enum AssignmentStatus {
    Unassigned,
    Assigned,
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, time::Duration};

    use super::*;

    use size::KiB;
    use tokio::time::timeout;

    const PIECE_SIZE: Size = Size::from_const(8 * KiB);
    const TOTAL_SIZE: Size = Size::from_const(24 * KiB);
    const BLOCK_SIZE: Size = Size::from_const(KiB);
    const BLOCK_SIZE_BYTES: usize = BLOCK_SIZE.bytes() as usize;

    #[tokio::test]
    async fn peer_has_pieces_but_choking() {
        let addr = "127.0.0.1:6881".parse().unwrap();
        let mut block_assigner = BlockAssigner::new(PIECE_SIZE, TOTAL_SIZE, BLOCK_SIZE);
        block_assigner
            .peer_has_pieces(addr, &BitSet::from_bytes(&[0b10000000]))
            .await;
        let result = timeout(Duration::from_millis(10), block_assigner.next()).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn one_unchoked_peer() {
        let addr = "127.0.0.1:6881".parse().unwrap();
        let mut block_assigner = BlockAssigner::new(PIECE_SIZE, TOTAL_SIZE, BLOCK_SIZE);
        block_assigner
            .peer_has_pieces(addr, &BitSet::from_bytes(&[0b10000000]))
            .await;
        block_assigner.peer_unchoked(addr);

        assert_eq!(
            block_assigner.next().await,
            (addr, Block::new(0, 0, BLOCK_SIZE_BYTES))
        );
    }

    #[tokio::test]
    async fn distribute_same_piece_between_two_peers() {
        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let addr2 = "127.0.0.2:6881".parse().unwrap();

        let mut result = HashSet::with_capacity(2);
        let mut block_assigner = BlockAssigner::new(PIECE_SIZE, TOTAL_SIZE, BLOCK_SIZE);
        block_assigner
            .peer_has_pieces(addr1, &BitSet::from_bytes(&[0b10000000]))
            .await;
        block_assigner
            .peer_has_pieces(addr2, &BitSet::from_bytes(&[0b10000000]))
            .await;

        block_assigner.peer_unchoked(addr1);
        result.insert(block_assigner.next().await);

        block_assigner.peer_unchoked(addr2);
        result.insert(block_assigner.next().await);

        assert_eq!(result.len(), 2);
        assert!(result.contains(&(addr1, Block::new(0, 0, 1024))));
        assert!(result.contains(&(addr2, Block::new(0, 1024, 1024))));
    }
}
