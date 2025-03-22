use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::net::SocketAddr;

use rand::seq::IteratorRandom;

use crate::peer::transfer_rate::TransferRate;

const TOP_PEERS: usize = 3;

pub struct Choker {
    interested_peers: HashSet<SocketAddr>,
    unchoked_peers: HashSet<SocketAddr>,
    transfer_rates: HashMap<SocketAddr, TransferRate>,
    optimistic_choking_cycle: usize,
    tick: usize,
}

impl Choker {
    pub fn new(optimistic_choking_cycle: usize) -> Self {
        Self {
            interested_peers: HashSet::new(),
            unchoked_peers: HashSet::new(),
            transfer_rates: HashMap::new(),
            optimistic_choking_cycle,
            tick: 0,
        }
    }

    pub fn peer_interested(&mut self, addr: SocketAddr) {
        self.interested_peers.insert(addr);
    }

    pub fn peer_not_interested(&mut self, addr: &SocketAddr) {
        self.interested_peers.remove(addr);
    }

    pub fn update_peer_transfer_rate(&mut self, addr: SocketAddr, transfer_rate: TransferRate) {
        let entry = self
            .transfer_rates
            .entry(addr)
            .or_insert(TransferRate::EMPTY);

        *entry += transfer_rate;
    }

    pub fn peer_disconnected(&mut self, addr: &SocketAddr) {
        self.interested_peers.remove(addr);
        self.unchoked_peers.remove(addr);
        self.transfer_rates.remove(addr);
    }

    pub fn run(&mut self) -> ChokeDecision {
        // TODO: test take
        let mut peers_to_choke = std::mem::take(&mut self.unchoked_peers);
        let mut peers_to_unchoke = HashSet::with_capacity(TOP_PEERS + 1);
        for peer in self.top_peers_by_transfer_rate() {
            self.unchoked_peers.insert(peer);
            if !peers_to_choke.remove(&peer) {
                peers_to_unchoke.insert(peer);
            }
        }

        self.tick += 1;
        if self.tick % self.optimistic_choking_cycle == 0 {
            // Optimistic run: randomly unchaoke one remaining peer
            if let Some(peer) = self.random_interested_peer() {
                self.unchoked_peers.insert(peer);
                if !peers_to_choke.remove(&peer) {
                    peers_to_unchoke.insert(peer);
                }
            }
        }

        ChokeDecision {
            peers_to_choke,
            peers_to_unchoke,
        }
    }

    pub fn is_unchoked(&self, addr: &SocketAddr) -> bool {
        self.unchoked_peers.contains(addr)
    }

    fn top_peers_by_transfer_rate(&self) -> impl Iterator<Item = SocketAddr> + use<> {
        let mut top_peers = BinaryHeap::with_capacity(TOP_PEERS + 1);
        for peer in &self.interested_peers {
            let transfer_rate = self
                .transfer_rates
                .get(peer)
                .unwrap_or(&TransferRate::EMPTY);
            top_peers.push(Reverse(PeerByTransferRate(*peer, *transfer_rate)));
            if top_peers.len() > TOP_PEERS {
                top_peers.pop();
            }
        }
        top_peers
            .into_iter()
            .map(|Reverse(PeerByTransferRate(peer, _))| peer)
    }

    fn random_interested_peer(&self) -> Option<SocketAddr> {
        let remaining = self.interested_peers.difference(&self.unchoked_peers);
        remaining.choose(&mut rand::rng()).copied()
    }
}

#[derive(Debug)]
pub struct ChokeDecision {
    pub peers_to_choke: HashSet<SocketAddr>,
    pub peers_to_unchoke: HashSet<SocketAddr>,
}

struct PeerByTransferRate(SocketAddr, TransferRate);

impl Eq for PeerByTransferRate {}

impl PartialEq for PeerByTransferRate {
    fn eq(&self, other: &Self) -> bool {
        self.1.eq(&other.1)
    }
}

impl Ord for PeerByTransferRate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.1.cmp(&other.1)
    }
}

