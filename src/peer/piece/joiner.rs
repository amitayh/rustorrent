#![allow(dead_code)]

use core::f64;

use sha1::Digest;

use crate::crypto::Sha1;
use crate::peer::sizes::Sizes;

pub struct Joiner {
    block_size: usize,
    pieces: Vec<PieceState>,
}

// TODO: alternative names: Combiner, Stitcher, Fuser
impl Joiner {
    pub fn new(sizes: &Sizes, hashes: Vec<Sha1>) -> Self {
        assert_eq!(sizes.total_pieces, hashes.len());
        let block_size = sizes.block_size.bytes() as usize;
        let mut pieces = Vec::with_capacity(sizes.total_pieces);
        for (piece, sha1) in hashes.into_iter().enumerate() {
            let offset = sizes.piece_offset(piece) as u64;
            pieces.push(PieceState::new(piece, offset, sha1, sizes));
        }
        Self { block_size, pieces }
    }

    pub fn add(&mut self, piece: usize, offset: usize, data: Vec<u8>) -> Status {
        assert_eq!(offset % self.block_size, 0, "invalid offset");
        let piece = self.pieces.get_mut(piece).expect("invalid piece");
        let block = offset / self.block_size;
        piece.add(block, data)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    Incomplete,
    Invalid,
    Complete { offset: u64, data: Vec<u8> },
}

struct PieceState {
    size: usize,
    offset: u64,
    sha1: Sha1,
    data: Vec<Option<Vec<u8>>>,
}

impl PieceState {
    fn new(piece: usize, offset: u64, sha1: Sha1, sizes: &Sizes) -> Self {
        let piece_size = sizes.piece_size(piece);
        let block_size = sizes.block_size.bytes() as f64;
        let blocks = ((piece_size as f64) / block_size).ceil() as usize;
        let data = vec![None; blocks];
        Self {
            size: piece_size,
            offset,
            sha1,
            data,
        }
    }

    fn add(&mut self, block: usize, data: Vec<u8>) -> Status {
        let block_data = self.data.get_mut(block).expect("invalid block index");
        *block_data = Some(data);

        if self.data.iter().any(|block_data| block_data.is_none()) {
            return Status::Incomplete;
        }

        let mut hasher = sha1::Sha1::new();
        let mut piece_data = Vec::with_capacity(self.size);
        for block_data in self.data.iter_mut() {
            let block_data = block_data.take().expect("complete piece");
            hasher.update(&block_data);
            piece_data.extend(block_data);
        }
        let sha1 = Sha1(hasher.finalize().into());

        if self.sha1 == sha1 {
            Status::Complete {
                offset: self.offset,
                data: piece_data,
            }
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
            Size::from_bytes(8),
            Size::from_bytes(12),
            Size::from_bytes(4),
        )
    }

    #[test]
    fn piece_incomplete() {
        let mut joiner = Joiner::new(&sizes(), vec![Sha1([0; 20]), Sha1([0; 20])]);

        assert_eq!(joiner.add(0, 0, vec![0; 4]), Status::Incomplete);
    }

    #[test]
    fn piece_complete() {
        let mut joiner = Joiner::new(
            &sizes(),
            vec![
                Sha1::from_hex("30c921e4aae2b54c45f25de2e89f35ec1ce24e14").unwrap(),
                Sha1([0; 20]),
            ],
        );

        assert_eq!(joiner.add(0, 0, vec![0; 4]), Status::Incomplete);
        assert_eq!(
            joiner.add(0, 4, vec![1; 4]),
            Status::Complete {
                offset: 0,
                data: vec![0, 0, 0, 0, 1, 1, 1, 1]
            }
        );
    }

    #[test]
    fn piece_complete_but_invalid() {
        let mut joiner = Joiner::new(&sizes(), vec![Sha1([0; 20]), Sha1([0; 20])]);

        assert_eq!(joiner.add(0, 0, vec![0; 4]), Status::Incomplete);
        assert_eq!(joiner.add(0, 4, vec![0; 4]), Status::Invalid);
    }

    #[test]
    fn reset_piece_after_invalidation() {
        let mut joiner = Joiner::new(
            &sizes(),
            vec![
                Sha1::from_hex("05fe405753166f125559e7c9ac558654f107c7e9").unwrap(),
                Sha1([0; 20]),
            ],
        );

        // Invalid piece data
        assert_eq!(joiner.add(0, 0, vec![1; 4]), Status::Incomplete);
        assert_eq!(joiner.add(0, 4, vec![1; 4]), Status::Invalid);

        // Valid piece data
        assert_eq!(joiner.add(0, 0, vec![0; 4]), Status::Incomplete);
        assert!(matches!(
            joiner.add(0, 4, vec![0; 4]),
            Status::Complete { .. }
        ));
    }

    #[test]
    fn last_piece_has_fewer_blocks() {
        let sizes = Sizes::new(
            Size::from_bytes(24),
            Size::from_bytes(36),
            Size::from_bytes(8),
        );
        let mut joiner = Joiner::new(
            &sizes,
            vec![
                Sha1::from_hex("d3399b7262fb56cb9ed053d68db9291c410839c4").unwrap(),
                Sha1::from_hex("2c513f149e737ec4063fc1d37aee9beabc4b4bbf").unwrap(),
            ],
        );

        assert_eq!(joiner.add(0, 0, vec![0; 8]), Status::Incomplete);
        assert_eq!(joiner.add(0, 8, vec![0; 8]), Status::Incomplete);
        assert!(matches!(
            joiner.add(0, 16, vec![0; 8]),
            Status::Complete { .. }
        ));
        assert_eq!(joiner.add(1, 0, vec![0; 8]), Status::Incomplete);
        assert!(matches!(
            joiner.add(1, 8, vec![0; 4]),
            Status::Complete { offset: 24, .. }
        ));
    }

    #[test]
    fn add_blocks_out_of_order() {
        let sizes = Sizes::new(
            Size::from_bytes(6),
            Size::from_bytes(6),
            Size::from_bytes(2),
        );
        let mut joiner = Joiner::new(
            &sizes,
            vec![Sha1::from_hex("20bb50f8b56e82fd951e14fd0476eee5e0fa26e4").unwrap()],
        );

        assert_eq!(joiner.add(0, 0, vec![1; 2]), Status::Incomplete);
        assert_eq!(joiner.add(0, 4, vec![3; 2]), Status::Incomplete);
        assert_eq!(
            joiner.add(0, 2, vec![2; 2]),
            Status::Complete {
                offset: 0,
                data: vec![1, 1, 2, 2, 3, 3]
            }
        );
    }
}
