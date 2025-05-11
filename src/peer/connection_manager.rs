use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use log::warn;
use tokio::net::TcpStream;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinSet;

use crate::client::Download;
use crate::event::Event;
use crate::message::Message;
use crate::peer::connection::Connection;

pub struct ConnectionManager {
    peers: HashMap<SocketAddr, Connection>,
    download: Arc<Download>,
    events: Sender<Event>,
}

impl ConnectionManager {
    pub fn new(download: Arc<Download>, events: Sender<Event>) -> Self {
        Self {
            peers: HashMap::new(),
            download,
            events,
        }
    }

    pub fn start(&mut self, addr: SocketAddr, socket: Option<TcpStream>) {
        if self.peers.contains_key(&addr) {
            return;
        }
        let conn = Connection::spawn(
            addr,
            socket,
            self.events.clone(),
            Arc::clone(&self.download),
        );
        self.peers.insert(addr, conn);
    }

    pub fn remove(&mut self, addr: &SocketAddr) {
        let peer = self.peers.remove(addr).expect("invalid peer");
        peer.abort();
    }

    pub fn peer(&self, addr: &SocketAddr) -> &Connection {
        self.peers.get(addr).expect("invalid peer")
    }

    pub fn send(&self, addr: &SocketAddr, message: Message) {
        self.peer(addr).send(message);
    }

    pub fn broadcast(&self, message: Message) {
        for peer in self.peers.values() {
            peer.send(message.clone());
        }
    }

    pub async fn shutdown(mut self) {
        let mut join_set = JoinSet::new();
        for (_, peer) in self.peers.drain() {
            join_set.spawn(async move { peer.shutdown().await });
        }
        while let Some(result) = join_set.join_next().await {
            if let Err(err) = result {
                warn!("error encountered while shutting down: {:?}", err);
            }
        }
    }
}
