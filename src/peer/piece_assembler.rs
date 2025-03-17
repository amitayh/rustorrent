#![allow(dead_code)]
use std::collections::HashMap;
use std::ptr::hash;

use sha1::Digest;
use size::Size;

use crate::crypto::Sha1;
use crate::peer::message::Block;

pub struct PieceAssembler {
    piece_size: Size,
    block_size: Size,
    blocks_per_piece: usize,
    pieces: Vec<Sha1>,
    completed_blocks: HashMap<usize, Vec<Option<Vec<u8>>>>,
}

impl PieceAssembler {
    pub fn new(piece_size: Size, block_size: Size, pieces: Vec<Sha1>) -> Self {
        let blocks_per_piece =
            ((piece_size.bytes() as f64) / (block_size.bytes() as f64)).ceil() as usize;
        Self {
            piece_size,
            block_size,
            blocks_per_piece,
            pieces,
            completed_blocks: HashMap::new(),
        }
    }

    pub fn add(&mut self, block: Block, data: Vec<u8>) -> bool {
        let entry = self
            .completed_blocks
            .entry(block.piece)
            .or_insert_with(|| vec![None; self.blocks_per_piece]);
        let block_index = block.offset / (self.block_size.bytes() as usize);
        let block_data = entry.get_mut(block_index).expect("invalid block index");
        *block_data = Some(data);

        let all_blocks_completed = entry.iter().all(|block_data| block_data.is_some());
        if !all_blocks_completed {
            return false;
        }

        let mut hasher = sha1::Sha1::new();
        for block_data in entry {
            if let Some(data) = block_data {
                hasher.update(&data);
            }
        }
        let actual = Sha1(hasher.finalize().into());
        let expected = self.pieces.get(block.piece);
        expected.is_some_and(|hash| hash == &actual)
    }
}

#[cfg(test)]
mod tests {
    use crate::peer::message::Block;

    use super::*;

    const PIECE_SIZE: Size = Size::from_const(16);
    const BLOCK_SIZE: Size = Size::from_const(8);

    #[test]
    fn piece_incomplete() {
        let mut assembler = PieceAssembler::new(PIECE_SIZE, BLOCK_SIZE, vec![]);
        let complete = assembler.add(Block::new(0, 0, 8), vec![0; 8]);

        assert!(!complete);
    }

    #[test]
    fn piece_complete() {
        let mut assembler = PieceAssembler::new(
            PIECE_SIZE,
            BLOCK_SIZE,
            vec![Sha1::from_hex("e129f27c5103bc5cc44bcdf0a15e160d445066ff").unwrap()],
        );
        assembler.add(Block::new(0, 0, 8), vec![0; 8]);
        let complete = assembler.add(Block::new(0, 8, 8), vec![0; 8]);

        assert!(complete);
    }

    #[test]
    fn piece_complete_but_invalid() {
        let mut assembler = PieceAssembler::new(PIECE_SIZE, BLOCK_SIZE, vec![]);

        assembler.add(Block::new(0, 0, 8), vec![0; 8]);
        let complete = assembler.add(Block::new(0, 8, 8), vec![0; 8]);

        assert!(!complete);
    }
}
