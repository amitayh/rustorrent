#![allow(dead_code)]
use std::collections::HashMap;

use sha1::Digest;
use size::Size;

use crate::crypto::Sha1;
use crate::peer::sizes::Sizes;

pub struct PieceAssembler {
    piece_size: Size,
    block_size: Size,
    blocks_per_piece: usize,
    pieces: Vec<Sha1>,
    completed_blocks: HashMap<usize, Vec<Option<Vec<u8>>>>,
}

impl PieceAssembler {
    pub fn new(sizes: &Sizes, pieces: Vec<Sha1>) -> Self {
        let blocks_per_piece =
            ((sizes.piece_size.bytes() as f64) / (sizes.block_size.bytes() as f64)).ceil() as usize;
        Self {
            piece_size: sizes.piece_size,
            block_size: sizes.block_size,
            blocks_per_piece,
            pieces,
            completed_blocks: HashMap::new(),
        }
    }

    pub fn add(&mut self, piece: usize, offset: usize, data: Vec<u8>) -> Status {
        let entry = self
            .completed_blocks
            .entry(piece)
            .or_insert_with(|| vec![None; self.blocks_per_piece]);
        let block_index = offset / (self.block_size.bytes() as usize);
        let block_data = entry.get_mut(block_index).expect("invalid block index");
        *block_data = Some(data);

        let all_blocks_completed = entry.iter().all(|block_data| block_data.is_some());
        if !all_blocks_completed {
            return Status::Incomplete;
        }

        let mut hasher = sha1::Sha1::new();
        for data in entry.iter_mut().flatten() {
            hasher.update(&data);
        }
        let actual = Sha1(hasher.finalize().into());
        let expected = self.pieces.get(piece);
        if expected.is_some_and(|hash| hash == &actual) {
            Status::Valid
        } else {
            Status::Invalid
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    Incomplete,
    Invalid,
    Valid,
}

struct PieceState {
    sha1: Sha1,
    data: Vec<Option<Vec<u8>>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sizes() -> Sizes {
        Sizes::new(
            Size::from_bytes(16),
            Size::from_bytes(24),
            Size::from_bytes(8),
        )
    }

    #[test]
    fn piece_incomplete() {
        let mut assembler = PieceAssembler::new(&sizes(), vec![]);

        assert_eq!(assembler.add(0, 0, vec![0; 8]), Status::Incomplete);
    }

    #[test]
    fn piece_complete() {
        let mut assembler = PieceAssembler::new(
            &sizes(),
            vec![Sha1::from_hex("e129f27c5103bc5cc44bcdf0a15e160d445066ff").unwrap()],
        );

        assert_eq!(assembler.add(0, 0, vec![0; 8]), Status::Incomplete);
        assert_eq!(assembler.add(0, 8, vec![0; 8]), Status::Valid);
    }

    #[test]
    fn piece_complete_but_invalid() {
        let mut assembler = PieceAssembler::new(&sizes(), vec![]);

        assert_eq!(assembler.add(0, 0, vec![0; 8]), Status::Incomplete);
        assert_eq!(assembler.add(0, 8, vec![0; 8]), Status::Invalid);
    }

    #[test]
    fn last_piece_has_fewer_blocks() {
        let sizes = Sizes::new(
            Size::from_bytes(24),
            Size::from_bytes(36),
            Size::from_bytes(8),
        );
        let mut assembler = PieceAssembler::new(
            &sizes,
            vec![
                Sha1::from_hex("e129f27c5103bc5cc44bcdf0a15e160d445066ff").unwrap(),
                Sha1::from_hex("e129f27c5103bc5cc44bcdf0a15e160d445066ff").unwrap(),
            ],
        );

        assert_eq!(assembler.add(0, 0, vec![0; 8]), Status::Incomplete);
        assert_eq!(assembler.add(0, 8, vec![0; 8]), Status::Incomplete);
        assert_eq!(assembler.add(0, 16, vec![0; 8]), Status::Valid);
        assert_eq!(assembler.add(1, 0, vec![0; 8]), Status::Incomplete);
        assert_eq!(assembler.add(1, 8, vec![0; 4]), Status::Valid);
    }
}
