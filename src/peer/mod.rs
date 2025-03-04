pub mod message;

use rand::RngCore;

#[derive(PartialEq)]
pub struct PeerId(pub [u8; 20]);

impl PeerId {
    pub fn random() -> Self {
        let mut data = [0; 20];
        rand::rng().fill_bytes(&mut data);
        Self(data)
    }
}

/*
use crate::torrent::Torrent;

struct Peer;

struct Download {
    torrent: Torrent,
    peers: Vec<Peer>,
}

struct Client {
    downloads: Vec<Download>,
}

*/
