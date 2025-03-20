use std::fmt::Formatter;
use std::io::{Error, ErrorKind, Result};

use bit_set::BitSet;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::codec::{AsyncDecoder, AsyncEncoder, TransportMessage};
use crate::crypto::Sha1;
use crate::peer::peer_id::PeerId;

const PROTOCOL: &str = "BitTorrent protocol";
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

#[derive(Debug, PartialEq)]
pub struct Handshake {
    pub protocol: String,
    pub info_hash: Sha1,
    pub peer_id: PeerId,
}

impl Handshake {
    pub fn new(info_hash: Sha1, peer_id: PeerId) -> Self {
        Self {
            protocol: PROTOCOL.to_string(),
            info_hash,
            peer_id,
        }
    }
}

impl AsyncDecoder for Handshake {
    async fn decode<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Self> {
        let protocol = {
            let length = stream.read_u8().await?;
            let mut buf = vec![0; length as usize];
            stream.read_exact(&mut buf).await?;
            String::from_utf8(buf).map_err(|err| Error::new(ErrorKind::InvalidData, err))?
        };
        {
            // Skip 8 reserved bytes
            let mut buf = [0; 8];
            stream.read_exact(&mut buf).await?;
        };
        let info_hash = {
            let mut buf = [0; 20];
            stream.read_exact(&mut buf).await?;
            Sha1(buf)
        };
        let peer_id = {
            let mut buf = [0; 20];
            stream.read_exact(&mut buf).await?;
            PeerId(buf)
        };
        Ok(Handshake {
            protocol,
            info_hash,
            peer_id,
        })
    }
}

impl AsyncEncoder for Handshake {
    async fn encode<S: AsyncWrite + Unpin>(&self, stream: &mut S) -> Result<()> {
        let len = self
            .protocol
            .len()
            .try_into()
            .expect("protocol string too long");
        stream.write_u8(len).await?;
        stream.write_all(self.protocol.as_bytes()).await?;
        stream.write_all(&[0; 8]).await?;
        stream.write_all(&self.info_hash.0).await?;
        stream.write_all(&self.peer_id.0).await?;
        stream.flush().await?;
        Ok(())
    }
}

impl TransportMessage for Handshake {
    fn transport_bytes(&self) -> usize {
        1 + // pstr len
            self.protocol.bytes().len() + // pstr bytes
            8 + // reserved
            20 + // info hash
            20 // peer id
    }
}

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
                format!("invlid message id {}", id),
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

#[cfg(test)]
mod tests {
    use std::{
        fmt::Debug,
        io::{Cursor, Seek},
    };

    use super::*;

    async fn verify_encode_decode<
        T: AsyncEncoder + AsyncDecoder + TransportMessage + PartialEq + Debug,
    >(
        message: T,
    ) {
        let length = message.transport_bytes();
        let mut cursor = Cursor::new(Vec::with_capacity(length));
        message.encode(&mut cursor).await.expect("unable to encode");
        assert_eq!(length, cursor.position() as usize);
        cursor.rewind().expect("unable to rewind");
        let message_read = T::decode(&mut cursor).await.expect("unable to decode");
        assert_eq!(message, message_read);
    }

    #[tokio::test]
    async fn handshake() {
        verify_encode_decode(Handshake::new(Sha1([1; 20]), PeerId([2; 20]))).await;
    }

    #[tokio::test]
    async fn keep_alive() {
        verify_encode_decode(Message::KeepAlive).await;
    }

    #[tokio::test]
    async fn choke() {
        verify_encode_decode(Message::Choke).await;
    }

    #[tokio::test]
    async fn unchoke() {
        verify_encode_decode(Message::Unchoke).await;
    }

    #[tokio::test]
    async fn interested() {
        verify_encode_decode(Message::Interested).await;
    }

    #[tokio::test]
    async fn not_interested() {
        verify_encode_decode(Message::NotInterested).await;
    }

    #[tokio::test]
    async fn have() {
        verify_encode_decode(Message::Have { piece: 1234 }).await;
    }

    #[tokio::test]
    async fn bitfield() {
        verify_encode_decode(Message::Bitfield(BitSet::from_bytes(&[0b11010000]))).await;
    }

    #[tokio::test]
    async fn request() {
        verify_encode_decode(Message::Request(Block::new(1, 2, 3))).await;
    }

    #[tokio::test]
    async fn piece() {
        verify_encode_decode(Message::Piece {
            piece: 1,
            offset: 2,
            data: vec![1, 2, 3],
        })
        .await;
    }

    #[tokio::test]
    async fn cancel() {
        verify_encode_decode(Message::Cancel(Block::new(1, 2, 3))).await;
    }

    #[tokio::test]
    async fn port() {
        verify_encode_decode(Message::Port(1234)).await;
    }
}
