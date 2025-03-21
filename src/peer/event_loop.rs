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

struct EventLoop {
    choker: Choker,
    distributor: Distributor,
    joiner: Joiner,
    am_interested: HashSet<SocketAddr>,
}

impl EventLoop {
    fn new(torrent_info: Info, config: Config) -> Self {
        let sizes = Sizes::new(
            torrent_info.piece_length,
            torrent_info.download_type.length(),
            config.block_size,
        );
        Self {
            choker: Choker::new(config.optimistic_choking_cycle),
            distributor: Distributor::new(&sizes),
            joiner: Joiner::new(&sizes, torrent_info.pieces.clone()),
            am_interested: HashSet::new(),
        }
    }

    fn keep_alive(&self) -> Actions {
        Action::Broadcast(Message::KeepAlive).into()
    }

    fn run_chokig_algorithm(&mut self) -> Actions {
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

    fn handle(&mut self, addr: SocketAddr, message: Message) -> Actions {
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
                self.handle(addr, Message::Bitfield(pieces))
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

    fn connect(&self, addr: SocketAddr) -> Actions {
        let pieces = BitSet::new();
        Action::Send(addr, Message::Bitfield(pieces)).into()
    }

    fn disconnect(&mut self, addr: &SocketAddr) -> Actions {
        self.choker.peer_disconnected(addr);
        self.distributor.peer_disconnected(addr);
        Actions::none()
    }
}

#[derive(Debug, PartialEq)]
enum Action {
    Broadcast(Message),
    Send(SocketAddr, Message),
    Upload(SocketAddr, Block),
    IntegratePiece { offset: u64, data: Vec<u8> },
}

#[derive(Debug, PartialEq)]
struct Actions(Vec<Action>);

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
        let event_loop = create_event_loop();

        assert_eq!(
            event_loop.keep_alive(),
            Actions(vec![Action::Broadcast(Message::KeepAlive)])
        );
    }

    #[test]
    fn sequence() {
        env_logger::init();

        let mut event_loop = create_event_loop();

        let addr = "127.0.0.1:6881".parse().unwrap();

        run(&mut event_loop, addr, Message::Have { piece: 0 });
        run(&mut event_loop, addr, Message::Unchoke);
        run(
            &mut event_loop,
            addr,
            Message::Piece {
                piece: 0,
                offset: 0,
                data: vec![0; 16384],
            },
        );
        dbg!(event_loop.run_chokig_algorithm());
    }

    fn run(event_loop: &mut EventLoop, addr: SocketAddr, message: Message) {
        info!("<<< {:?}", &message);
        let actions = event_loop.handle(addr, Message::Have { piece: 0 });
        info!(">>> {:?}", actions);
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

        EventLoop::new(torrent, Config::default())
    }
}
