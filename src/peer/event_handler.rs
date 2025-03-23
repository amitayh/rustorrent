use std::net::SocketAddr;

use bit_set::BitSet;
use log::warn;
use tokio::net::TcpStream;

use crate::message::{Block, Message};

use crate::peer::Config;
use crate::peer::event::Event;
use crate::peer::piece::Status;
use crate::peer::sizes::Sizes;
use crate::peer::{
    choke::Choker,
    piece::{Distributor, Joiner},
};
use crate::torrent::Info;

pub struct EventHandler {
    choker: Choker,
    distributor: Distributor,
    joiner: Joiner,
}

impl EventHandler {
    pub fn new(torrent_info: Info, config: Config, has_pieces: BitSet) -> Self {
        let sizes = Sizes::new(
            torrent_info.piece_length,
            torrent_info.download_type.length(),
            config.block_size,
        );
        let distributor = Distributor::new(sizes.clone(), has_pieces);
        Self {
            choker: Choker::new(config.optimistic_choking_cycle),
            distributor,
            joiner: Joiner::new(&sizes, torrent_info.pieces.clone()),
        }
    }

    pub fn handle(&mut self, event: Event) -> Vec<Action> {
        match event {
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
            Event::Message(addr, message) => self.handle_message(addr, message),
            Event::BlockTimeout(addr, block) => {
                warn!("block requet timed out: {} - {:?}", &addr, &block);
                match self.distributor.release(&addr, block) {
                    Some(next_block) => vec![Action::Send(addr, Message::Request(next_block))],
                    None => Vec::new(),
                }
            }
            Event::Connect(addr) => {
                let mut actions = Vec::with_capacity(2);
                actions.push(Action::EstablishConnection(addr, None));
                if !self.distributor.has_pieces.is_empty() {
                    let pieces = self.distributor.has_pieces.clone();
                    actions.push(Action::Send(addr, Message::Bitfield(pieces)));
                }
                actions
            }
            Event::AcceptConnection(addr, socket) => {
                let pieces = self.distributor.has_pieces.clone();
                vec![
                    Action::EstablishConnection(addr, Some(socket)),
                    Action::Send(addr, Message::Bitfield(pieces)),
                ]
            }
            Event::Disconnect(addr) => {
                self.choker.peer_disconnected(&addr);
                self.distributor.peer_disconnected(&addr);
                vec![Action::RemovePeer(addr)]
            }
        }
    }

    fn handle_message(&mut self, addr: SocketAddr, message: Message) -> Vec<Action> {
        match message {
            Message::KeepAlive => Vec::new(),

            Message::Choke => {
                self.distributor.peer_choked(addr);
                Vec::new()
            }

            Message::Unchoke => match self.distributor.peer_unchoked(addr) {
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
                    self.distributor.peer_has_pieces(addr, &pieces);
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
                if !self.distributor.is_available(block.piece) {
                    warn!("{} requested block which is not available", addr);
                    return Vec::new();
                }
                vec![Action::Upload(addr, block)]
            }

            Message::Piece(block_data) => {
                let block = (&block_data).into();
                if !self.distributor.block_in_flight(&addr, &block) {
                    warn!("{} sent block {:?} which was not requested", &addr, &block);
                    return Vec::new();
                }
                let mut actions = Vec::with_capacity(4);
                if let Some(next_block) = self.distributor.block_downloaded(&addr, &block) {
                    actions.push(Action::Send(addr, Message::Request(next_block)));
                }
                let piece = block_data.piece;
                match self.joiner.add(piece, block_data.offset, block_data.data) {
                    Status::Incomplete => (), // Noting to do, wait for next block
                    Status::Invalid => {
                        warn!("piece {} sha1 mismatch", piece);
                        self.distributor.invalidate(piece);
                        return Vec::new();
                    }
                    Status::Complete {
                        offset: piece_offset,
                        data: piece_data,
                    } => {
                        actions.push(Action::Broadcast(Message::Have(piece)));
                        for not_interesting in self.distributor.client_has_piece(piece) {
                            actions.push(Action::Send(not_interesting, Message::NotInterested));
                        }
                        actions.push(Action::IntegratePiece {
                            offset: piece_offset,
                            data: piece_data,
                        });
                    }
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

#[derive(Debug)]
pub enum Action {
    EstablishConnection(SocketAddr, Option<TcpStream>),
    Send(SocketAddr, Message),
    Broadcast(Message),
    Upload(SocketAddr, Block),
    IntegratePiece { offset: u64, data: Vec<u8> },
    RemovePeer(SocketAddr),
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

    //#[test]
    //fn keep_alive() {
    //    let mut event_handler = create_event_handler();

    //    assert_eq!(
    //        event_handler.handle(Event::KeepAliveTick),
    //        vec![Action::Broadcast(Message::KeepAlive)]
    //    );
    //}

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
            for action in event_handler.handle(event) {
                info!(target: ">>>", "{:?}", &action);
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
