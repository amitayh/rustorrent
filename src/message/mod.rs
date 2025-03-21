mod block;
mod handshake;
mod message;

pub use block::*;
pub use handshake::*;
pub use message::*;

#[cfg(test)]
mod tests {
    use std::{
        fmt::Debug,
        io::{Cursor, Seek},
    };

    use bit_set::BitSet;

    use crate::codec::{AsyncDecoder, AsyncEncoder, TransportMessage};
    use crate::{crypto::Sha1, peer::peer_id::PeerId};

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