impl PartialOrd for PeerByTransferRate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, time::Duration};

    use size::Size;

    use super::*;

    const SEC: Duration = Duration::from_secs(1);

    #[test]
    fn less_than_3_interested_peers() {
        let mut choker = Choker::new(3);

        let peer1 = "127.0.0.1:6881".parse().unwrap();
        choker.peer_interested(peer1);

        let peer2 = "127.0.0.2:6881".parse().unwrap();
        choker.peer_interested(peer2);

        let decision = choker.run();
        assert_eq!(decision.peers_to_unchoke, HashSet::from([peer1, peer2]));
        assert!(decision.peers_to_choke.is_empty());
    }

    #[test]
    fn select_top_peers_to_unchoke_by_transfer_rate() {
        let mut choker = Choker::new(3);

        let peer1 = "127.0.0.1:6881".parse().unwrap();
        choker.update_peer_transfer_rate(peer1, TransferRate(Size::from_kibibytes(20), SEC));
        choker.peer_interested(peer1);

        // Too slow
        let peer2 = "127.0.0.2:6881".parse().unwrap();
        choker.update_peer_transfer_rate(peer2, TransferRate(Size::from_kibibytes(10), SEC));
        choker.peer_interested(peer2);

        let peer3 = "127.0.0.3:6881".parse().unwrap();
        choker.update_peer_transfer_rate(peer3, TransferRate(Size::from_kibibytes(30), SEC));
        choker.peer_interested(peer3);

        let peer4 = "127.0.0.4:6881".parse().unwrap();
        choker.update_peer_transfer_rate(peer4, TransferRate(Size::from_kibibytes(40), SEC));
        choker.peer_interested(peer4);

        // Not interested
        let peer5 = "127.0.0.5:6881".parse().unwrap();
        choker.update_peer_transfer_rate(peer5, TransferRate(Size::from_kibibytes(50), SEC));

        let decision = choker.run();

        assert_eq!(
            decision.peers_to_unchoke,
            HashSet::from([peer1, peer3, peer4])
        );
        assert!(decision.peers_to_choke.is_empty());
    }

    #[test]
    fn rechoke_previously_unchoked_peers_if_not_selected() {
        let mut choker = Choker::new(3);

        let peer1 = "127.0.0.1:6881".parse().unwrap();
        choker.update_peer_transfer_rate(peer1, TransferRate(Size::from_kibibytes(20), SEC));
        choker.peer_interested(peer1);

        // Too slow
        let peer2 = "127.0.0.2:6881".parse().unwrap();
        choker.update_peer_transfer_rate(peer2, TransferRate(Size::from_kibibytes(10), SEC));
        choker.peer_interested(peer2);

        let peer3 = "127.0.0.3:6881".parse().unwrap();
        choker.update_peer_transfer_rate(peer3, TransferRate(Size::from_kibibytes(30), SEC));

        let peer4 = "127.0.0.4:6881".parse().unwrap();
        choker.update_peer_transfer_rate(peer4, TransferRate(Size::from_kibibytes(40), SEC));

        let peer5 = "127.0.0.5:6881".parse().unwrap();
        choker.update_peer_transfer_rate(peer5, TransferRate(Size::from_kibibytes(50), SEC));
        choker.peer_interested(peer5);

        // Unchokes peers 1, 2, 5
        let decision = choker.run();
        assert_eq!(
            decision.peers_to_unchoke,
            HashSet::from([peer1, peer2, peer5])
        );

        // Peers 3 and 4 are also interested
        choker.peer_interested(peer3);
        choker.peer_interested(peer4);

        // Peer 5 no longer interested
        choker.peer_not_interested(&peer5);

        let decision = choker.run();
        assert_eq!(decision.peers_to_unchoke, HashSet::from([peer3, peer4]));
        assert_eq!(decision.peers_to_choke, HashSet::from([peer2, peer5]));
    }

    #[test]
    fn optimistically_unchoke_randomly_selected_peer() {
        let mut choker = Choker::new(1); // Ensure first run is optimistic

        let peer1 = "127.0.0.1:6881".parse().unwrap();
        choker.update_peer_transfer_rate(peer1, TransferRate(Size::from_kibibytes(20), SEC));
        choker.peer_interested(peer1);

        // Too slow, selected optimistically
        let peer2 = "127.0.0.2:6881".parse().unwrap();
        choker.update_peer_transfer_rate(peer2, TransferRate(Size::from_kibibytes(10), SEC));
        choker.peer_interested(peer2);

        let peer3 = "127.0.0.3:6881".parse().unwrap();
        choker.update_peer_transfer_rate(peer3, TransferRate(Size::from_kibibytes(30), SEC));
        choker.peer_interested(peer3);

        let peer4 = "127.0.0.4:6881".parse().unwrap();
        choker.update_peer_transfer_rate(peer4, TransferRate(Size::from_kibibytes(40), SEC));
        choker.peer_interested(peer4);

        // Not interested
        let peer5 = "127.0.0.5:6881".parse().unwrap();
        choker.update_peer_transfer_rate(peer5, TransferRate(Size::from_kibibytes(50), SEC));

        let decision = choker.run();
        assert_eq!(
            decision.peers_to_unchoke,
            HashSet::from([peer1, peer2, peer3, peer4])
        );
    }

    #[test]
    fn keep_unchoked_peers_if_selected_again() {
        let mut choker = Choker::new(3);

        let peer1 = "127.0.0.1:6881".parse().unwrap();
        choker.peer_interested(peer1);

        let peer2 = "127.0.0.2:6881".parse().unwrap();
        choker.peer_interested(peer2);

        let peer3 = "127.0.0.3:6881".parse().unwrap();
        choker.peer_interested(peer3);

        // Unchokes all peers
        let decision = choker.run();
        assert_eq!(
            decision.peers_to_unchoke,
            HashSet::from([peer1, peer2, peer3])
        );

        // Keeps all peers unchoked in subsequent runs
        let decision = choker.run();
        assert!(decision.peers_to_choke.is_empty());
        assert!(decision.peers_to_unchoke.is_empty());

        let decision = choker.run();
        assert!(decision.peers_to_choke.is_empty());
        assert!(decision.peers_to_unchoke.is_empty());
    }
}
