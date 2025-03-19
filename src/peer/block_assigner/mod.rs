mod assignment;
mod peer_handle;

use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use bit_set::BitSet;
use size::Size;
use tokio::sync::{
    Mutex,
    mpsc::{self, Receiver, Sender},
};

use crate::peer::block_assigner::assignment::Assignment;
use crate::peer::block_assigner::peer_handle::PeerHandle;
use crate::peer::message::Block;

pub struct BlockAssigner {
    config: Arc<BlockAssignerConfig>,
    unchoked_peers: HashMap<SocketAddr, PeerHandle>,
    assignment: Arc<Mutex<Assignment>>,
    tx: Sender<(SocketAddr, Block)>,
    rx: Receiver<(SocketAddr, Block)>,
}

impl BlockAssigner {
    pub fn new(
        piece_size: Size,
        total_size: Size,
        block_size: Size,
        config: BlockAssignerConfig,
    ) -> Self {
        let assignment = Assignment::new(piece_size, total_size, block_size);
        let (tx, rx) = mpsc::channel(16);
        Self {
            config: Arc::new(config),
            unchoked_peers: HashMap::new(),
            assignment: Arc::new(Mutex::new(assignment)),
            tx,
            rx,
        }
    }

    pub async fn peer_has_pieces(&mut self, addr: SocketAddr, pieces: &BitSet) {
        self.assignment.lock().await.peer_has_pieces(addr, pieces);
        if let Some(peer) = self.unchoked_peers.get(&addr) {
            peer.notify();
        }
    }

    pub async fn peer_disconnected(&mut self, addr: &SocketAddr) {
        self.assignment.lock().await.peer_disconnected(addr);
    }

    pub fn peer_unchoked(&mut self, addr: SocketAddr) {
        let peer = PeerHandle::spawn(
            Arc::clone(&self.assignment),
            Arc::clone(&self.config),
            self.tx.clone(),
            addr,
        );
        self.unchoked_peers.insert(addr, peer);
    }

    pub async fn peer_choked(&mut self, addr: &SocketAddr) {
        self.unchoked_peers.remove(addr);
        self.assignment.lock().await.peer_choked(addr);
    }

    pub async fn block_downloaded(&self, addr: &SocketAddr, block: &Block) {
        self.assignment.lock().await.block_downloaded(addr, block);
        if let Some(peer) = self.unchoked_peers.get(addr) {
            peer.notify();
        }
    }

    pub async fn next(&mut self) -> (SocketAddr, Block) {
        self.rx.recv().await.unwrap()
    }
}

#[derive(Clone)]
pub struct BlockAssignerConfig {
    pub block_timeout: Duration,
}

