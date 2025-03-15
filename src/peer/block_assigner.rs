use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    sync::Arc,
};

use bit_set::BitSet;
use size::Size;
use tokio::{
    sync::{
        Mutex,
        mpsc::{self, Receiver, Sender},
    },
    task::JoinHandle,
};

use crate::peer::{blocks::Blocks, message::Block};

pub struct BlockAssigner {
    unchoked_peers: HashMap<SocketAddr, PeerState>,
    assignment: Arc<Mutex<AssignmentState>>,
    tx: Sender<(SocketAddr, Block)>,
    rx: Receiver<(SocketAddr, Block)>,
}

impl BlockAssigner {
    pub fn new(piece_size: Size, total_size: Size, block_size: Size) -> Self {
        let assignment = AssignmentState::new(piece_size, total_size, block_size);
        let (tx, rx) = mpsc::channel(16);
        Self {
            unchoked_peers: HashMap::new(),
            assignment: Arc::new(Mutex::new(assignment)),
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
        let (rate_limit_tx, mut rate_limit_rx) = mpsc::channel(1);
        let join_handle = tokio::spawn(async move {
            loop {
                if let Some(block) = assignment.lock().await.assign_block_for(&addr) {
                    tx.send((addr, block)).await.unwrap();
                }
                rate_limit_rx.recv().await;
            }
        });
        self.unchoked_peers
            .insert(addr, PeerState::new(join_handle, rate_limit_tx));
    }

    pub fn peer_choked(&mut self, addr: &SocketAddr) {
        if let Some(peer) = self.unchoked_peers.remove(addr) {
            peer.join_handle.abort();
        }
    }

    pub async fn block_downloaded(&self, addr: &SocketAddr) {
        if let Some(peer) = self.unchoked_peers.get(addr) {
            peer.tx.send(()).await.unwrap();
        }
    }

    pub async fn next(&mut self) -> (SocketAddr, Block) {
        self.rx.recv().await.unwrap()
    }
}

struct PeerState {
    join_handle: JoinHandle<()>,
    tx: Sender<()>,
}

impl PeerState {
    fn new(join_handle: JoinHandle<()>, tx: Sender<()>) -> Self {
        Self { join_handle, tx }
    }
}

struct AssignmentState {
    piece_size: Size,
    total_size: Size,
    block_size: Size,
    pieces: HashMap<usize, PieceState>,
}

impl AssignmentState {
    fn new(piece_size: Size, total_size: Size, block_size: Size) -> Self {
        Self {
            piece_size,
            total_size,
            block_size,
            pieces: HashMap::new(),
        }
    }

    fn peer_has_pieces(&mut self, addr: SocketAddr, pieces: &BitSet) {
        for piece in pieces {
            let state = self.pieces.entry(piece).or_insert({
                let blocks = Blocks::new(self.piece_size, self.total_size, self.block_size, piece);
                PieceState::new(blocks)
            });
            state.peers_with_piece.insert(addr);
        }
    }

    fn assign_block_for(&mut self, addr: &SocketAddr) -> Option<Block> {
        let state = self
            .pieces
            .values_mut()
            .filter(|state| state.peers_with_piece.contains(addr))
            .min_by_key(|state| state.peers_with_piece.len())?;

        state.unassigned_blocks.pop_front()
    }
}

struct PieceState {
    peers_with_piece: HashSet<SocketAddr>,
    unassigned_blocks: VecDeque<Block>,
}

impl PieceState {
    fn new(blocks: Blocks) -> Self {
        Self {
            peers_with_piece: HashSet::new(),
            unassigned_blocks: blocks.collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

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

        let mut block_assigner = BlockAssigner::new(PIECE_SIZE, TOTAL_SIZE, BLOCK_SIZE);
        block_assigner
            .peer_has_pieces(addr1, &BitSet::from_bytes(&[0b10000000]))
            .await;
        block_assigner
            .peer_has_pieces(addr2, &BitSet::from_bytes(&[0b10000000]))
            .await;

        // Both peers have piece #0. Distribute its blocks among them.
        block_assigner.peer_unchoked(addr1);
        assert_eq!(block_assigner.next().await, (addr1, Block::new(0, 0, 1024)));

        block_assigner.peer_unchoked(addr2);
        assert_eq!(
            block_assigner.next().await,
            (addr2, Block::new(0, 1024, 1024))
        );
    }

    #[tokio::test]
    async fn select_rarest_pieces_first() {
        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let addr2 = "127.0.0.2:6881".parse().unwrap();

        let mut block_assigner = BlockAssigner::new(PIECE_SIZE, TOTAL_SIZE, BLOCK_SIZE);
        block_assigner
            .peer_has_pieces(addr1, &BitSet::from_bytes(&[0b10000000]))
            .await;
        block_assigner
            .peer_has_pieces(addr2, &BitSet::from_bytes(&[0b11000000]))
            .await;

        // Peer 1 only has piece #0, select it.
        block_assigner.peer_unchoked(addr1);
        assert_eq!(block_assigner.next().await, (addr1, Block::new(0, 0, 1024)));

        // Peer 2 has both piece #0 and #1. Since piece #1 is rarer, slect it first.
        block_assigner.peer_unchoked(addr2);
        assert_eq!(block_assigner.next().await, (addr2, Block::new(1, 0, 1024)));
    }

    #[tokio::test]
    async fn wait_until_previous_block_is_completed_before_emitting_next_block() {
        let addr = "127.0.0.1:6881".parse().unwrap();
        let mut block_assigner = BlockAssigner::new(PIECE_SIZE, TOTAL_SIZE, BLOCK_SIZE);
        block_assigner
            .peer_has_pieces(addr, &BitSet::from_bytes(&[0b10000000]))
            .await;
        block_assigner.peer_unchoked(addr);

        assert_eq!(block_assigner.next().await, (addr, Block::new(0, 0, 1024)));

        // Next poll hangs
        let result = timeout(Duration::from_millis(10), block_assigner.next()).await;
        assert!(result.is_err());

        // Continue with next block once previous one completes
        block_assigner.block_downloaded(&addr).await;
        assert_eq!(
            block_assigner.next().await,
            (addr, Block::new(0, 1024, 1024))
        );
    }

    // TODO: timeout
}
