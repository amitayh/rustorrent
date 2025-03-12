#![allow(dead_code)]
use std::collections::HashMap;
use std::fmt::Debug;
use std::io::{Error, ErrorKind};
use std::{io::Result, net::SocketAddr};

use bit_set::BitSet;
use log::{info, warn};
use rand::seq::IteratorRandom;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::time;

use crate::peer::config::Config;
use crate::peer::message::Handshake;
use crate::peer::message::Message;
use crate::peer::peer_id::PeerId;
use crate::peer::transfer_rate::TransferRate;
use crate::{
    codec::{AsyncDecoder, AsyncEncoder},
    torrent::Info,
};

struct PeerMessage(SocketAddr, Message);

#[derive(Debug)]
struct PeerToPeer {
    transfer_rate: TransferRate,
    choking: bool,
    interested: bool,
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

#[derive(Debug)]
struct PeerState {
    tx: Sender<Message>,
    has_pieces: BitSet,
    client_to_peer: PeerToPeer,
    peer_to_client: PeerToPeer,
}

impl PeerState {
    fn new(tx: Sender<Message>) -> Self {
        Self {
            tx,
            has_pieces: BitSet::new(),
            client_to_peer: PeerToPeer::new(),
            peer_to_client: PeerToPeer::new(),
        }
    }

    fn eligible_for_unchoking(&self) -> bool {
        self.client_to_peer.choking && self.peer_to_client.interested
    }
}

struct Peer {
    peer_id: PeerId,
    listener: TcpListener,
    torrent_info: Info,
    peers: HashMap<SocketAddr, PeerState>,
    tx: Sender<PeerMessage>,
    rx: Receiver<PeerMessage>,
    handshake: Handshake,
    has_pieces: BitSet,
    config: Config,
}

impl Peer {
    fn new(listener: TcpListener, torrent_info: Info, config: Config) -> Self {
        let peer_id = PeerId::random();
        let handshake = Handshake::new(torrent_info.info_hash.clone(), peer_id.clone());
        let total_pieces = torrent_info.pieces.len();
        let (tx, rx) = mpsc::channel(128);
        Self {
            peer_id,
            listener,
            torrent_info,
            peers: HashMap::new(),
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
        let mut unchoke_best = time::interval(self.config.unchoking_interval);
        let mut unchoke_random = time::interval(self.config.optimistic_unchoking_interval);
        loop {
            tokio::select! {
                _ = unchoke_best.tick() => {
                    info!("running choking algorithm");
                    self.select_best_peers_to_unchoke();
                }
                _ = unchoke_random.tick() => {
                    info!("running optimistic choking algorithm");
                    self.select_random_peer_to_unchoke().await?;
                }
                Some(PeerMessage(addr, message)) = self.rx.recv() => {
                   self.handle_message(addr, message).await?;
                }
                Ok((socket, addr)) = self.listener.accept() => {
                    info!("got new connection from {}", &addr);
                    self.accept_connection(addr, socket).await?;
                }
            }
        }
    }

    async fn connect(&mut self, addr: SocketAddr) -> Result<()> {
        let socket = TcpStream::connect(addr).await?;
        self.new_peer(addr, socket, true).await
    }

    async fn accept_connection(&mut self, addr: SocketAddr, socket: TcpStream) -> Result<()> {
        self.new_peer(addr, socket, false).await
    }

    async fn new_peer(
        &mut self,
        addr: SocketAddr,
        socket: TcpStream,
        send_handshake_first: bool,
    ) -> Result<()> {
        let (rx, tx) = mpsc::channel(16);
        let mut handler = ConnectionHandler::new(addr, socket, self.tx.clone(), tx);
        if send_handshake_first {
            handler.send(&self.handshake).await?;
            handler.wait_for_handshake().await?;
        } else {
            handler.wait_for_handshake().await?;
            handler.send(&self.handshake).await?;
        }
        if !self.has_pieces.is_empty() {
            let bitfield = Message::Bitfield(self.has_pieces.clone());
            handler.send(&bitfield).await?;
        }
        self.peers.insert(addr, PeerState::new(rx));
        tokio::spawn(async move { handler.start().await });
        Ok(())
    }

    fn select_best_peers_to_unchoke(&self) {
        //
    }

    async fn select_random_peer_to_unchoke(&mut self) -> anyhow::Result<()> {
        let random_peer = {
            let mut rng = rand::rng();
            self.peers
                .iter_mut()
                .filter(|(_, peer)| peer.eligible_for_unchoking())
                .choose(&mut rng)
        };

        if let Some((addr, peer)) = random_peer {
            info!("unchoking peer {}", addr);
            peer.client_to_peer.choking = false;
            peer.tx.send(Message::Unchoke).await?;
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
                }
                Message::NotInterested => {
                    peer_state.peer_to_client.interested = false;
                }
                Message::Have { piece } => {
                    peer_state.has_pieces.insert(piece);
                }
                Message::Bitfield(pieces) => {
                    peer_state.has_pieces.union_with(&pieces);
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

struct ConnectionHandler {
    addr: SocketAddr,
    socket: TcpStream,
    tx: Sender<PeerMessage>,
    rx: Receiver<Message>,
}

impl ConnectionHandler {
    fn new(
        addr: SocketAddr,
        socket: TcpStream,
        tx: Sender<PeerMessage>,
        rx: Receiver<Message>,
    ) -> Self {
        Self {
            addr,
            socket,
            tx,
            rx,
        }
    }

    pub async fn wait_for_handshake(&mut self) -> Result<()> {
        let handshake = Handshake::decode(&mut self.socket).await?;
        info!("< got handshake {:?}", handshake);
        Ok(())
    }

    pub async fn send<T: AsyncEncoder + Debug>(&mut self, message: &T) -> Result<()> {
        message.encode(&mut self.socket).await?;
        info!("> sent message: {:?}", message);
        Ok(())
    }

    pub async fn start(&mut self) -> Result<()> {
        loop {
            tokio::select! {
                Some(message) = self.rx.recv() => {
                    self.send(&message).await?;
                }
                message = Message::decode(&mut self.socket) => {
                    match message {
                        Ok(message) => {
                            info!("< got message: {:?}", &message);
                            self.tx
                                .send(PeerMessage(self.addr, message))
                                .await
                                .map_err(|err| Error::new(ErrorKind::Other, err))?;
                        }
                        Err(err) => {
                            warn!("failed to decode message: {}", err);
                            if err.kind() == ErrorKind::UnexpectedEof {
                                // Disconnected
                                break;
                            }
                        }
                    }
                }
            }
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
                .with_optimistic_unchoking_interval(Duration::from_secs(2));
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
