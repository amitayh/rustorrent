#![allow(dead_code)]

use std::{collections::HashSet, net::SocketAddr};

use bit_set::BitSet;
use log::warn;

use crate::message::{Block, Message};

use crate::peer::piece::Status;
use crate::peer::{
    choke::Choker,
    piece::{Distributor, Joiner},
};

struct EventLoop {
    choker: Choker,
    distributor: Distributor,
    joiner: Joiner,
    am_interested: HashSet<SocketAddr>,
}

impl EventLoop {
    fn keep_alive(&self) -> Action {
        Action::Broadcast(Message::KeepAlive)
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
                    Status::Invalid => self.distributor.invalidate(piece),
                    Status::Valid {
                        offset: piece_offset,
                        data: piece_data,
                    } => {
                        self.distributor.client_has_piece(piece);
                        actions.push(Action::Broadcast(Message::Have { piece }));
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

enum Action {
    Broadcast(Message),
    Send(SocketAddr, Message),
    Upload(SocketAddr, Block),
    IntegratePiece { offset: u64, data: Vec<u8> },
}

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
