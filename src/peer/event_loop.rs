#![allow(dead_code)]

use std::{collections::HashSet, net::SocketAddr};

use bit_set::BitSet;
use log::warn;

use crate::message::{Block, Message};

use crate::peer::Config;
use crate::peer::piece::Status;
use crate::peer::sizes::Sizes;
use crate::peer::{
    choke::Choker,
    piece::{Distributor, Joiner},
};
use crate::torrent::Info;

pub struct EventLoop {
    choker: Choker,
    distributor: Distributor,
    joiner: Joiner,
    am_interested: HashSet<SocketAddr>,
}

impl EventLoop {
    pub fn new(torrent_info: Info, config: Config, has_pieces: BitSet) -> Self {
        let sizes = Sizes::new(
            torrent_info.piece_length,
            torrent_info.download_type.length(),
            config.block_size,
        );
        let mut distributor = Distributor::new(&sizes);
        distributor.client_has_pieces(&has_pieces);
        Self {
            choker: Choker::new(config.optimistic_choking_cycle),
            distributor,
            joiner: Joiner::new(&sizes, torrent_info.pieces.clone()),
            am_interested: HashSet::new(),
        }
    }

    pub fn handle(&mut self, event: Event) -> Actions {
        match event {
            Event::KeepAliveTick => Action::Broadcast(Message::KeepAlive).into(),
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
                Actions(choke.chain(unchoke).collect())
            }
            Event::Message(addr, message) => self.handle_message(addr, message),
            Event::Connect(addr) => {
                let pieces = self.distributor.has_pieces.clone();
                Action::Send(addr, Message::Bitfield(pieces)).into()
            }
            Event::Disconnect(addr) => {
                self.choker.peer_disconnected(&addr);
                self.distributor.peer_disconnected(&addr);
                Actions::none()
            }
        }
    }

    fn handle_message(&mut self, addr: SocketAddr, message: Message) -> Actions {
        match message {
            Message::KeepAlive => Actions::none(),

            Message::Choke => {
                self.distributor.peer_choked(addr);
                Actions::none()
            }

            Message::Unchoke => match self.distributor.peer_unchoked(addr) {
                Some(block) => Action::Send(addr, Message::Request(block)).into(),
                None => Actions::none(),
            },

            Message::Interested => {
                self.choker.peer_interested(addr);
                Actions::none()
            }

            Message::NotInterested => {
                self.choker.peer_not_interested(&addr);
                Actions::none()
            }

            Message::Have { piece } => {
                let pieces = BitSet::from_iter([piece]);
                self.handle_message(addr, Message::Bitfield(pieces))
            }

            Message::Bitfield(pieces) => {
                let mut actions = Vec::new();
                if self.am_interested.insert(addr) {
                    // TODO: check if peer has a piece we actually need
                    actions.push(Action::Send(addr, Message::Interested));
                }
                if let Some(block) = self.distributor.peer_has_pieces(addr, &pieces) {
                    actions.push(Action::Send(addr, Message::Request(block)));
                }
                Actions(actions)
            }

            Message::Request(block) => {
                if !self.choker.is_unchoked(&addr) {
                    warn!("{} requested block while being choked", addr);
                    return Actions::none();
                }
                if !self.distributor.is_available(block.piece) {
                    warn!("{} requested block which is not available", addr);
                    return Actions::none();
                }
                Action::Upload(addr, block).into()
            }

            Message::Piece {
                piece,
                offset: block_offset,
                data: block_data,
            } => {
                let mut actions = Vec::new();
                let block = Block::new(piece, block_offset, block_data.len());
                if let Some(next_block) = self.distributor.block_downloaded(&addr, &block) {
                    actions.push(Action::Send(addr, Message::Request(next_block)));
                }
                match self.joiner.add(piece, block_offset, block_data) {
                    Status::Incomplete => (), // Noting to do, wait for next block
                    Status::Invalid => {
                        warn!("piece {} sha1 mismatch", piece);
                        self.distributor.invalidate(piece);
                    }
                    Status::Complete {
                        offset: piece_offset,
                        data: piece_data,
                    } => {
                        actions.push(Action::Broadcast(Message::Have { piece }));
                        for not_interesting in self.distributor.client_has_piece(piece) {
                            actions.push(Action::Send(not_interesting, Message::NotInterested));
                        }
                        actions.push(Action::IntegratePiece {
                            offset: piece_offset,
                            data: piece_data,
                        });
                    }
                }
                Actions(actions)
            }

            _ => {
                warn!("unhandled message: {:?}", message);
                Actions::none()
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Event {
    KeepAliveTick,
    ChokeTick,
    Message(SocketAddr, Message),
    Connect(SocketAddr),
    Disconnect(SocketAddr),
}

#[derive(Debug, PartialEq)]
pub enum Action {
    Send(SocketAddr, Message),
    Broadcast(Message),
    Upload(SocketAddr, Block),
    IntegratePiece { offset: u64, data: Vec<u8> },
}

#[derive(Debug, PartialEq)]
pub struct Actions(pub Vec<Action>);

impl Actions {
    fn none() -> Self {
        Self(vec![])
    }
}

impl From<Action> for Actions {
    fn from(action: Action) -> Self {
        Self(vec![action])
    }
}

#[cfg(test)]
mod tests {
    use log::info;
    use size::Size;

    use crate::{
        crypto::{Md5, Sha1},
        torrent::DownloadType,
    };

    use super::*;

    #[test]
    fn keep_alive() {
        let mut event_loop = create_event_loop();

        assert_eq!(
            event_loop.handle(Event::KeepAliveTick),
            Actions(vec![Action::Broadcast(Message::KeepAlive)])
        );
    }

    #[test]
    fn sequence() {
        env_logger::init();

        let mut event_loop = create_event_loop();

        let addr = "127.0.0.1:6881".parse().unwrap();

        run(
            &mut event_loop,
            Event::Message(addr, Message::Have { piece: 0 }),
        );
        run(&mut event_loop, Event::Message(addr, Message::Unchoke));
        run(
            &mut event_loop,
            Event::Message(
                addr,
                Message::Piece {
                    piece: 0,
                    offset: 0,
                    data: vec![0; 16384],
                },
            ),
        );
    }

    fn run(event_loop: &mut EventLoop, event: Event) {
        info!("<<< {:?}", &event);
        info!(">>> {:?}", event_loop.handle(event));
    }

    fn create_event_loop() -> EventLoop {
        let torrent = Info {
            info_hash: Sha1::from_hex("e90cf5ec83e174d7dcb94821560dac201ae1f663").unwrap(),
            piece_length: Size::from_kibibytes(32),
            pieces: vec![
                Sha1::from_hex("8fdfb566405fc084761b1fe0b6b7f8c6a37234ed").unwrap(),
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

        EventLoop::new(torrent, Config::default(), BitSet::new())
    }
}
