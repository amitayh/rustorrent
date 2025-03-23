use std::fmt::Formatter;
use std::io::{Error, ErrorKind, Result};

use bit_set::BitSet;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::codec::{AsyncDecoder, AsyncEncoder, TransportMessage};
use crate::message::Block;

const ID_CHOKE: u8 = 0;
const ID_UNCHOKE: u8 = 1;
const ID_INTERESTED: u8 = 2;
const ID_NOT_INTERESTED: u8 = 3;
const ID_HAVE: u8 = 4;
const ID_BITFIELD: u8 = 5;
const ID_REQUEST: u8 = 6;
const ID_PIECE: u8 = 7;
const ID_CANCEL: u8 = 8;
const ID_PORT: u8 = 9;

#[derive(PartialEq, Clone)]
pub enum Message {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have {
        piece: usize,
    },
    Bitfield(BitSet),
    Request(Block),
    Piece {
        piece: usize,
        offset: usize,
        data: Vec<u8>,
    },
    Cancel(Block),
    Port(u16),
}

impl std::fmt::Debug for Message {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Message::KeepAlive => write!(f, "KeepAlive"),
            Message::Choke => write!(f, "Choke"),
            Message::Unchoke => write!(f, "Unchoke"),
            Message::Interested => write!(f, "Interested"),
            Message::NotInterested => write!(f, "NotInterested"),
            Message::Have { piece } => write!(f, "Have {{ piece: {} }}", piece),
            Message::Bitfield(bitset) => write!(f, "Bitfield({:?})", bitset),
            Message::Request(block) => write!(f, "Request({:?})", block),
            Message::Piece {
                piece,
                offset,
                data,
            } => {
                write!(
                    f,
                    "Piece {{ piece: {}, offset: {}, data: <{} bytes> }}",
                    piece,
                    offset,
                    data.len()
                )
            }
            Message::Cancel(block) => write!(f, "Cancel({:?})", block),
            Message::Port(port) => write!(f, "Port({})", port),
        }
    }
}

impl AsyncDecoder for Message {
    async fn decode<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Self> {
        let length = stream.read_u32().await? as usize;
        if length == 0 {
            return Ok(Self::KeepAlive);
        }
        let id = stream.read_u8().await?;
        match (id, length) {
            (ID_CHOKE, 1) => Ok(Self::Choke),
            (ID_UNCHOKE, 1) => Ok(Self::Unchoke),
            (ID_INTERESTED, 1) => Ok(Self::Interested),
            (ID_NOT_INTERESTED, 1) => Ok(Self::NotInterested),
            (ID_HAVE, 5) => {
                let piece = stream.read_u32().await? as usize;
                Ok(Self::Have { piece })
            }
            (ID_BITFIELD, 1..) => {
                let mut buf = vec![0; length - 1];
                stream.read_exact(&mut buf).await?;
                let bitset = BitSet::from_bytes(&buf);
                Ok(Self::Bitfield(bitset))
            }
            (ID_REQUEST, 13) => {
                let block = Block::decode(stream).await?;
                Ok(Self::Request(block))
            }
            (ID_PIECE, 9..) => {
                let piece = stream.read_u32().await? as usize;
                let offset = stream.read_u32().await? as usize;
                let mut data = vec![0; length - 9];
                stream.read_exact(&mut data).await?;
                Ok(Self::Piece {
                    piece,
                    offset,
                    data,
                })
            }
            (ID_CANCEL, 13) => {
                let block = Block::decode(stream).await?;
                Ok(Self::Cancel(block))
            }
            (ID_PORT, 3) => {
                let port = stream.read_u16().await?;
                Ok(Self::Port(port))
            }
            _ => Err(Error::new(
                ErrorKind::InvalidData,
                format!("invalid message id {} with length {}", id, length),
            )),
        }
    }
}

impl AsyncEncoder for Message {
    async fn encode<S: AsyncWrite + Unpin>(&self, stream: &mut S) -> Result<()> {
        match self {
            Self::KeepAlive => stream.write_u32(0).await?,
            Self::Choke => {
                stream.write_u32(1).await?;
                stream.write_u8(ID_CHOKE).await?;
            }
            Self::Unchoke => {
                stream.write_u32(1).await?;
                stream.write_u8(ID_UNCHOKE).await?;
            }
            Self::Interested => {
                stream.write_u32(1).await?;
                stream.write_u8(ID_INTERESTED).await?;
            }
            Self::NotInterested => {
                stream.write_u32(1).await?;
                stream.write_u8(ID_NOT_INTERESTED).await?;
            }
            Self::Have { piece } => {
                stream.write_u32(5).await?;
                stream.write_u8(ID_HAVE).await?;
                stream.write_u32(*piece as u32).await?;
            }
            Self::Bitfield(bitset) => {
                let bytes = bitset.get_ref().to_bytes();
                stream.write_u32(1 + (bytes.len() as u32)).await?;
                stream.write_u8(ID_BITFIELD).await?;
                stream.write_all(&bytes).await?;
            }
            Self::Request(block) => {
                stream.write_u32(13).await?;
                stream.write_u8(ID_REQUEST).await?;
                block.encode(stream).await?;
            }
            Self::Piece {
                piece,
                offset,
                data,
            } => {
                let length = 9 + data.len();
                stream.write_u32(length as u32).await?;
                stream.write_u8(ID_PIECE).await?;
                stream.write_u32(*piece as u32).await?;
                stream.write_u32(*offset as u32).await?;
                stream.write_all(data).await?;
            }
            Self::Cancel(block) => {
                stream.write_u32(13).await?;
                stream.write_u8(ID_CANCEL).await?;
                block.encode(stream).await?;
            }
            Self::Port(port) => {
                stream.write_u32(3).await?;
                stream.write_u8(ID_PORT).await?;
                stream.write_u16(*port).await?;
            }
        }
        stream.flush().await?;
        Ok(())
    }
}

impl TransportMessage for Message {
    fn transport_bytes(&self) -> usize {
        let bytes = match self {
            Self::KeepAlive => 0,
            Self::Choke => 1,
            Self::Unchoke => 1,
            Self::Interested => 1,
            Self::NotInterested => 1,
            Self::Have { piece: _ } => 5,
            Self::Bitfield(bitset) => 1 + bitset.get_ref().to_bytes().len(),
            Self::Request(_) => 13,
            Self::Piece {
                piece: _,
                offset: _,
                data,
            } => 9 + data.len(),
            Self::Cancel(_) => 13,
            Self::Port(_) => 3,
        };
        4 + bytes
    }
}
