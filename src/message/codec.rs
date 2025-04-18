use std::fmt::Formatter;
use std::io::{Error, ErrorKind, Result};

use bit_set::BitSet;
use tokio_util::bytes::{Buf, BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use crate::core::TransportMessage;
use crate::message::{Block, BlockData, Message};

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
const LENGTH_SIZE: usize = 4;

pub struct MessageCodec {
    max_length: usize,
}

impl MessageCodec {
    pub fn new(max_length: usize) -> Self {
        Self { max_length }
    }
}

impl Encoder<Message> for MessageCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: Message, dst: &mut BytesMut) -> Result<()> {
        dst.reserve(item.transport_bytes());
        match item {
            Message::KeepAlive => dst.put_u32(0),
            Message::Choke => {
                dst.put_u32(1);
                dst.put_u8(ID_CHOKE);
            }
            Message::Unchoke => {
                dst.put_u32(1);
                dst.put_u8(ID_UNCHOKE);
            }
            Message::Interested => {
                dst.put_u32(1);
                dst.put_u8(ID_INTERESTED);
            }
            Message::NotInterested => {
                dst.put_u32(1);
                dst.put_u8(ID_NOT_INTERESTED);
            }
            Message::Have(piece) => {
                dst.put_u32(5);
                dst.put_u8(ID_HAVE);
                dst.put_u32(piece as u32);
            }
            Message::Bitfield(bitset) => {
                let bytes = bitset.get_ref().to_bytes();
                dst.put_u32(1 + (bytes.len() as u32));
                dst.put_u8(ID_BITFIELD);
                dst.extend_from_slice(&bytes);
            }
            Message::Request(block) => {
                dst.put_u32(13);
                dst.put_u8(ID_REQUEST);
                encode_block(block, dst);
            }
            Message::Piece(BlockData {
                piece,
                offset,
                data,
            }) => {
                let length = 9 + data.len();
                dst.put_u32(length as u32);
                dst.put_u8(ID_PIECE);
                dst.put_u32(piece as u32);
                dst.put_u32(offset as u32);
                dst.extend_from_slice(&data);
            }
            Message::Cancel(block) => {
                dst.put_u32(13);
                dst.put_u8(ID_CANCEL);
                encode_block(block, dst);
            }
            Message::Port(port) => {
                dst.put_u32(3);
                dst.put_u8(ID_PORT);
                dst.put_u16(port);
            }
        }
        Ok(())
    }
}

fn encode_block(block: Block, dst: &mut BytesMut) {
    dst.put_u32(block.piece as u32);
    dst.put_u32(block.offset as u32);
    dst.put_u32(block.length as u32);
}

impl Decoder for MessageCodec {
    type Error = std::io::Error;
    type Item = Message;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>> {
        if src.len() < LENGTH_SIZE {
            // Not enough data to read length marker.
            return Ok(None);
        }

        let mut length_bytes = [0; LENGTH_SIZE];
        length_bytes.copy_from_slice(&src[0..LENGTH_SIZE]);
        let length = u32::from_be_bytes(length_bytes) as usize;

        if length == 0 {
            src.advance(LENGTH_SIZE);
            return Ok(Some(Message::KeepAlive));
        }

        if length > self.max_length {
            src.advance(LENGTH_SIZE);
            return Err(Error::new(
                ErrorKind::Other,
                format!(
                    "message length {} exceeds maximum of {}",
                    length, self.max_length
                ),
            ));
        }

        if src.len() < LENGTH_SIZE + length {
            src.reserve(LENGTH_SIZE + length - src.len());
            return Ok(None);
        }

        src.advance(LENGTH_SIZE);
        let id = src.get_u8();
        match (id, length) {
            (ID_CHOKE, 1) => Ok(Some(Message::Choke)),
            (ID_UNCHOKE, 1) => Ok(Some(Message::Unchoke)),
            (ID_INTERESTED, 1) => Ok(Some(Message::Interested)),
            (ID_NOT_INTERESTED, 1) => Ok(Some(Message::NotInterested)),
            (ID_HAVE, 5) => {
                let piece = src.get_u32() as usize;
                Ok(Some(Message::Have(piece)))
            }
            (ID_BITFIELD, 1..) => {
                let bitset_length = length - 1;
                let bitset = BitSet::from_bytes(&src[0..bitset_length]);
                src.advance(bitset_length);
                Ok(Some(Message::Bitfield(bitset)))
            }
            (ID_REQUEST, 13) => {
                let block = decode_block(src);
                Ok(Some(Message::Request(block)))
            }
            (ID_PIECE, 9..) => {
                let piece = src.get_u32() as usize;
                let offset = src.get_u32() as usize;
                let data_length = length - 9;
                let data = src[0..data_length].to_vec();
                src.advance(data_length);
                Ok(Some(Message::Piece(BlockData {
                    piece,
                    offset,
                    data,
                })))
            }
            (ID_CANCEL, 13) => {
                let block = decode_block(src);
                Ok(Some(Message::Cancel(block)))
            }
            (ID_PORT, 3) => {
                let port = src.get_u16();
                Ok(Some(Message::Port(port)))
            }
            _ => Err(Error::new(
                ErrorKind::InvalidData,
                format!("invalid message id {} with length {}", id, length),
            )),
        }
    }
}

fn decode_block(src: &mut BytesMut) -> Block {
    let piece = src.get_u32() as usize;
    let offset = src.get_u32() as usize;
    let length = src.get_u32() as usize;
    Block::new(piece, offset, length)
}

impl std::fmt::Debug for Message {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Message::KeepAlive => write!(f, "KeepAlive"),
            Message::Choke => write!(f, "Choke"),
            Message::Unchoke => write!(f, "Unchoke"),
            Message::Interested => write!(f, "Interested"),
            Message::NotInterested => write!(f, "NotInterested"),
            Message::Have(piece) => write!(f, "Have {{ piece: {} }}", piece),
            Message::Bitfield(bitset) => write!(f, "Bitfield(<{} pieces>)", bitset.len()),
            Message::Request(block) => write!(f, "Request({:?})", block),
            Message::Piece(block) => {
                write!(
                    f,
                    "Piece {{ piece: {}, offset: {}, data: <{} bytes> }}",
                    block.piece,
                    block.offset,
                    block.data.len()
                )
            }
            Message::Cancel(block) => write!(f, "Cancel({:?})", block),
            Message::Port(port) => write!(f, "Port({})", port),
        }
    }
}

impl TransportMessage for Message {
    fn transport_bytes(&self) -> usize {
        let payload_size = match self {
            Self::KeepAlive => 0,
            Self::Choke => 1,
            Self::Unchoke => 1,
            Self::Interested => 1,
            Self::NotInterested => 1,
            Self::Have(_) => 5,
            Self::Bitfield(bitset) => 1 + bitset.get_ref().to_bytes().len(),
            Self::Request(_) => 13,
            Self::Piece(block) => 9 + block.data.len(),
            Self::Cancel(_) => 13,
            Self::Port(_) => 3,
        };
        LENGTH_SIZE + payload_size
    }
}
