use std::io::Result;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::codec::{AsyncDecoder, AsyncEncoder};

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

impl AsyncDecoder for Block {
    async fn decode<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Self> {
        let piece = stream.read_u32().await? as usize;
        let offset = stream.read_u32().await? as usize;
        let length = stream.read_u32().await? as usize;
        Ok(Block {
            piece,
            offset,
            length,
        })
    }
}

impl AsyncEncoder for Block {
    async fn encode<S: AsyncWrite + Unpin>(&self, stream: &mut S) -> Result<()> {
        stream.write_u32(self.piece as u32).await?;
        stream.write_u32(self.offset as u32).await?;
        stream.write_u32(self.length as u32).await?;
        Ok(())
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
