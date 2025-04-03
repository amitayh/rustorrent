use std::fmt::Formatter;
use std::io::{Error, ErrorKind, Result};

use bit_set::BitSet;
use tokio_util::bytes::{Buf, BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use crate::codec::TransportMessage;
use crate::message::Block;
use crate::message::BlockData;

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

/// All of the remaining messages in the protocol take the form of <length prefix><message
/// ID><payload>. The length prefix is a four byte big-endian value. The message ID is a single
/// decimal byte. The payload is message dependent.
#[derive(PartialEq, Clone)]
pub enum Message {
    /// # keep-alive: <len=0000>
    ///
    /// The **keep-alive** message is a message with zero bytes, specified with the length prefix
    /// set to zero. There is no message ID and no payload. Peers may close a connection if they
    /// receive no messages (**keep-alive** or any other message) for a certain period of time, so
    /// a keep-alive message must be sent to maintain the connection *alive* if no command has been
    /// sent for a given amount of time. This amount of time is generally two minutes.
    KeepAlive,

    /// # choke: <len=0001><id=0>
    ///
    /// The **choke** message is fixed-length and has no payload.
    Choke,

    /// # unchoke: <len=0001><id=1>
    ///
    /// The **unchoke** message is fixed-length and has no payload.
    Unchoke,

    /// # interested: <len=0001><id=2>
    ///
    /// The **interested** message is fixed-length and has no payload.
    Interested,

    /// # not interested: <len=0001><id=3>
    ///
    /// The **not interested** message is fixed-length and has no payload.
    NotInterested,

    /// # have: <len=0005><id=4><piece index>
    ///
    /// The **have** message is fixed length. The payload is the zero-based index of a piece that
    /// has just been successfully downloaded and verified via the hash.
    Have(usize),

    /// # bitfield: <len=0001+X><id=5><bitfield>
    ///
    /// The **bitfield** message may only be sent immediately after the handshaking sequence is
    /// completed, and before any other messages are sent. It is optional, and need not be sent if
    /// a client has no pieces.
    ///
    /// The **bitfield** message is variable length, where X is the length of the bitfield. The
    /// payload is a bitfield representing the pieces that have been successfully downloaded. The
    /// high bit in the first byte corresponds to piece index 0. Bits that are cleared indicated a
    /// missing piece, and set bits indicate a valid and available piece. Spare bits at the end are
    /// set to zero.
    ///
    /// Some clients (Deluge for example) send **bitfield** with missing pieces even if it has all
    /// data. Then it sends rest of pieces as **have** messages. They are saying this helps against
    /// ISP filtering of BitTorrent protocol. It is called **lazy bitfield**.
    ///
    /// *A bitfield of the wrong length is considered an error. Clients should drop the connection
    /// if they receive bitfields that are not of the correct size, or if the bitfield has any of
    /// the spare bits set.*
    Bitfield(BitSet),

    /// # request: <len=0013><id=6><index><begin><length>
    ///
    /// The **request** message is fixed length, and is used to request a block. The payload
    /// contains the following information:
    ///
    /// * **index**: integer specifying the zero-based piece index
    /// * **begin**: integer specifying the zero-based byte offset within the piece
    /// * **length**: integer specifying the requested length.
    Request(Block),

    /// # piece: <len=0009+X><id=7><index><begin><block>
    ///
    /// The **piece** message is variable length, where X is the length of the block. The payload
    /// contains the following information:
    ///
    /// * **index**: integer specifying the zero-based piece index
    /// * **begin**: integer specifying the zero-based byte offset within the piece
    /// * **block**: block of data, which is a subset of the piece specified by index.
    Piece(BlockData),

    /// # cancel: <len=0013><id=8><index><begin><length>
    ///
    /// The **cancel** message is fixed length, and is used to cancel block requests. The payload
    /// is identical to that of the "request" message. It is typically used during "End Game" (see
    /// the Algorithms section below).
    Cancel(Block),

    /// # port: <len=0003><id=9><listen-port>
    ///
    /// The **port** message is sent by newer versions of the Mainline that implements a DHT
    /// tracker. The listen port is the port this peer's DHT node is listening on. This peer should
    /// be inserted in the local routing table (if DHT tracker is supported).
    Port(u16),
}

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
