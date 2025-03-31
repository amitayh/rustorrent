use crate::peer::Config;
use crate::peer::blocks::Blocks;
use crate::torrent::Torrent;

#[derive(Debug)]
pub struct Download {
    pub torrent: Torrent,
    pub config: Config,
}

impl Download {
    pub fn blocks(&self, piece: usize) -> Blocks {
        Blocks::new(
            piece,
            self.torrent.info.piece_size(piece),
            self.config.block_size.bytes() as usize,
        )
    }
}
