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
        io::{Cursor, Read, Seek},
    };

    use bit_set::BitSet;
    use futures::{SinkExt, StreamExt};
    use tokio_util::codec::{FramedRead, FramedWrite};

    use crate::codec::{AsyncDecoder, AsyncEncoder, TransportMessage};
    use crate::{crypto::Sha1, peer::PeerId};

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

    async fn verify_encode_decode2(message: Message) {
        let (input, output) = tokio::io::duplex(message.transport_bytes() * 2);

        let mut writer = FramedWrite::new(input, MessageCodec);
        writer.send(message.clone()).await.expect("unable to write");
        writer.send(message.clone()).await.expect("unable to write");

        let mut reader = FramedRead::new(output, MessageCodec);
        let message1 = reader.next().await.expect("empty").expect("error");
        assert_eq!(message1, message);

        let message2 = reader.next().await.expect("empty").expect("error");
        assert_eq!(message2, message);
    }

    #[tokio::test]
    async fn handshake() {
        verify_encode_decode(Handshake::new(Sha1([1; 20]), PeerId([2; 20]))).await;
    }

    #[tokio::test]
    async fn keep_alive() {
        verify_encode_decode2(Message::KeepAlive).await;
    }

    #[tokio::test]
    async fn choke() {
        verify_encode_decode2(Message::Choke).await;
    }

    #[tokio::test]
    async fn unchoke() {
        verify_encode_decode2(Message::Unchoke).await;
    }

    #[tokio::test]
    async fn interested() {
        verify_encode_decode2(Message::Interested).await;
    }

    #[tokio::test]
    async fn not_interested() {
        verify_encode_decode2(Message::NotInterested).await;
    }

    #[tokio::test]
    async fn have() {
        verify_encode_decode2(Message::Have(1234)).await;
    }

    #[tokio::test]
    async fn bitfield() {
        verify_encode_decode2(Message::Bitfield(BitSet::from_bytes(&[0b11010000]))).await;
    }

    #[tokio::test]
    async fn request() {
        verify_encode_decode2(Message::Request(Block::new(1, 2, 3))).await;
    }

    #[tokio::test]
    async fn piece() {
        verify_encode_decode2(Message::Piece(BlockData {
            piece: 1,
            offset: 2,
            data: vec![1, 2, 3],
        }))
        .await;
    }

    #[tokio::test]
    async fn cancel() {
        verify_encode_decode2(Message::Cancel(Block::new(1, 2, 3))).await;
    }

    #[tokio::test]
    async fn port() {
        verify_encode_decode2(Message::Port(1234)).await;
    }
}
