use crate::message::Block;

pub struct Blocks {
    block_size: usize,
    piece: usize,
    offset: usize,
    end: usize,
}

impl Blocks {
    pub fn new(piece: usize, piece_size: usize, block_size: usize) -> Self {
        Self {
            block_size,
            piece,
            offset: 0,
            end: piece_size,
        }
    }
}

impl Iterator for Blocks {
    type Item = Block;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset < self.end {
            let block_size = self.block_size.min(self.end - self.offset);
            let block = Block::new(self.piece, self.offset, block_size);
            self.offset += block_size;
            Some(block)
        } else {
            None
        }
    }
}

impl ExactSizeIterator for Blocks {
    fn len(&self) -> usize {
        ((self.end as f64) / (self.block_size as f64)).ceil() as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_piece_one_block() {
        let mut blocks = Blocks::new(0, 1024, 1024);

        assert_eq!(blocks.len(), 1);
        assert_eq!(Some(Block::new(0, 0, 1024)), blocks.next());
        assert_eq!(None, blocks.next());
    }

    #[test]
    fn one_piece_multiple_blocks() {
        let mut blocks = Blocks::new(0, 2048, 1024);

        assert_eq!(blocks.len(), 2);
        assert_eq!(Some(Block::new(0, 0, 1024)), blocks.next());
        assert_eq!(Some(Block::new(0, 1024, 1024)), blocks.next());
        assert_eq!(None, blocks.next());
    }

    #[test]
    fn uneven_block_sizes() {
        let mut blocks = Blocks::new(0, 1066, 1024);

        assert_eq!(blocks.len(), 2);
        assert_eq!(Some(Block::new(0, 0, 1024)), blocks.next());
        assert_eq!(Some(Block::new(0, 1024, 42)), blocks.next());
        assert_eq!(None, blocks.next());
    }

    #[test]
    fn uneven_block_sizes_in_non_first_piece() {
        let mut blocks = Blocks::new(1, 1066, 1024);

        assert_eq!(blocks.len(), 2);
        assert_eq!(Some(Block::new(1, 0, 1024)), blocks.next());
        assert_eq!(Some(Block::new(1, 1024, 42)), blocks.next());
        assert_eq!(None, blocks.next());
    }

    #[test]
    fn uneven_block_sizes_in_last_piece() {
        let mut blocks = Blocks::new(1, 1024 * 3, 1024);

        assert_eq!(blocks.len(), 3);
        assert_eq!(Some(Block::new(1, 0, 1024)), blocks.next());
        assert_eq!(Some(Block::new(1, 1024, 1024)), blocks.next());
        assert_eq!(Some(Block::new(1, 2048, 1024)), blocks.next());
        assert_eq!(None, blocks.next());

        let mut blocks = Blocks::new(2, 1024 * 2, 1024);

        assert_eq!(blocks.len(), 2);
        assert_eq!(Some(Block::new(2, 0, 1024)), blocks.next());
        assert_eq!(Some(Block::new(2, 1024, 1024)), blocks.next());
        assert_eq!(None, blocks.next());
    }

    #[test]
    fn real_world_example() {
        let mut blocks = Blocks::new(5, 6760, 16384);

        assert_eq!(blocks.len(), 1);
        assert_eq!(Some(Block::new(5, 0, 6760)), blocks.next());
        assert_eq!(None, blocks.next());
    }
}
