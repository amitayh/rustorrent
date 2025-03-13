#![allow(dead_code)]
pub mod blocks;
pub mod choke;
pub mod config;
pub mod connection;
pub mod message;
pub mod peer_id;
pub mod piece_selector;
pub mod state;
pub mod transfer_rate;

use std::collections::{HashMap, HashSet};
use std::{io::Result, net::SocketAddr};

use bit_set::BitSet;
use log::{info, warn};
use size::{KiB, Size};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::time::{self, Instant};

use crate::peer::choke::choke;
use crate::peer::config::Config;
use crate::peer::connection::Connection;
use crate::peer::message::Handshake;
use crate::peer::message::Message;
use crate::peer::peer_id::PeerId;
use crate::peer::piece_selector::PieceSelector;
use crate::peer::state::PeerState;
use crate::peer::transfer_rate::TransferRate;
use crate::torrent::Info;

const BLOCK_SIZE: Size = Size::from_const(16 * KiB);

pub struct PeerEvent(SocketAddr, Event);

enum Event {
    Disconnected,
    Message(Message),
}

struct Peer {
    peer_id: PeerId,
    listener: TcpListener,
    torrent_info: Info,
    peers: HashMap<SocketAddr, PeerState>,
    interested: HashSet<SocketAddr>,
    unchoked: HashSet<SocketAddr>,
    transfer_rates: HashMap<SocketAddr, TransferRate>,
    piece_selector: PieceSelector,
    choke_tick: usize,
    tx: Sender<PeerEvent>,
    rx: Receiver<PeerEvent>,
    handshake: Handshake,
    has_pieces: BitSet,
    config: Config,
}

impl Peer {
    fn new(listener: TcpListener, torrent_info: Info, config: Config) -> Self {
        let peer_id = PeerId::random();
        let handshake = Handshake::new(torrent_info.info_hash.clone(), peer_id.clone());
        let total_pieces = torrent_info.pieces.len();
        let piece_selector = PieceSelector::new(
            torrent_info.piece_length,
            torrent_info.download_type.length(),
            BLOCK_SIZE,
        );
        let (tx, rx) = mpsc::channel(128);
        Self {
            peer_id,
            listener,
            torrent_info,
            peers: HashMap::new(),
            interested: HashSet::new(),
            unchoked: HashSet::new(),
            transfer_rates: HashMap::new(),
            piece_selector,
            choke_tick: 0,
            tx,
            rx,
            handshake,
            has_pieces: BitSet::with_capacity(total_pieces),
            config,
        }
    }

    fn temp_has_file(&mut self) {
        for piece in 0..self.torrent_info.pieces.len() {
            self.has_pieces.insert(piece);
        }
    }

    fn address(&self) -> Result<SocketAddr> {
        self.listener.local_addr()
    }

    async fn start(&mut self) -> anyhow::Result<()> {
        let start_choking = Instant::now() + self.config.choking_interval;
        let mut choke = time::interval_at(start_choking, self.config.choking_interval);
        loop {
            tokio::select! {
                _ = choke.tick() => {
                    self.choke().await?;
                }
                block = self.piece_selector.next() => {
                    info!("@@@ {:?}", block);
                }
                Some(PeerEvent(addr, event)) = self.rx.recv() => {
                    match event {
                        Event::Message(message) => {
                            self.handle_message(addr, message).await?;
                        }
                        Event::Disconnected => {
                            self.peer_disconnected(addr);
                        }
                    }
                }
                Ok((socket, addr)) = self.listener.accept() => {
                    info!("got new connection from {}", &addr);
                    self.new_peer(addr, socket, false).await?;
                }
            }
        }
    }

    async fn connect(&mut self, addr: SocketAddr) -> Result<()> {
        let socket = TcpStream::connect(addr).await?;
        self.new_peer(addr, socket, true).await
    }

    async fn new_peer(
        &mut self,
        addr: SocketAddr,
        socket: TcpStream,
        send_handshake_first: bool,
    ) -> Result<()> {
        let (rx, tx) = mpsc::channel(16);
        let mut connection = Connection::new(addr, socket, self.tx.clone(), tx);
        if send_handshake_first {
            connection.send(&self.handshake).await?;
            connection.wait_for_handshake().await?;
        } else {
            connection.wait_for_handshake().await?;
            connection.send(&self.handshake).await?;
        }
        if !self.has_pieces.is_empty() {
            let bitfield = Message::Bitfield(self.has_pieces.clone());
            connection.send(&bitfield).await?;
        }
        self.peers.insert(addr, PeerState::new(rx));
        let tx = self.tx.clone();
        tokio::spawn(async move {
            connection.start().await?;
            tx.send(PeerEvent(addr, Event::Disconnected)).await?;
            Ok::<(), anyhow::Error>(())
        });
        Ok(())
    }

