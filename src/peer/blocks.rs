use size::Size;

use crate::peer::message::Block;

pub struct Blocks {
    piece_size: usize,
    block_size: usize,
    piece: usize,
    offset: usize,
    end: usize,
}

impl Blocks {
    pub fn new(piece_size: Size, total_size: Size, block_size: Size, piece: usize) -> Self {
        let piece_size = piece_size.bytes() as usize;
        let total_size = total_size.bytes() as usize;
        let block_size = block_size.bytes() as usize;

        let piece_start = piece_size * piece;
        let piece_end = (piece_start + piece_size).min(total_size);

        Self {
            piece_size,
            block_size,
            piece,
            offset: 0,
            end: piece_end - piece_start,
        }
    }
}

impl Iterator for Blocks {
    type Item = Block;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset < self.end {
            let block_size = self.block_size.min(self.piece_size - self.offset);
            let block = Block::new(self.piece, self.offset, block_size);
            self.offset += block_size;
            Some(block)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use size::KiB;

    use super::*;

    const BLOCK_SIZE: Size = Size::from_const(KiB);
    const BLOCK_SIZE_BYTES: usize = BLOCK_SIZE.bytes() as usize;

    #[test]
    fn one_piece_one_block() {
        let mut blocks = Blocks::new(
            Size::from_kibibytes(1),
            Size::from_kibibytes(1),
            BLOCK_SIZE,
            0,
        );

        assert_eq!(Some(Block::new(0, 0, BLOCK_SIZE_BYTES)), blocks.next());
        assert_eq!(None, blocks.next());
    }

    #[test]
    fn one_piece_multiple_blocks() {
        let mut blocks = Blocks::new(
            Size::from_kibibytes(2),
            Size::from_kibibytes(2),
            BLOCK_SIZE,
            0,
        );

        assert_eq!(Some(Block::new(0, 0, BLOCK_SIZE_BYTES)), blocks.next());
        assert_eq!(Some(Block::new(0, 1024, BLOCK_SIZE_BYTES)), blocks.next());
        assert_eq!(None, blocks.next());
    }

    #[test]
    fn uneven_block_sizes() {
        let mut blocks = Blocks::new(
            BLOCK_SIZE + Size::from_bytes(42),
            Size::from_kibibytes(2),
            BLOCK_SIZE,
            0,
        );

        assert_eq!(Some(Block::new(0, 0, BLOCK_SIZE_BYTES)), blocks.next());
        assert_eq!(Some(Block::new(0, 1024, 42)), blocks.next());
        assert_eq!(None, blocks.next());
    }

    #[test]
    fn uneven_block_sizes_in_non_first_piece() {
        let mut blocks = Blocks::new(
            BLOCK_SIZE + Size::from_bytes(42),
            Size::from_kibibytes(4),
            BLOCK_SIZE,
            1,
        );

        assert_eq!(Some(Block::new(1, 0, BLOCK_SIZE_BYTES)), blocks.next());
        assert_eq!(Some(Block::new(1, 1024, 42)), blocks.next());
        assert_eq!(None, blocks.next());
    }

    #[test]
    fn uneven_block_sizes_in_last_piece() {
        let mut blocks = Blocks::new(
            Size::from_kibibytes(3),
            Size::from_kibibytes(8),
            BLOCK_SIZE,
            1,
        );

        assert_eq!(Some(Block::new(1, 0, BLOCK_SIZE_BYTES)), blocks.next());
        assert_eq!(Some(Block::new(1, 1024, BLOCK_SIZE_BYTES)), blocks.next());
        assert_eq!(Some(Block::new(1, 2048, BLOCK_SIZE_BYTES)), blocks.next());
        assert_eq!(None, blocks.next());

        let mut blocks = Blocks::new(
            Size::from_kibibytes(3),
            Size::from_kibibytes(8),
            BLOCK_SIZE,
            2,
        );

        assert_eq!(Some(Block::new(2, 0, BLOCK_SIZE_BYTES)), blocks.next());
        assert_eq!(Some(Block::new(2, 1024, BLOCK_SIZE_BYTES)), blocks.next());
        assert_eq!(None, blocks.next());
    }
}
