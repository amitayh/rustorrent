use std::fmt::Formatter;
use std::net::SocketAddr;
use std::sync::Arc;

use bit_set::BitSet;
use log::{trace, warn};
use tokio::net::TcpStream;
use tokio::time::Instant;

use crate::message::{Block, BlockData, Message};

use crate::peer::choke::Choker;
use crate::peer::event::Event;
use crate::peer::scheduler::Scheduler;
use crate::peer::stats::GlobalStats;
use crate::peer::sweeper::Sweeper;

use super::Download;
use super::scheduler::HaveResult;

pub struct EventHandler {
    choker: Choker,
    scheduler: Scheduler,
    sweeper: Sweeper,
    stats: GlobalStats,
    has_pieces: BitSet,
    download: Arc<Download>,
}

impl EventHandler {
    pub fn new(download: Arc<Download>, has_pieces: BitSet) -> Self {
        let stats = GlobalStats::new(download.torrent.info.pieces.len(), has_pieces.len());
        let scheduler = Scheduler::new(Arc::clone(&download), &has_pieces);
        Self {
            choker: Choker::new(download.config.optimistic_choking_cycle),
            scheduler,
            sweeper: Sweeper::new(
                download.config.idle_peer_timeout,
                download.config.block_timeout,
            ),
            stats,
            has_pieces,
            download,
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
                    actions.extend(self.handle(Event::Disconnect(addr)));
                }
                for (addr, block) in result.blocks {
                    warn!("block requet timed out: {} - {:?}", &addr, &block);
                    for next_block in self.scheduler.release(&addr, block) {
                        actions.push(Action::Send(addr, Message::Request(next_block)));
                    }
                }
                actions
            }

            Event::Message(addr, message) => {
                self.sweeper.update_peer_activity(addr, now);
                self.handle_message(addr, message)
            }

            Event::Stats(addr, stats) => {
                self.choker.update_peer_transfer_rate(addr, stats.download);
                self.stats.upload_rate += stats.upload;
                self.stats.download_rate += stats.download;
                Vec::new()
            }

            Event::PieceCompleted(piece) => {
                self.stats.completed_pieces += 1;
                self.has_pieces.insert(piece);
                self.stats.downloaded += self.download.torrent.info.piece_size(piece);
                let mut actions = vec![
                    Action::Broadcast(Message::Have(piece)),
                    Action::UpdateStats(self.stats.clone()),
                ];
                for not_interesting in self.scheduler.client_has_piece(piece) {
                    actions.push(Action::Send(not_interesting, Message::NotInterested));
                }
                actions
            }

            Event::PieceInvalid(piece) => {
                self.scheduler.invalidate(piece);
                Vec::new()
            }

            Event::Connect(addr) => {
                self.stats.connected_peers += 1;
                let mut actions = Vec::with_capacity(2);
                actions.push(Action::EstablishConnection(addr, None));
                if !self.has_pieces.is_empty() {
                    let pieces = self.has_pieces.clone();
                    actions.push(Action::Send(addr, Message::Bitfield(pieces)));
                }
                actions
            }

            Event::AcceptConnection(addr, socket) => {
                self.stats.connected_peers += 1;
                let mut actions = Vec::with_capacity(2);
                actions.push(Action::EstablishConnection(addr, Some(socket)));
                if !self.has_pieces.is_empty() {
                    let pieces = self.has_pieces.clone();
                    actions.push(Action::Send(addr, Message::Bitfield(pieces)));
                }
                actions
            }

            Event::Disconnect(addr) => {
                self.choker.peer_disconnected(&addr);
                self.scheduler.peer_disconnected(&addr);
                self.sweeper.peer_disconnected(&addr);
                self.stats.connected_peers -= 1;
                vec![Action::RemovePeer(addr)]
            }

