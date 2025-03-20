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
}
