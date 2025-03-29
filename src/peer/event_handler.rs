use std::fmt::Formatter;
use std::net::SocketAddr;

use bit_set::BitSet;
use log::{trace, warn};
use tokio::net::TcpStream;
use tokio::time::Instant;

use crate::message::{Block, BlockData, Message};

use crate::peer::Config;
use crate::peer::event::Event;
use crate::peer::sizes::Sizes;
use crate::peer::stats::Stats;
use crate::peer::sweeper::Sweeper;
use crate::peer::{choke::Choker, piece::Allocator};
use crate::torrent::Info;

pub struct EventHandler {
    choker: Choker,
    allocator: Allocator,
    sweeper: Sweeper,
    stats: Stats,
}

impl EventHandler {
    pub fn new(torrent_info: Info, config: Config, has_pieces: BitSet) -> Self {
        let sizes = Sizes::new(
            torrent_info.piece_length,
            torrent_info.download_type.length(),
            config.block_size,
        );
        let allocator = Allocator::new(sizes, has_pieces);
        Self {
            choker: Choker::new(config.optimistic_choking_cycle),
            allocator,
            sweeper: Sweeper::new(config.idle_peer_timeout, config.block_timeout),
            stats: Stats::default(),
        }
    }

    pub fn handle(&mut self, event: Event) -> Vec<Action> {
        trace!("handling event: {:?}", &event);
        let now = Instant::now();
        let actions = match event {
            Event::KeepAliveTick => vec![Action::Broadcast(Message::KeepAlive)],
            Event::ChokeTick => {
                let decision = self.choker.run();
                let choke = decision
                    .peers_to_choke
                    .into_iter()
                    .map(|addr| Action::Send(addr, Message::Choke));
                let unchoke = decision
                    .peers_to_unchoke
                    .into_iter()
                    .map(|addr| Action::Send(addr, Message::Unchoke));
                choke.chain(unchoke).collect()
            }
            Event::SweepTick(instant) => {
                let result = self.sweeper.sweep(instant);
                let mut actions = Vec::with_capacity(result.peers.len());
                for addr in result.peers {
                    warn!("peer {} has been idle for too long", &addr);
                    self.choker.peer_disconnected(&addr);
                    self.allocator.peer_disconnected(&addr);
                    actions.push(Action::RemovePeer(addr));
                }
                for (addr, block) in result.blocks {
                    warn!("block requet timed out: {} - {:?}", &addr, &block);
                    if let Some(next_block) = self.allocator.release(&addr, block) {
                        actions.push(Action::Send(addr, Message::Request(next_block)));
                    }
                }
                actions
            }
            Event::Message(addr, message) => {
                self.sweeper.update(addr, now);
                self.handle_message(addr, message)
            }
            Event::Stats(addr, stats) => {
                self.choker.update_peer_transfer_rate(addr, stats.download);
                self.stats += stats;
                Vec::new()
            }
            Event::PieceCompleted(piece) => {
                let mut actions = vec![Action::Broadcast(Message::Have(piece))];
                for not_interesting in self.allocator.client_has_piece(piece) {
                    actions.push(Action::Send(not_interesting, Message::NotInterested));
                }
                // TODO: remove this
                if self.allocator.is_complete() {
                    actions.push(Action::Shutdown);
                }
                actions
            }
            Event::PieceInvalid(piece) => {
                self.allocator.invalidate(piece);
                Vec::new()
            }
            Event::Connect(addr) => {
                let mut actions = Vec::with_capacity(2);
                actions.push(Action::EstablishConnection(addr, None));
                if !self.allocator.has_pieces.is_empty() {
                    let pieces = self.allocator.has_pieces.clone();
                    actions.push(Action::Send(addr, Message::Bitfield(pieces)));
                }
                actions
            }
            Event::AcceptConnection(addr, socket) => {
                let pieces = self.allocator.has_pieces.clone();
                vec![
                    Action::EstablishConnection(addr, Some(socket)),
                    Action::Send(addr, Message::Bitfield(pieces)),
                ]
            }
            Event::Disconnect(addr) => {
                self.choker.peer_disconnected(&addr);
                self.allocator.peer_disconnected(&addr);
                self.sweeper.peer_disconnected(&addr);
                vec![Action::RemovePeer(addr)]
            }
            Event::Shutdown => vec![Action::Shutdown],
        };
        for action in &actions {
            trace!("action to perform: {:?}", &action);
            if let Action::Send(addr, Message::Request(block)) = action {
                self.sweeper.block_requested(*addr, *block, now);
            }
        }
        actions
    }