            Event::Shutdown => vec![Action::Shutdown],
        };
        for action in &actions {
            trace!("action to perform: {:?}", &action);
            if let Action::Send(addr, Message::Request(block)) = action {
                self.sweeper.block_requested(*addr, *block, now);
            }
            if let Action::Upload(_, block) = action {
                self.stats.uploaded += self.download.torrent.info.piece_size(block.piece);
            }
        }
        actions
    }

    fn handle_message(&mut self, addr: SocketAddr, message: Message) -> Vec<Action> {
        match message {
            Message::KeepAlive => Vec::new(),

            Message::Choke => {
                self.scheduler.peer_choked(addr);
                Vec::new()
            }

            Message::Unchoke => self
                .scheduler
                .peer_unchoked(addr)
                .into_iter()
                .map(|block| Action::Send(addr, Message::Request(block)))
                .collect(),

            Message::Interested => {
                self.choker.peer_interested(addr);
                Vec::new()
            }

            Message::NotInterested => {
                self.choker.peer_not_interested(&addr);
                Vec::new()
            }

            Message::Have(piece) => match self.scheduler.peer_has_piece(addr, piece) {
                HaveResult::None => Vec::new(),
                HaveResult::Interested => vec![Action::Send(addr, Message::Interested)],
                HaveResult::InterestedAndRequest(blocks) => {
                    let mut actions = Vec::with_capacity(blocks.len() + 1);
                    actions.push(Action::Send(addr, Message::Interested));
                    for block in blocks {
                        actions.push(Action::Send(addr, Message::Request(block)));
                    }
                    actions
                }
                HaveResult::Request(blocks) => blocks
                    .into_iter()
                    .map(|block| Action::Send(addr, Message::Request(block)))
                    .collect(),
            },

            Message::Bitfield(pieces) => pieces
                .iter()
                .flat_map(|piece| self.handle_message(addr, Message::Have(piece)))
                .collect(),

            Message::Request(block) => {
                if !self.choker.is_unchoked(&addr) {
                    warn!("{} requested block while being choked", addr);
                    return Vec::new();
                }
                if !self.has_pieces.contains(block.piece) {
                    warn!("{} requested block which is not available", addr);
                    return Vec::new();
                }
                vec![Action::Upload(addr, block)]
            }

            Message::Piece(block_data) => {
                let block = Block::from(&block_data);
                if !self.scheduler.block_in_flight(&addr, &block) {
                    warn!("{} sent block {:?} which was not requested", &addr, &block);
                    return Vec::new();
                }
                self.sweeper.block_downloaded(addr, block);
                let mut actions = vec![Action::IntegrateBlock(block_data)];
                for next_block in self.scheduler.block_downloaded(&addr, &block) {
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
    /// Send a message to a specific peer
    Send(SocketAddr, Message),
    /// Send a message to all connected peers
    Broadcast(Message),
    Upload(SocketAddr, Block),
    IntegrateBlock(BlockData),
    RemovePeer(SocketAddr),
    UpdateStats(GlobalStats),
    Shutdown,
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EstablishConnection(addr, socket) => {
                write!(f, "EstablishConnection({:?}, {:?}", addr, socket)
            }
            Self::Send(addr, message) => write!(f, "Send({:?}, {:?})", addr, message),
            Self::Broadcast(message) => write!(f, "Broadcast({:?})", message),
            Self::Upload(addr, block) => write!(f, "Upload({:?}, {:?})", addr, block),
            Self::IntegrateBlock(block_data) => write!(
                f,
                "IntegrateBlock {{ piece: {}, offset: {}, data: <{} bytes> }}",
                block_data.piece,
                block_data.offset,
                block_data.data.len()
            ),
            Self::RemovePeer(addr) => write!(f, "RemovePeer({:?})", addr),
            Self::UpdateStats(stats) => write!(f, "UpdateStats({:?})", stats),
            Self::Shutdown => write!(f, "Shutdown"),
        }
    }
}

#[cfg(test)]
mod tests {
    use log::info;

    use crate::{message::BlockData, peer::tests::create_download};

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
        EventHandler::new(Arc::new(create_download()), BitSet::new())
    }
}