    fn peer_disconnected(&mut self, addr: SocketAddr) {
        self.peers.remove(&addr);
        self.interested.remove(&addr);
        self.unchoked.remove(&addr);
        self.transfer_rates.remove(&addr);
        self.piece_selector.peer_disconnected(&addr);
    }

    async fn choke(&mut self) -> anyhow::Result<()> {
        self.choke_tick += 1;
        let optimistic = self.choke_tick % self.config.optimistic_choking_cycle == 0;
        let decision = choke(
            &self.interested,
            &self.unchoked,
            &self.transfer_rates,
            optimistic,
        );
        for addr in &decision.peers_to_choke {
            info!("choking {}", addr);
            let peer = self.peers.get_mut(addr).expect("invalid peer");
            peer.tx.send(Message::Choke).await?;
            self.unchoked.remove(addr);
        }
        for addr in &decision.peers_to_unchoke {
            if self.unchoked.insert(*addr) {
                info!("unchoking {}", addr);
                let peer = self.peers.get_mut(addr).expect("invalid peer");
                peer.tx.send(Message::Unchoke).await?;
            }
        }
        Ok(())
    }

    async fn handle_message(&mut self, addr: SocketAddr, message: Message) -> anyhow::Result<()> {
        match self.peers.get_mut(&addr) {
            Some(peer_state) => match message {
                Message::KeepAlive => (), // No-op
                Message::Choke => {
                    peer_state.peer_to_client.choking = true;
                }
                Message::Unchoke => {
                    peer_state.peer_to_client.choking = false;
                }
                Message::Interested => {
                    peer_state.peer_to_client.interested = true;
                    self.interested.insert(addr);
                }
                Message::NotInterested => {
                    peer_state.peer_to_client.interested = false;
                    self.interested.remove(&addr);
                }
                Message::Have { piece } => {
                    peer_state.has_pieces.insert(piece);
                }
                Message::Bitfield(pieces) => {
                    peer_state.has_pieces.union_with(&pieces);
                    self.piece_selector.peer_has_pieces(addr, &pieces);
                    let difference = peer_state.has_pieces.difference(&self.has_pieces);
                    if !peer_state.client_to_peer.interested
                        && peer_state.peer_to_client.choking
                        && difference.count() > 0
                    {
                        peer_state.tx.send(Message::Interested).await?;
                    }
                }
                Message::Request(block) => {
                    if peer_state.client_to_peer.choking {
                        warn!(
                            "peer {} requested block {:?} while being choked",
                            addr, block
                        );
                        return Ok(());
                    }
                    if self.has_pieces.contains(block.piece) {
                        warn!(
                            "peer {} requested piece {} which is not available",
                            addr, block.piece
                        );
                        return Ok(());
                    }
                }
                _ => todo!(),
            },
            None => panic!("invalid peer"),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use std::time::Duration;

    use tokio::{task::JoinSet, time::timeout};

    use super::*;

    #[tokio::test]
    async fn one_seeder_one_leecher() {
        env_logger::init();

        let torrent_info = Info::load("assets/alice_in_wonderland.txt").await.unwrap();

        let mut seeder = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let config = Config::default()
                .with_unchoking_interval(Duration::from_secs(1))
                .with_optimistic_unchoking_cycle(2);
            let mut peer = Peer::new(listener, torrent_info.clone(), config);
            peer.temp_has_file();
            peer
        };
        let seeder_addr = seeder.address().unwrap();
        let mut leecher = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            Peer::new(listener, torrent_info, Config::default())
        };

        let mut set = JoinSet::new();
        set.spawn(async move { seeder.start().await });
        set.spawn(async move {
            leecher.connect(seeder_addr).await.unwrap();
            leecher.start().await
        });

        let result = timeout(Duration::from_secs(20), set.join_all()).await;

        // assert leecher has file
        assert!(result.is_ok());
    }
}
