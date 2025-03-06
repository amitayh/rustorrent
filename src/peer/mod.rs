pub mod message;

use std::collections::HashSet;
use std::io::Result;
use std::{collections::HashMap, net::SocketAddr};

use log::info;
use message::Handshake;
use rand::RngCore;
use tokio::net::TcpStream;

use crate::codec::{AsyncDecoder, AsyncEncoder};
use crate::peer::message::Message;
use crate::torrent::Torrent;

#[derive(PartialEq, Clone)]
pub struct PeerId(pub [u8; 20]);

impl PeerId {
    pub fn random() -> Self {
        let mut data = [0; 20];
        rand::rng().fill_bytes(&mut data);
        Self(data)
    }
}

struct Download {
    torrent: Torrent,
    peers: HashMap<SocketAddr, Peer>,
}

impl Download {
    pub fn handle(&mut self, socket: TcpStream, message: Message) {
        match message {
            _ => (),
        }
    }
}

pub struct Peer {
    socket: TcpStream,
    am_choking: bool,
    am_interested: bool,
    peer_choking: bool,
    peer_interested: bool,
}

impl Peer {
    pub fn new(socket: TcpStream) -> Self {
        Self {
            socket,
            am_choking: true,
            am_interested: false,
            peer_choking: true,
            peer_interested: false,
        }
    }

    pub async fn wait_for_handshake(&mut self) -> Result<()> {
        let handshake = Handshake::decode(&mut self.socket).await?;
        info!("< got handshake {:?}", handshake);
        Ok(())
    }

    pub async fn send_handshake(&mut self, handshake: &Handshake) -> Result<()> {
        handshake.encode(&mut self.socket).await?;
        info!("> sent handshake");
        Ok(())
    }

    pub async fn handle(&mut self) -> Result<()> {
        loop {
            let message = Message::decode(&mut self.socket).await?;
            info!("< got message {:?}", message);
        }
    }
}

struct PeerSet {
    peer_to_piece: HashMap<SocketAddr, PieceInfo>,
    piece_to_peer: HashMap<usize, HashSet<SocketAddr>>,
}

impl PeerSet {
    fn new() -> Self {
        PeerSet {
            peer_to_piece: HashMap::new(),
            piece_to_peer: HashMap::new(),
        }
    }

    fn have(&mut self, peer: SocketAddr, piece: usize) {
        //
    }
}

struct PieceInfo {
    downloading: Option<SocketAddr>,
    have: HashSet<SocketAddr>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn have() {
        let mut peer_set = PeerSet::new();
        let peer = "0.0.0.1:6881".parse().unwrap();
        peer_set.have(peer, 1234);
    }
}
