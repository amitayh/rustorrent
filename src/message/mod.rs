mod block;
mod handshake;
mod message;

pub use block::*;
pub use handshake::*;
pub use message::*;

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Seek};

    use bit_set::BitSet;
    use futures::{SinkExt, StreamExt};
    use tokio_util::codec::Framed;

    use crate::codec::{AsyncDecoder, AsyncEncoder, TransportMessage};
    use crate::{crypto::Sha1, peer::PeerId};

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
