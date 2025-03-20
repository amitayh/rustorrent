use size::Size;

pub struct Sizes {
    pub piece_size: Size,
    pub total_size: Size,
    pub block_size: Size,
    pub total_pieces: usize,
}

impl Sizes {
    pub fn new(piece_size: Size, total_size: Size, block_size: Size) -> Self {
        let total_pieces =
            ((total_size.bytes() as f64) / (piece_size.bytes() as f64)).ceil() as usize;
        Self {
            piece_size,
            total_size,
            block_size,
            total_pieces,
        }
    }

    pub fn piece_size(&self, piece: usize) -> usize {
        let piece_size = self.piece_size.bytes() as usize;
        let total_size = self.total_size.bytes() as usize;
        let piece_start = self.piece_offset(piece);
        let piece_end = (piece_start + piece_size).min(total_size);
        piece_end - piece_start
    }

    pub fn piece_offset(&self, piece: usize) -> usize {
        let piece_size = self.piece_size.bytes() as usize;
        piece_size * piece
    }
}
