use std::{cmp::Reverse, collections::VecDeque, net::SocketAddr, time::Duration};

use priority_queue::PriorityQueue;
use tokio::time::Instant;

use crate::message::Block;

pub struct Sweeper {
    limit: Duration,
    peer_activity: PriorityQueue<SocketAddr, Reverse<Instant>>,
    blocks_in_flight: PriorityQueue<(SocketAddr, Block), Reverse<Instant>>,
}

impl Sweeper {
    pub fn new(limit: Duration) -> Self {
        Self {
            limit,
            peer_activity: PriorityQueue::new(),
            blocks_in_flight: PriorityQueue::new(),
        }
    }

    pub fn update(&mut self, addr: SocketAddr, instant: Instant) {
        self.peer_activity.push_decrease(addr, Reverse(instant));
    }

    fn block_requested(&mut self, addr: SocketAddr, block: Block, instant: Instant) {
        self.blocks_in_flight
            .push_decrease((addr, block), Reverse(instant));
    }

    fn block_downloaded(&mut self, addr: SocketAddr, block: Block) {
        self.blocks_in_flight.remove(&(addr, block));
    }

    pub fn sweep(&mut self, now: Instant) -> SweepResult {
        let mut peers = Vec::new();
        while let Some(addr) = self.pop_idle_peer(now) {
            peers.push(addr);
        }
        let mut blocks = Vec::new();
        while let Some(block) = self.pop_abandoned_block(now) {
            blocks.push(block);
        }
        SweepResult { peers, blocks }
    }

    pub fn peer_disconnected(&mut self, addr: &SocketAddr) {
        self.peer_activity.remove(addr);
        self.blocks_in_flight
            .retain_mut(|(other, _), _| other != addr);
    }

    fn pop_idle_peer(&mut self, now: Instant) -> Option<SocketAddr> {
        self.peer_activity
            .pop_if(|_, Reverse(last_activity)| *last_activity + self.limit <= now)
            .map(|(addr, _)| addr)
    }

    fn pop_abandoned_block(&mut self, now: Instant) -> Option<(SocketAddr, Block)> {
        self.blocks_in_flight
            .pop_if(|_, Reverse(last_activity)| *last_activity + self.limit <= now)
            .map(|(block, _)| block)
    }
}

pub struct SweepResult {
    pub peers: Vec<SocketAddr>,
    pub blocks: Vec<(SocketAddr, Block)>,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn no_peer_to_sweep() {
        let mut sweeper = Sweeper::new(Duration::from_secs(2));
        let now = Instant::now();

        let addr = "127.0.0.1:6881".parse().unwrap();
        sweeper.update(addr, now);

        assert!(sweeper.sweep(now).peers.is_empty());
    }

    #[test]
    fn sweep_idle_peer() {
        let mut sweeper = Sweeper::new(Duration::from_secs(2));
        let now = Instant::now();

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        sweeper.update(addr1, now);

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        sweeper.update(addr2, now);

        // Peer #1 was active more recently, keep him
        sweeper.update(addr1, now + Duration::from_secs(1));

        assert_eq!(
            sweeper.sweep(now + Duration::from_secs(2)).peers,
            vec![addr2]
        );
    }

    #[test]
    fn no_block_to_sweep() {
        let mut sweeper = Sweeper::new(Duration::from_secs(2));
        let now = Instant::now();

        let addr = "127.0.0.1:6881".parse().unwrap();
        let block = Block::new(0, 0, 8);
        sweeper.block_requested(addr, block, now);

        assert!(sweeper.sweep(now).blocks.is_empty());
    }

    #[test]
    fn sweep_blocks_that_are_in_flight_for_too_long() {
        let mut sweeper = Sweeper::new(Duration::from_secs(2));
        let now = Instant::now();

        let addr1 = "127.0.0.1:6881".parse().unwrap();
        let block1 = Block::new(0, 0, 8);
        sweeper.block_requested(addr1, block1, now);

        let addr2 = "127.0.0.2:6881".parse().unwrap();
        let block2 = Block::new(0, 8, 8);
        sweeper.block_requested(addr2, block2, now + Duration::from_secs(1));

        assert_eq!(
            sweeper.sweep(now + Duration::from_secs(2)).blocks,
            vec![(addr1, block1)]
        );
    }

    #[test]
    fn mark_downloaded_blocks() {
        let mut sweeper = Sweeper::new(Duration::from_secs(2));
        let now = Instant::now();

        let addr = "127.0.0.1:6881".parse().unwrap();
        let block = Block::new(0, 0, 8);
        sweeper.block_requested(addr, block, now);

        sweeper.block_downloaded(addr, block);

        assert!(
            sweeper
                .sweep(now + Duration::from_secs(2))
                .blocks
                .is_empty()
        );
    }
}
