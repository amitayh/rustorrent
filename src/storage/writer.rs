use std::io::SeekFrom;
use std::sync::Arc;

use log::warn;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc::Sender;

use crate::client::Download;
use crate::event::Event;
use crate::message::BlockData;
use crate::storage::{Joiner, Status};

pub struct FileWriter {
    download: Arc<Download>,
    joiner: Joiner,
    tx: Sender<Event>,
}

impl FileWriter {
    pub fn new(download: Arc<Download>, tx: Sender<Event>) -> Self {
        let joiner = Joiner::new(&download);
        Self {
            download,
            joiner,
            tx,
        }
    }

    pub async fn write(&mut self, block_data: BlockData) -> anyhow::Result<()> {
        let piece = block_data.piece;
        match self.joiner.add(block_data) {
            Status::Incomplete => (), // Noting to do, wait for next block
            Status::Invalid => {
                warn!("piece {} sha1 mismatch", piece);
                self.tx.send(Event::PieceVerificationFailed(piece)).await?;
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
                self.tx.send(Event::PieceCompleted(piece)).await?;
            }
        }
        Ok(())
    }
}
