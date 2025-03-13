use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::net::SocketAddr;

use rand::seq::IteratorRandom;

use crate::peer::transfer_rate::TransferRate;

const TOP_PEERS: usize = 3;

#[derive(Debug)]
pub struct ChokeDecision {
    pub peers_to_choke: HashSet<SocketAddr>,
    pub peers_to_unchoke: HashSet<SocketAddr>,
}

pub fn choke(
    interested: &HashSet<SocketAddr>,
    unchoked: &HashSet<SocketAddr>,
    transfer_rates: &HashMap<SocketAddr, TransferRate>,
    optimisitic: bool,
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

    if optimisitic {
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

        let interested = HashSet::from([peer1, peer2]);
        let unchoked = HashSet::new();
        let transfer_rate = HashMap::from([
            (peer1, TransferRate(Size::from_kibibytes(10), SEC)),
            (peer2, TransferRate(Size::from_kibibytes(10), SEC)),
        ]);
        let decision = choke(&interested, &unchoked, &transfer_rate, false);

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

        let interested = HashSet::from([peer1, peer2, peer3, peer4]);
        let unchoked = HashSet::new();
        let transfer_rate = HashMap::from([
            (peer1, TransferRate(Size::from_kibibytes(20), SEC)),
            (peer2, TransferRate(Size::from_kibibytes(10), SEC)),
            (peer3, TransferRate(Size::from_kibibytes(30), SEC)),
            (peer4, TransferRate(Size::from_kibibytes(40), SEC)),
            (peer5, TransferRate(Size::from_kibibytes(50), SEC)),
        ]);
        let decision = choke(&interested, &unchoked, &transfer_rate, false);

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
