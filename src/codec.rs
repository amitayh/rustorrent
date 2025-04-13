use std::io::{Result, Write};

use tokio::io::{AsyncRead, AsyncWrite};

/// A trait for messages that can be encoded into a stream.
pub trait Encoder {
    /// Encodes a message into a stream.
    fn encode(&self, out: &mut impl Write) -> Result<()>;
}

/// A trait for messages that can be encoded into an async stream.
pub trait AsyncEncoder {
    /// Encodes a message into an async stream.
    async fn encode<S: AsyncWrite + Unpin>(&self, stream: &mut S) -> Result<()>;
}

/// A trait for messages that can be decoded from a stream.
pub trait AsyncDecoder: Sized {
    /// Decodes a message from a stream.
    async fn decode<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Self>;
}

/// A trait for messages that can be transported over a network connection.
pub trait TransportMessage {
    /// Returns the total number of bytes needed to transport this message,
    /// including any length prefixes, message IDs, and payload data.
    fn transport_bytes(&self) -> usize;
}
