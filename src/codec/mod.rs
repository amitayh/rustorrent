use std::io::{Result, Write};

use size::Size;
use tokio::io::{AsyncRead, AsyncWrite};

pub trait Encoder {
    fn encode(&self, out: &mut impl Write) -> Result<()>;
}

pub trait AsyncEncoder {
    async fn encode<S: AsyncWrite + Unpin>(&self, stream: &mut S) -> Result<()>;
}

pub trait AsyncDecoder: Sized {
    async fn decode<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Self>;
}

pub trait TransportMessage {
    fn transport_bytes(&self) -> usize;
}
