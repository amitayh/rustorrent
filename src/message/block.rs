#[derive(Debug, PartialEq, Clone, Copy, Eq, Hash)]
pub struct Block {
    pub piece: usize,
    pub offset: usize,
    pub length: usize,
}

impl Block {
    pub fn new(piece: usize, offset: usize, length: usize) -> Self {
        Self {
            piece,
            offset,
            length,
        }
    }

    pub fn global_offset(&self, piece_size: usize) -> usize {
        (self.piece * piece_size) + self.offset
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct BlockData {
    pub piece: usize,
    pub offset: usize,
    pub data: Vec<u8>,
}

impl From<&BlockData> for Block {
    fn from(value: &BlockData) -> Self {
        Self::new(value.piece, value.offset, value.data.len())
    }
}
