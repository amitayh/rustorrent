pub mod message;

use std::{
    collections::HashMap,
    net::{SocketAddr, TcpStream},
};

use rand::RngCore;

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

struct Peer {
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
}
