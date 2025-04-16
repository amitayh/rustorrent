use std::io::SeekFrom;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::{fs::File, sync::mpsc::Sender};

use crate::message::Block;
use crate::message::{BlockData, Message};
use crate::peer::Download;

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
        let offset = block.global_offset(self.download.torrent.info.piece_size);
        file.seek(SeekFrom::Start(offset as u64)).await?;
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
