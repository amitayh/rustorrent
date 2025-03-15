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
        self.tick += 1;
        let optimistic = self.tick % self.optimistic_choking_cycle == 0;
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

        let mut peers_to_choke = self.unchoked_peers.clone();
        let mut peers_to_unchoke = HashSet::with_capacity(TOP_PEERS + 1);
        for Reverse(PeerByTransferRate(peer, _)) in top_peers {
            peers_to_choke.remove(&peer);
            peers_to_unchoke.insert(peer);
        }

        if optimistic {
            // Add one randomly selected peer from remaining interested peers
            let remaining = self.interested_peers.difference(&peers_to_unchoke);
            if let Some(peer) = remaining.choose(&mut rand::rng()) {
                peers_to_choke.remove(peer);
                peers_to_unchoke.insert(*peer);
            }
        }

        ChokeDecision {
            peers_to_choke,
            peers_to_unchoke,
        }
    }
}

#[derive(Debug)]
pub struct ChokeDecision {
    pub peers_to_choke: HashSet<SocketAddr>,
    pub peers_to_unchoke: HashSet<SocketAddr>,
}

pub fn choke(
    interested: &HashSet<SocketAddr>,
    unchoked: &HashSet<SocketAddr>,
    transfer_rates: &HashMap<SocketAddr, TransferRate>,
    optimistic: bool,
) -> ChokeDecision {
    let mut top_peers = BinaryHeap::with_capacity(TOP_PEERS + 1);
    for peer in interested {
        let transfer_rate = transfer_rates.get(peer).unwrap_or(&TransferRate::EMPTY);
        top_peers.push(Reverse(PeerByTransferRate(*peer, *transfer_rate)));
        if top_peers.len() > TOP_PEERS {
            top_peers.pop();
        }
    }

    let mut peers_to_choke = unchoked.clone();
    let mut peers_to_unchoke = HashSet::with_capacity(TOP_PEERS + 1);
    for Reverse(PeerByTransferRate(peer, _)) in top_peers {
        peers_to_choke.remove(&peer);
        peers_to_unchoke.insert(peer);
    }

    if optimistic {
        // Add one randomly selected peer from remaining interested peers
        let remaining = interested.difference(&peers_to_unchoke);
        if let Some(peer) = remaining.choose(&mut rand::rng()) {
            peers_to_choke.remove(peer);
            peers_to_unchoke.insert(*peer);
        }
    }

    ChokeDecision {
        peers_to_choke,
        peers_to_unchoke,
    }
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
        let peer1 = "127.0.0.1:6881".parse().unwrap();
        let peer2 = "127.0.0.2:6881".parse().unwrap();

        let mut choker = Choker::new(3);
        choker.peer_interested(peer1);
        choker.peer_interested(peer2);
        choker.update_peer_transfer_rate(peer1, TransferRate(Size::from_kibibytes(10), SEC));
        choker.update_peer_transfer_rate(peer2, TransferRate(Size::from_kibibytes(10), SEC));
        let decision = choker.run();

        assert_eq!(decision.peers_to_unchoke, HashSet::from([peer1, peer2]));
        assert!(decision.peers_to_choke.is_empty());
    }

    #[test]
    fn select_top_peers_to_unchoke_by_transfer_rate() {
        let peer1 = "127.0.0.1:6881".parse().unwrap();
        let peer2 = "127.0.0.2:6881".parse().unwrap(); // Too slow
        let peer3 = "127.0.0.3:6881".parse().unwrap();
        let peer4 = "127.0.0.4:6881".parse().unwrap();
        let peer5 = "127.0.0.5:6881".parse().unwrap(); // Not interested

        let mut choker = Choker::new(3);
        choker.peer_interested(peer1);
        choker.peer_interested(peer2);
        choker.peer_interested(peer3);
        choker.peer_interested(peer4);
        choker.update_peer_transfer_rate(peer1, TransferRate(Size::from_kibibytes(20), SEC));
        choker.update_peer_transfer_rate(peer2, TransferRate(Size::from_kibibytes(10), SEC));
        choker.update_peer_transfer_rate(peer3, TransferRate(Size::from_kibibytes(30), SEC));
        choker.update_peer_transfer_rate(peer4, TransferRate(Size::from_kibibytes(40), SEC));
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
        let peer1 = "127.0.0.1:6881".parse().unwrap();
        let peer2 = "127.0.0.2:6881".parse().unwrap(); // Too slow
        let peer3 = "127.0.0.3:6881".parse().unwrap();
        let peer4 = "127.0.0.4:6881".parse().unwrap();
        let peer5 = "127.0.0.5:6881".parse().unwrap(); // Not interested

        //let mut choker = Choker::new(3);
        //choker.peer_interested(peer1);
        //choker.peer_interested(peer2);
        //choker.peer_interested(peer3);
        //choker.peer_interested(peer4);

        let interested = HashSet::from([peer1, peer2, peer3, peer4]);
        let unchoked = HashSet::from([peer1, peer2, peer5]);
        let transfer_rate = HashMap::from([
            (peer1, TransferRate(Size::from_kibibytes(20), SEC)),
            (peer2, TransferRate(Size::from_kibibytes(10), SEC)),
            (peer3, TransferRate(Size::from_kibibytes(30), SEC)),
            (peer4, TransferRate(Size::from_kibibytes(40), SEC)),
            (peer5, TransferRate(Size::from_kibibytes(50), SEC)),
        ]);
        let decision = choke(&interested, &unchoked, &transfer_rate, false);

        assert_eq!(decision.peers_to_choke, HashSet::from([peer2, peer5]));
    }

    #[test]
    fn optimistically_unchoke_randomly_selected_peer() {
        let peer1 = "127.0.0.1:6881".parse().unwrap();
        let peer2 = "127.0.0.2:6881".parse().unwrap(); // Too slow
        let peer3 = "127.0.0.3:6881".parse().unwrap();
        let peer4 = "127.0.0.4:6881".parse().unwrap();
        let peer5 = "127.0.0.5:6881".parse().unwrap(); // Not interested

        let interested = HashSet::from([peer1, peer2, peer3, peer4]);
        let unchoked = HashSet::new();
        let transfer_rate = HashMap::from([
            (peer1, TransferRate(Size::from_kibibytes(20), SEC)),
            (peer2, TransferRate(Size::from_kibibytes(10), SEC)),
            (peer3, TransferRate(Size::from_kibibytes(30), SEC)),
            (peer4, TransferRate(Size::from_kibibytes(40), SEC)),
            (peer5, TransferRate(Size::from_kibibytes(50), SEC)),
        ]);
        let decision = choke(&interested, &unchoked, &transfer_rate, true);

        assert_eq!(
            decision.peers_to_unchoke,
            HashSet::from([peer1, peer2, peer3, peer4])
        );
    }

    #[test]
    fn keep_unchoked_peers_if_selected_again() {
        let peer1 = "127.0.0.1:6881".parse().unwrap();
        let peer2 = "127.0.0.2:6881".parse().unwrap();
        let peer3 = "127.0.0.3:6881".parse().unwrap();
        let peer4 = "127.0.0.4:6881".parse().unwrap();

        let interested = HashSet::from([peer1, peer2, peer3, peer4]);
        let unchoked = HashSet::from([peer1, peer2, peer3, peer4]);
        let transfer_rate = HashMap::from([
            (peer1, TransferRate(Size::from_kibibytes(10), SEC)),
            (peer2, TransferRate(Size::from_kibibytes(20), SEC)),
            (peer3, TransferRate(Size::from_kibibytes(30), SEC)),
            (peer4, TransferRate(Size::from_kibibytes(40), SEC)),
        ]);
        let decision = choke(&interested, &unchoked, &transfer_rate, true);

        assert!(decision.peers_to_choke.is_empty());
    }
}
