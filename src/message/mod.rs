mod block;
mod codec;
mod handshake;

use bit_set::BitSet;

pub use block::*;
pub use codec::*;
pub use handshake::*;

/// All of the remaining messages in the protocol take the form of <length prefix><message
/// ID><payload>. The length prefix is a four byte big-endian value. The message ID is a single
/// decimal byte. The payload is message dependent.
#[derive(PartialEq, Eq, Clone)]
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

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Seek};

    use bit_set::BitSet;
    use futures::{SinkExt, StreamExt};
    use tokio_util::codec::Framed;

    use crate::core::{AsyncDecoder, AsyncEncoder, PeerId, Sha1, TransportMessage};

    use super::*;

    async fn verify_encode_decode(message: Message) {
        let length = message.transport_bytes();
        let cursor = Cursor::new(Vec::with_capacity(length));
        let mut framed = Framed::new(cursor, MessageCodec::new(16));

        framed.send(message.clone()).await.expect("unable to write");
        assert_eq!(length, framed.get_ref().position() as usize);
        framed.get_mut().rewind().expect("unable to rewind");
        let message_read = framed.next().await.expect("empty").expect("error");
        assert_eq!(message_read, message);
    }

    #[tokio::test]
    async fn handshake() {
        let handshake = Handshake::new(Sha1([1; 20]), PeerId([2; 20]));

        let length = handshake.transport_bytes();
        let mut cursor = Cursor::new(Vec::with_capacity(length));

        handshake
            .encode(&mut cursor)
            .await
            .expect("unable to encode");

        assert_eq!(length, cursor.position() as usize);
        cursor.rewind().expect("unable to rewind");

        let handshake_read = Handshake::decode(&mut cursor)
            .await
            .expect("unable to decode");

        assert_eq!(handshake, handshake_read);
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
        verify_encode_decode(Message::Have(1234)).await;
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
        verify_encode_decode(Message::Piece(BlockData {
            piece: 1,
            offset: 2,
            data: vec![1, 2, 3],
        }))
        .await;
    }

    #[tokio::test]
    async fn verify_max_length() {
        let message = Message::Piece(BlockData {
            piece: 1,
            offset: 2,
            data: vec![1, 2, 3],
        });
        let length = message.transport_bytes();
        let cursor = Cursor::new(Vec::with_capacity(length));
        let mut framed = Framed::new(cursor, MessageCodec::new(5));

        framed.send(message.clone()).await.expect("unable to write");
        framed.get_mut().rewind().expect("unable to rewind");
        let message_read = framed.next().await.expect("empty");
        assert!(message_read.is_err());
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
