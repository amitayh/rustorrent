use std::{cmp::Reverse, net::SocketAddr, time::Duration};

use priority_queue::PriorityQueue;
use tokio::time::Instant;

pub struct Sweeper {
    limit: Duration,
    peer_activity: PriorityQueue<SocketAddr, Reverse<Instant>>,
}

impl Sweeper {
    pub fn new(limit: Duration) -> Self {
        Self {
            limit,
            peer_activity: PriorityQueue::new(),
        }
    }

    pub fn update(&mut self, addr: SocketAddr, instant: Instant) {
        self.peer_activity.push_decrease(addr, Reverse(instant));
    }

    pub fn sweep(&mut self, now: Instant) -> Vec<SocketAddr> {
        let mut result = Vec::new();
        while let Some(addr) = self.pop_idle_peer(now) {
            result.push(addr);
        }
        result
    }

    pub fn peer_disconnected(&mut self, addr: &SocketAddr) {
        self.peer_activity.remove(addr);
    }

    fn pop_idle_peer(&mut self, now: Instant) -> Option<SocketAddr> {
        self.peer_activity
            .pop_if(|_, Reverse(last_activity)| *last_activity + self.limit <= now)
            .map(|(addr, _)| addr)
    }
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

        assert!(sweeper.sweep(now).is_empty());
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

        assert_eq!(sweeper.sweep(now + Duration::from_secs(2)), vec![addr2]);
    }
}
