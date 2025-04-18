use std::net::SocketAddr;
use std::sync::Arc;

use bit_set::BitSet;
use log::warn;
use tokio::net::TcpStream;
use tokio::time::Instant;

use crate::client::Download;
use crate::command::Command;
use crate::event::Event;
use crate::message::{Block, Message};
use crate::peer::choke::Choker;
use crate::peer::stats::GlobalStats;
use crate::peer::sweeper::Sweeper;
use crate::scheduler::PeerPieceResponse;
use crate::scheduler::Scheduler;

/// Handles events from peers and maintains the state of the download.
///
/// The `EventHandler` is responsible for:
/// - Managing peer choking/unchoking decisions through the `Choker`
/// - Scheduling piece downloads and tracking piece availability via the `Scheduler`
/// - Handling timeouts for idle peers and stalled block requests with the `Sweeper`
/// - Tracking global download statistics
/// - Maintaining the set of pieces this peer has
pub struct EventHandler {
    /// Manages peer choking/unchoking decisions
    choker: Choker,

    /// Schedules piece downloads and tracks piece availability
    scheduler: Scheduler,

    /// Handles timeouts for idle peers and stalled block requests
    sweeper: Sweeper,

    /// Tracks global download statistics
    stats: GlobalStats,

    /// Set of pieces that this peer has
    has_pieces: BitSet,

    /// Download metadata and configuration
    download: Arc<Download>,
}

impl EventHandler {
    pub fn new(download: Arc<Download>, has_pieces: BitSet) -> Self {
        let config = &download.config;
        let stats = GlobalStats::new(download.torrent.info.pieces.len(), has_pieces.len());
        let choker = Choker::new(config.optimistic_choking_cycle);
        let scheduler = Scheduler::new(Arc::clone(&download), &has_pieces);
        let sweeper = Sweeper::new(config.idle_peer_timeout, config.block_timeout);
        Self {
            choker,
            scheduler,
            sweeper,
            stats,
            has_pieces,
            download,
        }
    }

    pub fn handle(&mut self, event: Event) -> Vec<Command> {
        let now = Instant::now();
        match event {
            Event::KeepAliveTicked => vec![Command::Broadcast(Message::KeepAlive)],
            Event::StatsTicked => vec![Command::UpdateStats(self.stats.clone())],

            Event::ChokeTicked => {
                let decision = self.choker.run();
                let choke = decision
                    .peers_to_choke
                    .into_iter()
                    .map(|addr| Command::Send(addr, Message::Choke));
                let unchoke = decision
                    .peers_to_unchoke
                    .into_iter()
                    .map(|addr| Command::Send(addr, Message::Unchoke));
                choke.chain(unchoke).collect()
            }

            Event::SweepTicked(instant) => {
                let result = self.sweeper.sweep(instant);
                let mut commands = Vec::with_capacity(result.peers.len());
                for addr in result.peers {
                    warn!("peer {} has been idle for too long", &addr);
                    commands.push(self.disconnect(addr));
                }
                for (addr, block) in result.blocks {
                    warn!("block requet timed out: {} - {:?}", &addr, &block);
                    for next_block in self.scheduler.release(&addr, block) {
                        commands.push(self.request(addr, next_block, now));
                    }
                }
                commands
            }

            Event::MessageReceived(addr, message) => {
                self.sweeper.update_peer_activity(addr, now);
                self.handle_message(addr, message, now)
            }

            Event::StatsUpdated(addr, stats) => {
                self.choker.update_peer_transfer_rate(addr, stats.download);
                self.stats.upload_rate += stats.upload;
                self.stats.download_rate += stats.download;
                Vec::new()
            }

            Event::PieceCompleted(piece) => {
                self.stats.completed_pieces += 1;
                self.has_pieces.insert(piece);
                self.stats.downloaded += self.download.torrent.info.piece_size(piece);
                let mut commands = vec![Command::Broadcast(Message::Have(piece))];
                for not_interesting in self.scheduler.client_has_piece(piece) {
                    commands.push(Command::Send(not_interesting, Message::NotInterested));
                }
                commands
            }

            Event::PieceVerificationFailed(piece) => {
                self.scheduler.invalidate(piece);
                Vec::new()
            }

            Event::ConnectionRequested(addr) => self.establish_connection(addr, None),
            Event::ConnectionAccepted(addr, socket) => {
                self.establish_connection(addr, Some(socket))
            }
            Event::Disconnected(addr) => vec![self.disconnect(addr)],
            Event::ShutdownRequested => vec![Command::Shutdown],
        }
    }

