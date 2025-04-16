use std::io::SeekFrom;
use std::sync::Arc;

use log::warn;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::{fs::File, sync::mpsc::Sender};

use crate::message::{BlockData, Message};
use crate::peer::Download;
use crate::storage::{Joiner, Status};
use crate::{event::Event, message::Block};

pub struct FileReaderWriter {
    download: Arc<Download>,
    joiner: Joiner,
}

impl FileReaderWriter {
    pub fn new(download: Arc<Download>) -> Self {
        let joiner = Joiner::new(&download);
        Self { download, joiner }
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

    pub async fn write(&mut self, block_data: BlockData, tx: Sender<Event>) -> anyhow::Result<()> {
        let piece = block_data.piece;
        match self.joiner.add(block_data) {
            Status::Incomplete => (), // Noting to do, wait for next block
            Status::Invalid => {
                warn!("piece {} sha1 mismatch", piece);
                tx.send(Event::PieceInvalid(piece)).await?;
            }
            Status::Complete { offset, data } => {
                let mut file = OpenOptions::new()
                    .write(true)
                    .truncate(false)
                    .open(&self.download.config.download_path)
                    .await?;
                file.seek(SeekFrom::Start(offset)).await?;
                file.write_all(&data).await?;
                file.flush().await?;
                tx.send(Event::PieceCompleted(piece)).await?;
            }
        }
        Ok(())
    }
}
