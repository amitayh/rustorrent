use bit_set::BitSet;
use tokio::sync::mpsc::Sender;

use crate::peer::message::Message;
use crate::peer::transfer_rate::TransferRate;

#[derive(Debug)]
pub struct PeerState {
    pub tx: Sender<Message>,
    pub has_pieces: BitSet,
    pub client_to_peer: PeerToPeer,
    pub peer_to_client: PeerToPeer,
}

impl PeerState {
    pub fn new(tx: Sender<Message>) -> Self {
        Self {
            tx,
            has_pieces: BitSet::new(),
            client_to_peer: PeerToPeer::new(),
            peer_to_client: PeerToPeer::new(),
        }
    }
}

#[derive(Debug)]
pub struct PeerToPeer {
    pub transfer_rate: TransferRate,
    pub choking: bool,
    pub interested: bool,
}

impl PeerToPeer {
    fn new() -> Self {
        Self {
            transfer_rate: TransferRate::EMPTY,
            choking: true,
            interested: false,
        }
    }
}
