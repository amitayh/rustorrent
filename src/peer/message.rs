use std::io::Error;
use std::io::ErrorKind;
use std::io::Result;

use bit_set::BitSet;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;

const ID_CHOKE: u8 = b'0';
const ID_UNCHOKE: u8 = b'1';
const ID_INTERESTED: u8 = b'2';
const ID_NOT_INTERESTED: u8 = b'3';
const ID_HAVE: u8 = b'4';
const ID_BITFIELD: u8 = b'5';
const ID_REQUEST: u8 = b'6';
const ID_PIECE: u8 = b'7';
const ID_CANCEL: u8 = b'8';
const ID_PORT: u8 = b'9';

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

impl Message {
    pub async fn read<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Self> {
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
    async fn keep_alive() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(0).await.unwrap(); // length

        let message = Message::read(&mut client).await.unwrap();
        assert_eq!(message, Message::KeepAlive);
    }

    #[tokio::test]
    async fn choke() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(1).await.unwrap(); // length
        server.write_u8(ID_CHOKE).await.unwrap(); // id

        let message = Message::read(&mut client).await.unwrap();
        assert_eq!(message, Message::Choke);
    }

    #[tokio::test]
    async fn unchoke() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(1).await.unwrap(); // length
        server.write_u8(ID_UNCHOKE).await.unwrap(); // id

        let message = Message::read(&mut client).await.unwrap();
        assert_eq!(message, Message::Unchoke);
    }

    #[tokio::test]
    async fn interested() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(1).await.unwrap(); // length
        server.write_u8(ID_INTERESTED).await.unwrap(); // id

        let message = Message::read(&mut client).await.unwrap();
        assert_eq!(message, Message::Interested);
    }

    #[tokio::test]
    async fn not_interested() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(1).await.unwrap(); // length
        server.write_u8(ID_NOT_INTERESTED).await.unwrap(); // id

        let message = Message::read(&mut client).await.unwrap();
        assert_eq!(message, Message::NotInterested);
    }

    #[tokio::test]
    async fn have() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(5).await.unwrap(); // length
        server.write_u8(ID_HAVE).await.unwrap(); // id
        server.write_u32(1234).await.unwrap(); // piece index

        let message = Message::read(&mut client).await.unwrap();
        assert_eq!(message, Message::Have { piece: 1234 });
    }

    #[tokio::test]
    async fn bitfield() {
        let (mut client, mut server) = tokio::io::duplex(64);
        server.write_u32(2).await.unwrap(); // length
        server.write_u8(ID_BITFIELD).await.unwrap(); // id
        server.write_u8(0b11010000).await.unwrap(); // bit mask

        let message = Message::read(&mut client).await.unwrap();
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

        let message = Message::read(&mut client).await.unwrap();
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

        let message = Message::read(&mut client).await.unwrap();
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

        let message = Message::read(&mut client).await.unwrap();
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

        let message = Message::read(&mut client).await.unwrap();
        assert_eq!(message, Message::Port(1234));
    }
}