    fn handle_message(&mut self, addr: SocketAddr, message: Message) -> Vec<Action> {
        match message {
            Message::KeepAlive => Vec::new(),

            Message::Choke => {
                self.allocator.peer_choked(addr);
                Vec::new()
            }

            Message::Unchoke => match self.allocator.peer_unchoked(addr) {
                Some(block) => vec![Action::Send(addr, Message::Request(block))],
                None => Vec::new(),
            },

            Message::Interested => {
                self.choker.peer_interested(addr);
                Vec::new()
            }

            Message::NotInterested => {
                self.choker.peer_not_interested(&addr);
                Vec::new()
            }

            Message::Have(piece) => {
                let pieces = BitSet::from_iter([piece]);
                self.handle_message(addr, Message::Bitfield(pieces))
            }

            Message::Bitfield(pieces) => {
                let mut actions = Vec::with_capacity(2);
                let (became_interesting, next_block) =
                    self.allocator.peer_has_pieces(addr, &pieces);
                if became_interesting {
                    actions.push(Action::Send(addr, Message::Interested));
                }
                if let Some(block) = next_block {
                    actions.push(Action::Send(addr, Message::Request(block)));
                }
                actions
            }

            Message::Request(block) => {
                if !self.choker.is_unchoked(&addr) {
                    warn!("{} requested block while being choked", addr);
                    return Vec::new();
                }
                if !self.allocator.is_available(block.piece) {
                    warn!("{} requested block which is not available", addr);
                    return Vec::new();
                }
                vec![Action::Upload(addr, block)]
            }

            Message::Piece(block_data) => {
                let block = Block::from(&block_data);
                if !self.allocator.block_in_flight(&addr, &block) {
                    warn!("{} sent block {:?} which was not requested", &addr, &block);
                    return Vec::new();
                }
                self.sweeper.block_downloaded(addr, block);
                let mut actions = vec![Action::IntegrateBlock(block_data)];
                if let Some(next_block) = self.allocator.block_downloaded(&addr, &block) {
                    actions.push(Action::Send(addr, Message::Request(next_block)));
                }
                actions
            }

            _ => {
                warn!("unhandled message: {:?}", message);
                Vec::new()
            }
        }
    }
}

pub enum Action {
    EstablishConnection(SocketAddr, Option<TcpStream>),
    Send(SocketAddr, Message),
    /// Send a message to all connected peers
    Broadcast(Message),
    /// Send a message to a specific peer
    Upload(SocketAddr, Block),
    IntegrateBlock(BlockData),
    RemovePeer(SocketAddr),
    Shutdown,
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::EstablishConnection(addr, socket) => {
                write!(f, "EstablishConnection({:?}, {:?}", addr, socket)
            }
            Action::Send(addr, message) => write!(f, "Send({:?}, {:?})", addr, message),
            Action::Broadcast(message) => write!(f, "Broadcast({:?})", message),
            Action::Upload(addr, block) => write!(f, "Upload({:?}, {:?})", addr, block),
            Action::IntegrateBlock(block_data) => write!(
                f,
                "IntegrateBlock {{ piece: {}, offset: {}, data: <{} bytes> }}",
                block_data.piece,
                block_data.offset,
                block_data.data.len()
            ),
            Action::RemovePeer(addr) => write!(f, "RemovePeer({:?})", addr),
            Action::Shutdown => write!(f, "Shutdown"),
        }
    }
}

#[cfg(test)]
mod tests {
    use log::info;
    use size::Size;

    use crate::{
        crypto::{Md5, Sha1},
        message::BlockData,
        torrent::DownloadType,
    };

    use super::*;

    #[test]
    fn keep_alive() {
        let mut event_handler = create_event_handler();

        let actions = event_handler.handle(Event::KeepAliveTick);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Broadcast(Message::KeepAlive)));
    }

    #[test]
    fn sequence() {
        env_logger::init();

        let mut event_handler = create_event_handler();

        let addr = "127.0.0.1:6881".parse().unwrap();

        run(
            &mut event_handler,
            vec![
                Event::Message(addr, Message::Have(0)),
                Event::Message(addr, Message::Unchoke),
                Event::Message(
                    addr,
                    Message::Piece(BlockData {
                        piece: 0,
                        offset: 0,
                        data: vec![0; 16384],
                    }),
                ),
                Event::Message(
                    addr,
                    Message::Piece(BlockData {
                        piece: 0,
                        offset: 16384,
                        data: vec![1; 16384],
                    }),
                ),
            ],
        );
    }

    fn run(event_handler: &mut EventHandler, events: Vec<Event>) {
        for event in events {
            info!(target: "<<<", "{:?}", &event);
            for _action in event_handler.handle(event) {
                //info!(target: ">>>", "{:?}", &action);
            }
        }
    }

    fn create_event_handler() -> EventHandler {
        let torrent = Info {
            info_hash: Sha1::from_hex("e90cf5ec83e174d7dcb94821560dac201ae1f663").unwrap(),
            piece_length: Size::from_kibibytes(32),
            pieces: vec![
                Sha1::from_hex("a9af20024fc50543163b6be66fe4660be2170f6c").unwrap(),
                Sha1::from_hex("2494039151d7db3e56b3ec021d233742e3de55a6").unwrap(),
                Sha1::from_hex("af99be061f2c5eee12374055cf1a81909d276db5").unwrap(),
                Sha1::from_hex("3c12e1fcba504fedc13ee17ea76b62901dc8c9f7").unwrap(),
                Sha1::from_hex("d5facb89cbdc2e3ed1a1cd1050e217ec534f1fad").unwrap(),
                Sha1::from_hex("d5d2b296f52ab11791aad35a7d493833d39c6786").unwrap(),
            ],
            download_type: DownloadType::SingleFile {
                name: "alice_in_wonderland.txt".to_string(),
                length: Size::from_bytes(174357),
                md5sum: Some(Md5::from_hex("9a930de3cfc64468c05715237a6b4061").unwrap()),
            },
        };

        EventHandler::new(torrent, Config::default(), BitSet::new())
    }
}
