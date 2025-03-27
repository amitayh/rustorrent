use std::{io::SeekFrom, path::PathBuf};

use log::warn;
use size::Size;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::{fs::File, sync::mpsc::Sender};

use crate::message::{BlockData, Message};
use crate::peer::piece::Joiner;
use crate::peer::sizes::Sizes;
use crate::torrent::Info;
use crate::{message::Block, peer::Event};

use super::piece::Status;

pub struct FileReaderWriter {
    path: PathBuf,
    joiner: Joiner,
    piece_size: Size,
}

impl FileReaderWriter {
    pub fn new(path: PathBuf, sizes: &Sizes, torrent_info: Info) -> Self {
        let joiner = Joiner::new(sizes, torrent_info.pieces);
        let piece_size = sizes.piece_size;
        Self {
            path,
            joiner,
            piece_size,
        }
    }

    pub async fn read(&self, block: Block, tx: Sender<Message>) -> anyhow::Result<()> {
        let mut data = vec![0; block.length];
        let mut file = File::open(&self.path).await?;
        let offset = block.global_offset(self.piece_size.bytes() as usize);
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
        let add = self.joiner.add(block_data);
        match add {
            Status::Incomplete => (), // Noting to do, wait for next block
            Status::Invalid => {
                warn!("piece {} sha1 mismatch", piece);
                tx.send(Event::PieceInvalid(piece)).await?;
            }
            Status::Complete { offset, data } => {
                let mut file = OpenOptions::new()
                    .write(true)
                    .truncate(false)
                    .open(&self.path)
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
