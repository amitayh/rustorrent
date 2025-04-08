use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
};

use crate::peer::scheduler::available_pieces::AvailablePiece;
use crate::{
    message::Block,
    peer::{Download, blocks::Blocks},
};

pub struct ActivePieces(HashMap<usize, ActivePiece>);

impl ActivePieces {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    pub fn insert(&mut self, index: usize, piece: ActivePiece) {
        self.0.insert(index, piece);
    }

    pub fn get_mut<'a>(&'a mut self, piece: &usize) -> &'a mut ActivePiece {
        self.0.get_mut(piece).expect("invalid piece")
    }

    pub fn contains(&self, piece: &usize) -> bool {
        self.0.contains_key(piece)
    }

    pub fn remove(&mut self, piece: &usize) -> ActivePiece {
        self.0.remove(piece).expect("invalid piece")
    }

    pub fn peer_pieces<'a>(
        &'a mut self,
        addr: &SocketAddr,
    ) -> impl Iterator<Item = &'a mut ActivePiece> {
        self.0
            .values_mut()
            .filter(|piece| piece.peers_with_piece.contains(addr))
    }
}

#[derive(Debug)]
pub struct ActivePiece {
    pub index: usize,
    total_blocks: usize,
    downloaded_blocks: usize,
    unassigned_blocks: Blocks,
    released_blocks: Vec<Block>,
    peers_with_piece: HashSet<SocketAddr>,
}

impl ActivePiece {
    pub fn new(piece: AvailablePiece, download: &Download) -> Self {
        let blocks = download.blocks(piece.index);
        Self {
            index: piece.index,
            total_blocks: blocks.len(),
            downloaded_blocks: 0,
            unassigned_blocks: blocks,
            released_blocks: Vec::new(),
            peers_with_piece: piece.peers_with_piece,
        }
    }

    pub fn iter_peers<'a>(&'a self) -> impl Iterator<Item = &'a SocketAddr> {
        self.peers_with_piece.iter()
    }

    pub fn peer_has_piece(&mut self, addr: SocketAddr) {
        self.peers_with_piece.insert(addr);
    }

    /// Mark block as downloaded. Returns `true` if piece is completed
    pub fn block_downloaded(&mut self) -> bool {
        self.downloaded_blocks += 1;
        self.downloaded_blocks == self.total_blocks
    }

    /// Returns `true` is there are no more connected peers with this piece
    pub fn peer_disconnected(&mut self, addr: &SocketAddr) -> bool {
        self.peers_with_piece.remove(addr);
        self.peers_with_piece.is_empty()
    }

    pub fn unassign(&mut self, block: Block) {
        self.released_blocks.push(block);
    }

    pub fn try_assign_n(&mut self, n: usize, dest: &mut Vec<Block>) -> usize {
        let mut assigned = 0;

        // Use released blocks first
        let released_n = n.min(self.released_blocks.len());
        dest.extend(self.released_blocks.drain(0..released_n));
        assigned += released_n;

        // Assign remaining from unassigned blocks
        while assigned < n {
            if let Some(block) = self.unassigned_blocks.next() {
                dest.push(block);
                assigned += 1;
            } else {
                break;
            }
        }

        assigned
    }
}
