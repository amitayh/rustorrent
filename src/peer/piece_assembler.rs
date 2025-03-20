#![allow(dead_code)]

use sha1::Digest;

use crate::crypto::Sha1;
use crate::peer::sizes::Sizes;

pub struct PieceAssembler {
    block_size: usize,
    pieces: Vec<PieceState>,
}

impl PieceAssembler {
    pub fn new(sizes: &Sizes, hashes: Vec<Sha1>) -> Self {
        assert_eq!(sizes.total_pieces, hashes.len());
        let block_size = sizes.block_size.bytes() as usize;
        let mut pieces = Vec::with_capacity(sizes.total_pieces);
        for (piece, sha1) in hashes.into_iter().enumerate() {
            pieces.push(PieceState::new(piece, sha1, sizes));
        }
        Self { block_size, pieces }
    }

    pub fn add(&mut self, piece: usize, offset: usize, data: Vec<u8>) -> Status {
        let piece = self.pieces.get_mut(piece).expect("invalid piece");
        let block = offset / self.block_size;
        piece.add(block, data)
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

impl PieceState {
    fn new(piece: usize, sha1: Sha1, sizes: &Sizes) -> Self {
        let piece_size = sizes.piece_size(piece) as f64;
        let block_size = sizes.block_size.bytes() as f64;
        let blocks_per_piece = (piece_size / block_size).ceil() as usize;
        let data = vec![None; blocks_per_piece];
        Self { sha1, data }
    }

    fn add(&mut self, block: usize, data: Vec<u8>) -> Status {
        let block_data = self.data.get_mut(block).expect("invalid block index");
        *block_data = Some(data);

        let all_blocks_completed = self.data.iter().all(|block_data| block_data.is_some());
        if !all_blocks_completed {
            return Status::Incomplete;
        }

        let mut hasher = sha1::Sha1::new();
        for data in self.data.iter_mut().flatten() {
            hasher.update(&data);
        }
        let sha1 = Sha1(hasher.finalize().into());
        if self.sha1 == sha1 {
            Status::Valid
        } else {
            Status::Invalid
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use size::Size;

    fn sizes() -> Sizes {
        Sizes::new(
            Size::from_bytes(16),
            Size::from_bytes(24),
            Size::from_bytes(8),
        )
    }

    #[test]
    fn piece_incomplete() {
        let mut assembler = PieceAssembler::new(&sizes(), vec![Sha1([0; 20]), Sha1([0; 20])]);

        assert_eq!(assembler.add(0, 0, vec![0; 8]), Status::Incomplete);
    }

    #[test]
    fn piece_complete() {
        let mut assembler = PieceAssembler::new(
            &sizes(),
            vec![
                Sha1::from_hex("e129f27c5103bc5cc44bcdf0a15e160d445066ff").unwrap(),
                Sha1([0; 20]),
            ],
        );

        assert_eq!(assembler.add(0, 0, vec![0; 8]), Status::Incomplete);
        assert_eq!(assembler.add(0, 8, vec![0; 8]), Status::Valid);
    }

    #[test]
    fn piece_complete_but_invalid() {
        let mut assembler = PieceAssembler::new(&sizes(), vec![Sha1([0; 20]), Sha1([0; 20])]);

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
                Sha1::from_hex("d3399b7262fb56cb9ed053d68db9291c410839c4").unwrap(),
                Sha1::from_hex("2c513f149e737ec4063fc1d37aee9beabc4b4bbf").unwrap(),
            ],
        );

        assert_eq!(assembler.add(0, 0, vec![0; 8]), Status::Incomplete);
        assert_eq!(assembler.add(0, 8, vec![0; 8]), Status::Incomplete);
        assert_eq!(assembler.add(0, 16, vec![0; 8]), Status::Valid);
        assert_eq!(assembler.add(1, 0, vec![0; 8]), Status::Incomplete);
        assert_eq!(assembler.add(1, 8, vec![0; 4]), Status::Valid);
    }

    #[test]
    fn add_blocks_out_of_order() {
        let sizes = Sizes::new(
            Size::from_bytes(24),
            Size::from_bytes(36),
            Size::from_bytes(8),
        );
        let mut assembler = PieceAssembler::new(
            &sizes,
            vec![
                Sha1::from_hex("d3399b7262fb56cb9ed053d68db9291c410839c4").unwrap(),
                Sha1::from_hex("2c513f149e737ec4063fc1d37aee9beabc4b4bbf").unwrap(),
            ],
        );

        assert_eq!(assembler.add(0, 0, vec![0; 8]), Status::Incomplete);
        assert_eq!(assembler.add(0, 16, vec![0; 8]), Status::Incomplete);
        assert_eq!(assembler.add(0, 8, vec![0; 8]), Status::Valid);
    }
}
