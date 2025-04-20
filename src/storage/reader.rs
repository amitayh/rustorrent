use std::io::SeekFrom;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::{fs::File, sync::mpsc::Sender};

use crate::client::Download;
use crate::message::{Block, BlockData, Message};

pub struct FileReader {
    download: Arc<Download>,
}

impl FileReader {
    pub fn new(download: Arc<Download>) -> Self {
        Self { download }
    }

    pub async fn read(&self, block: Block, tx: Sender<Message>) -> anyhow::Result<()> {
        let mut data = vec![0; block.length];
        let mut file = File::open(&self.download.config.download_path).await?;
        let piece_size = self.download.torrent.info.piece_size;
        let global_offset = (block.piece * piece_size) + block.offset;
        file.seek(SeekFrom::Start(global_offset as u64)).await?;
        file.read_exact(&mut data).await?;
        let message = Message::Piece(BlockData {
            piece: block.piece,
            offset: block.offset,
            data,
        });
        tx.send(message).await?;
        Ok(())
    }
}