impl Default for BlockAssignerConfig {
    fn default() -> Self {
        Self {
            block_timeout: Duration::from_secs(5),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::time;

    const PIECE_SIZE: Size = Size::from_const(24);
    const TOTAL_SIZE: Size = Size::from_const(32);
    const BLOCK_SIZE: Size = Size::from_const(8);

    #[tokio::test]
    async fn peer_has_pieces_but_choking() {
        let mut block_assigner = BlockAssigner::new(
            PIECE_SIZE,
            TOTAL_SIZE,
            BLOCK_SIZE,
            BlockAssignerConfig::default(),
        );

        let addr = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_bytes(&[0b10000000]);
        block_assigner.peer_has_pieces(addr, &pieces).await;

        let result = time::timeout(Duration::from_millis(10), block_assigner.next()).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn one_unchoked_peer() {
        let mut block_assigner = BlockAssigner::new(
            PIECE_SIZE,
            TOTAL_SIZE,
            BLOCK_SIZE,
            BlockAssignerConfig::default(),
        );

        let addr = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_bytes(&[0b10000000]);
        block_assigner.peer_has_pieces(addr, &pieces).await;
        block_assigner.peer_unchoked(addr);

        assert_eq!(block_assigner.next().await, (addr, Block::new(0, 0, 8)));
    }

    #[tokio::test]
    async fn peer_unchoked_before_notifying_on_completed_piece() {
        let mut block_assigner = BlockAssigner::new(
            PIECE_SIZE,
            TOTAL_SIZE,
            BLOCK_SIZE,
            BlockAssignerConfig::default(),
        );

        let addr = "127.0.0.1:6881".parse().unwrap();
        block_assigner.peer_unchoked(addr);

        let pieces = BitSet::from_bytes(&[0b10000000]);
        block_assigner.peer_has_pieces(addr, &pieces).await;

        assert_eq!(block_assigner.next().await, (addr, Block::new(0, 0, 8)));
    }

    #[tokio::test]
    async fn distribute_same_piece_between_two_peers() {
        let mut block_assigner = BlockAssigner::new(
            PIECE_SIZE,
            TOTAL_SIZE,
            BLOCK_SIZE,
            BlockAssignerConfig::default(),
        );

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_bytes(&[0b10000000]);
        block_assigner.peer_has_pieces(addr1, &pieces).await;

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        block_assigner.peer_has_pieces(addr2, &pieces).await;

        // Both peers have piece #0. Distribute its blocks among them.
        block_assigner.peer_unchoked(addr1);
        assert_eq!(block_assigner.next().await, (addr1, Block::new(0, 0, 8)));

        block_assigner.peer_unchoked(addr2);
        assert_eq!(block_assigner.next().await, (addr2, Block::new(0, 8, 8)));
    }

    #[tokio::test]
    async fn select_rarest_pieces_first() {
        let mut block_assigner = BlockAssigner::new(
            PIECE_SIZE,
            TOTAL_SIZE,
            BLOCK_SIZE,
            BlockAssignerConfig::default(),
        );

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_bytes(&[0b10000000]);
        block_assigner.peer_has_pieces(addr1, &pieces).await;

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        let pieces = BitSet::from_bytes(&[0b11000000]);
        block_assigner.peer_has_pieces(addr2, &pieces).await;

        // Peer 2 has both piece #0 and #1. Since piece #1 is rarer, slect it first.
        block_assigner.peer_unchoked(addr2);
        assert_eq!(block_assigner.next().await, (addr2, Block::new(1, 0, 8)));

        // Peer 1 only has piece #0, select it.
        block_assigner.peer_unchoked(addr1);
        assert_eq!(block_assigner.next().await, (addr1, Block::new(0, 0, 8)));
    }

    #[tokio::test]
    async fn prioritize_pieces_that_already_started_downloading() {
        let mut block_assigner = BlockAssigner::new(
            PIECE_SIZE,
            TOTAL_SIZE,
            BLOCK_SIZE,
            BlockAssignerConfig::default(),
        );

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_bytes(&[0b10000000]);
        block_assigner.peer_has_pieces(addr1, &pieces).await;

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        let pieces = BitSet::from_bytes(&[0b11000000]);
        block_assigner.peer_has_pieces(addr2, &pieces).await;

        // Peer 1 only has piece #0, select it.
        block_assigner.peer_unchoked(addr1);
        assert_eq!(block_assigner.next().await, (addr1, Block::new(0, 0, 8)));

        // Peer 2 has both piece #0 and #1. Piece #1 is rarer, but peer 1 already started
        // downloading piece #0, so prioritize it first.
        block_assigner.peer_unchoked(addr2);
        assert_eq!(block_assigner.next().await, (addr2, Block::new(0, 8, 8)));
    }

    #[tokio::test]
    async fn wait_until_previous_block_is_completed_before_emitting_next_block() {
        let mut block_assigner = BlockAssigner::new(
            PIECE_SIZE,
            TOTAL_SIZE,
            BLOCK_SIZE,
            BlockAssignerConfig::default(),
        );

        let addr = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_bytes(&[0b10000000]);
        block_assigner.peer_has_pieces(addr, &pieces).await;
        block_assigner.peer_unchoked(addr);

        let block = Block::new(0, 0, 8);
        assert_eq!(block_assigner.next().await, (addr, block));

        // Next poll hangs
        let result = time::timeout(Duration::from_millis(10), block_assigner.next()).await;
        assert!(result.is_err());

        // Continue with next block once previous one completes
        block_assigner.block_downloaded(&addr, &block).await;
        assert_eq!(block_assigner.next().await, (addr, Block::new(0, 8, 8)));
    }

    #[tokio::test]
    async fn give_up_on_block_after_timeout_expires() {
        time::pause();
        let mut block_assigner = BlockAssigner::new(
            PIECE_SIZE,
            TOTAL_SIZE,
            BLOCK_SIZE,
            BlockAssignerConfig::default(),
        );

        let addr = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_bytes(&[0b10000000]);
        block_assigner.peer_has_pieces(addr, &pieces).await;
        block_assigner.peer_unchoked(addr);

        assert_eq!(block_assigner.next().await, (addr, Block::new(0, 0, 8)));

        //time::advance(Duration::from_secs(5)).await;

        assert_eq!(block_assigner.next().await, (addr, Block::new(0, 0, 8)));
    }

    #[tokio::test]
    async fn do_not_release_downloaded_blocks() {
        time::pause();
        let mut block_assigner = BlockAssigner::new(
            PIECE_SIZE,
            TOTAL_SIZE,
            BLOCK_SIZE,
            BlockAssignerConfig::default(),
        );

        let addr = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_bytes(&[0b10000000]);
        block_assigner.peer_has_pieces(addr, &pieces).await;
        block_assigner.peer_unchoked(addr);

        let block1 = Block::new(0, 0, 8);
        assert_eq!(block_assigner.next().await, (addr, block1));
        block_assigner.block_downloaded(&addr, &block1).await;

        time::advance(Duration::from_secs(5)).await;

        let block2 = Block::new(0, 8, 8);
        assert_eq!(block_assigner.next().await, (addr, block2));
        block_assigner.block_downloaded(&addr, &block2).await;

        let block3 = Block::new(0, 16, 8);
        assert_eq!(block_assigner.next().await, (addr, block3));
    }

    #[tokio::test]
    async fn assign_abandoned_block_to_other_peer_if_needed() {
        let mut block_assigner = BlockAssigner::new(
            PIECE_SIZE,
            TOTAL_SIZE,
            BLOCK_SIZE,
            BlockAssignerConfig::default(),
        );

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_bytes(&[0b10000000]);
        block_assigner.peer_has_pieces(addr1, &pieces).await;
        block_assigner.peer_unchoked(addr1);

        assert_eq!(block_assigner.next().await, (addr1, Block::new(0, 0, 8)));

        // Peer 1 choked before completing the block, assign to peer 2
        block_assigner.peer_choked(&addr1).await;

        let addr2 = "127.0.0.1:6881".parse().unwrap();
        block_assigner.peer_has_pieces(addr2, &pieces).await;
        block_assigner.peer_unchoked(addr2);

        assert_eq!(block_assigner.next().await, (addr2, Block::new(0, 0, 8)));
    }

    #[tokio::test]
    async fn do_not_reassign_downloaded_block() {
        let mut block_assigner = BlockAssigner::new(
            PIECE_SIZE,
            TOTAL_SIZE,
            BLOCK_SIZE,
            BlockAssignerConfig::default(),
        );

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let pieces = BitSet::from_bytes(&[0b10000000]);
        block_assigner.peer_has_pieces(addr1, &pieces).await;
        block_assigner.peer_unchoked(addr1);

        let block = Block::new(0, 0, 8);
        assert_eq!(block_assigner.next().await, (addr1, block));

        // Peer 1 completed downloading block and choked. Assign next block to peer 2
        block_assigner.block_downloaded(&addr1, &block).await;
        block_assigner.peer_choked(&addr1).await;

        let addr2 = "127.0.0.1:6881".parse().unwrap();
        block_assigner.peer_has_pieces(addr2, &pieces).await;
        block_assigner.peer_unchoked(addr2);

        assert_eq!(block_assigner.next().await, (addr2, Block::new(0, 8, 8)));
    }
}
