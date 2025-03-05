use std::io::{Error, ErrorKind, Result};

use bit_set::BitSet;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::codec::{Decoder, Encoder};
use crate::crypto::Sha1;
use crate::peer::PeerId;

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

impl Decoder for Handshake {
    async fn decode<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Self> {
        let protocol = {
            let length = stream.read_u8().await? as usize;
            let mut buf = vec![0; length];
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

impl Encoder for Handshake {
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
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
pub enum Message {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have { piece: usize },
    Bitfield(BitSet),
    Request(Block),
    Piece(Block),
    Cancel(Block),
    Port(u16),
}

impl Decoder for Message {
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
                let block = Block::read(stream).await?;
                Ok(Self::Request(block))
            }
            (ID_PIECE, 9..) => {
                let piece_index = stream.read_u32().await? as usize;
                let offset = stream.read_u32().await? as usize;
                Ok(Self::Piece(Block {
                    piece: piece_index,
                    offset,
                    length: length - 9,
                }))
            }
            (ID_CANCEL, 13) => {
                let block = Block::read(stream).await?;
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

#[derive(Debug, PartialEq)]
pub struct Block {
    pub piece: usize,
    pub offset: usize,
    pub length: usize,
}

impl Block {
    async fn read<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Block> {
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

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;

    use super::*;

    #[tokio::test]
    async fn handshake() {
        let (mut client, mut server) = tokio::io::duplex(128);
        let message_written = Handshake::new(Sha1([1; 20]), PeerId([2; 20]));
        message_written.encode(&mut server).await.unwrap();

        let message_read = Handshake::decode(&mut client).await.unwrap();
        assert_eq!(message_read, message_written);
    }

    #[tokio::test]
    async fn keep_alive() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(0).await.unwrap(); // length

        let message = Message::decode(&mut client).await.unwrap();
        assert_eq!(message, Message::KeepAlive);
    }

    #[tokio::test]
    async fn choke() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(1).await.unwrap(); // length
        server.write_u8(ID_CHOKE).await.unwrap(); // id

        let message = Message::decode(&mut client).await.unwrap();
        assert_eq!(message, Message::Choke);
    }

    #[tokio::test]
    async fn unchoke() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(1).await.unwrap(); // length
        server.write_u8(ID_UNCHOKE).await.unwrap(); // id

        let message = Message::decode(&mut client).await.unwrap();
        assert_eq!(message, Message::Unchoke);
    }

    #[tokio::test]
    async fn interested() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(1).await.unwrap(); // length
        server.write_u8(ID_INTERESTED).await.unwrap(); // id

        let message = Message::decode(&mut client).await.unwrap();
        assert_eq!(message, Message::Interested);
    }

    #[tokio::test]
    async fn not_interested() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(1).await.unwrap(); // length
        server.write_u8(ID_NOT_INTERESTED).await.unwrap(); // id

        let message = Message::decode(&mut client).await.unwrap();
        assert_eq!(message, Message::NotInterested);
    }

    #[tokio::test]
    async fn have() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(5).await.unwrap(); // length
        server.write_u8(ID_HAVE).await.unwrap(); // id
        server.write_u32(1234).await.unwrap(); // piece index

        let message = Message::decode(&mut client).await.unwrap();
        assert_eq!(message, Message::Have { piece: 1234 });
    }

    #[tokio::test]
    async fn bitfield() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(2).await.unwrap(); // length
        server.write_u8(ID_BITFIELD).await.unwrap(); // id
        server.write_u8(0b11010000).await.unwrap(); // bit mask

        let message = Message::decode(&mut client).await.unwrap();
        assert_eq!(
            message,
            Message::Bitfield(BitSet::from_bytes(&[0b11010000]))
        );
    }

    #[tokio::test]
    async fn request() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(13).await.unwrap(); // length
        server.write_u8(ID_REQUEST).await.unwrap(); // id
        server.write_u32(1).await.unwrap(); // index
        server.write_u32(2).await.unwrap(); // begin
        server.write_u32(3).await.unwrap(); // length

        let message = Message::decode(&mut client).await.unwrap();
        assert_eq!(
            message,
            Message::Request(Block {
                piece: 1,
                offset: 2,
                length: 3
            })
        );
    }

    #[tokio::test]
    async fn piece() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(14).await.unwrap(); // length
        server.write_u8(ID_PIECE).await.unwrap(); // id
        server.write_u32(1).await.unwrap(); // index
        server.write_u32(2).await.unwrap(); // begin
        server.write_all("hello".as_bytes()).await.unwrap(); // data

        let message = Message::decode(&mut client).await.unwrap();
        assert_eq!(
            message,
            Message::Piece(Block {
                piece: 1,
                offset: 2,
                length: 5
            })
        );

        let mut buf = [0; 5];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, "hello".as_bytes());
    }

    #[tokio::test]
    async fn cancel() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(13).await.unwrap(); // length
        server.write_u8(ID_CANCEL).await.unwrap(); // id
        server.write_u32(1).await.unwrap(); // index
        server.write_u32(2).await.unwrap(); // begin
        server.write_u32(3).await.unwrap(); // length

        let message = Message::decode(&mut client).await.unwrap();
        assert_eq!(
            message,
            Message::Cancel(Block {
                piece: 1,
                offset: 2,
                length: 3
            })
        );
    }

    #[tokio::test]
    async fn port() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(3).await.unwrap(); // length
        server.write_u8(ID_PORT).await.unwrap(); // id
        server.write_u16(1234).await.unwrap(); // port

        let message = Message::decode(&mut client).await.unwrap();
        assert_eq!(message, Message::Port(1234));
    }
}
