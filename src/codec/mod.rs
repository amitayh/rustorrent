use std::io::Result;

use tokio::io::{AsyncRead, AsyncWrite};

pub trait Encoder {
    async fn encode<S: AsyncWrite + Unpin>(&self, stream: &mut S) -> Result<()>;
}

pub trait Decoder: Sized {
    async fn decode<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Self>;
}
