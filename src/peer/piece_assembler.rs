use std::collections::HashMap;

use super::message::Block;

pub struct PieceAssembler {
    completed_blocks: HashMap<usize, Vec<Option<Vec<u8>>>>,
}

impl PieceAssembler {
    pub fn new() -> Self {
        Self {
            completed_blocks: HashMap::new(),
        }
    }

    pub fn add(&mut self, block: Block, data: Vec<u8>) {
        //
    }
}

#[cfg(test)]
mod tests {
    use crate::peer::message::Block;

    use super::*;

    #[test]
    fn piece_incomplete() {
        let mut assembler = PieceAssembler::new();
        assembler.add(Block::new(0, 0, 8), vec![0; 8]);
        assert_eq!(1 + 1, 3);
    }
}