    fn establish_connection(
        &mut self,
        addr: SocketAddr,
        socket: Option<TcpStream>,
    ) -> Vec<Command> {
        self.stats.connected_peers += 1;
        let mut commands = Vec::with_capacity(2);
        commands.push(Command::EstablishConnection(addr, socket));
        if !self.has_pieces.is_empty() {
            let pieces = self.has_pieces.clone();
            commands.push(Command::Send(addr, Message::Bitfield(pieces)));
        }
        commands
    }

    fn request(&mut self, addr: SocketAddr, block: Block, now: Instant) -> Command {
        self.sweeper.block_requested(addr, block, now);
        Command::Send(addr, Message::Request(block))
    }

    fn disconnect(&mut self, addr: SocketAddr) -> Command {
        self.choker.peer_disconnected(&addr);
        self.scheduler.peer_disconnected(&addr);
        self.sweeper.peer_disconnected(&addr);
        self.stats.connected_peers -= 1;
        Command::RemovePeer(addr)
    }

    fn have(&mut self, addr: SocketAddr, piece: usize, now: Instant) -> Vec<Command> {
        match self.scheduler.peer_has_piece(addr, piece) {
            PeerPieceResponse::NoAction => Vec::new(),
            PeerPieceResponse::ExpressInterest => vec![Command::Send(addr, Message::Interested)],
            PeerPieceResponse::ExpressInterestAndRequest(blocks) => {
                let mut commands = Vec::with_capacity(blocks.len() + 1);
                commands.push(Command::Send(addr, Message::Interested));
                for block in blocks {
                    commands.push(self.request(addr, block, now));
                }
                commands
            }
            PeerPieceResponse::RequestBlocks(blocks) => blocks
                .into_iter()
                .map(|block| self.request(addr, block, now))
                .collect(),
        }
    }

    fn handle_message(&mut self, addr: SocketAddr, message: Message, now: Instant) -> Vec<Command> {
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
                .map(|block| self.request(addr, block, now))
                .collect(),

            Message::Interested => {
                self.choker.peer_interested(addr);
                Vec::new()
            }

            Message::NotInterested => {
                self.choker.peer_not_interested(&addr);
                Vec::new()
            }

            Message::Have(piece) => self.have(addr, piece, now),

            Message::Bitfield(pieces) => pieces
                .iter()
                .flat_map(|piece| self.have(addr, piece, now))
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
                self.stats.uploaded += self.download.torrent.info.piece_size(block.piece);
                vec![Command::Upload(addr, block)]
            }

            Message::Piece(block_data) => {
                let block = Block::from(&block_data);
                if !self.scheduler.block_in_flight(&addr, &block) {
                    warn!("{} sent block {:?} which was not requested", &addr, &block);
                    return Vec::new();
                }
                self.sweeper.block_downloaded(addr, block);
                let mut commands = vec![Command::IntegrateBlock(block_data)];
                for next_block in self.scheduler.block_downloaded(&addr, &block) {
                    commands.push(self.request(addr, next_block, now));
                }
                commands
            }

            _ => {
                warn!("unhandled message: {:?}", message);
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{client::tests::create_download, message::BlockData};

    use super::*;

    #[test]
    fn keep_alive() {
        let mut event_handler = create_event_handler();

        assert_eq!(
            event_handler.handle(Event::KeepAliveTicked),
            vec![Command::Broadcast(Message::KeepAlive)]
        );
    }

    #[test]
    fn sequence() {
        let _ = env_logger::try_init();

        let mut handler = create_event_handler();

        let addr = "127.0.0.1:6881".parse().unwrap();

        assert_eq!(
            handler.handle(Event::ConnectionRequested(addr)),
            vec![Command::EstablishConnection(addr, None)]
        );

        assert_eq!(
            handler.handle(Event::MessageReceived(addr, Message::Have(0))),
            vec![Command::Send(addr, Message::Interested)]
        );

        assert_eq!(
            handler.handle(Event::MessageReceived(addr, Message::Unchoke)),
            vec![
                Command::Send(addr, Message::Request(Block::new(0, 0, 16384))),
                Command::Send(addr, Message::Request(Block::new(0, 16384, 16384))),
            ]
        );

        let block_data = BlockData {
            piece: 0,
            offset: 0,
            data: vec![0; 16384],
        };
        assert_eq!(
            handler.handle(Event::MessageReceived(
                addr,
                Message::Piece(block_data.clone())
            )),
            vec![Command::IntegrateBlock(block_data)]
        );

        #[rustfmt::skip]
        assert_eq!(
            handler.handle(Event::MessageReceived(addr, Message::Have(5))),
            vec![Command::Send(addr, Message::Request(Block::new(5, 0, 10517)))]
        );

        assert_eq!(
            handler.handle(Event::Disconnected(addr)),
            vec![Command::RemovePeer(addr)]
        );
    }

    fn create_event_handler() -> EventHandler {
        EventHandler::new(Arc::new(create_download()), BitSet::new())
    }
}
