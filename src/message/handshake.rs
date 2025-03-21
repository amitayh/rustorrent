use std::io::{Error, ErrorKind, Result};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::codec::{AsyncDecoder, AsyncEncoder};
use crate::crypto::Sha1;
use crate::peer::peer_id::PeerId;

const PROTOCOL: &str = "BitTorrent protocol";

#[derive(Debug, PartialEq)]
pub struct Handshake {
    pub protocol: String,
    pub info_hash: Sha1,
    pub peer_id: PeerId,
}

impl Handshake {
    pub fn new(info_hash: Sha1, peer_id: PeerId) -> Self {
        Self {
            protocol: PROTOCOL.to_string(),
            info_hash,
            peer_id,
        }
    }
}

impl AsyncDecoder for Handshake {
    async fn decode<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Self> {
        let protocol = {
            let length = stream.read_u8().await?;
            let mut buf = vec![0; length as usize];
            stream.read_exact(&mut buf).await?;
            String::from_utf8(buf).map_err(|err| Error::new(ErrorKind::InvalidData, err))?
        };
        {
            // Skip 8 reserved bytes
            let mut buf = [0; 8];
            stream.read_exact(&mut buf).await?;
        };
        let info_hash = {
            let mut buf = [0; 20];
            stream.read_exact(&mut buf).await?;
            Sha1(buf)
        };
        let peer_id = {
            let mut buf = [0; 20];
            stream.read_exact(&mut buf).await?;
            PeerId(buf)
        };
        Ok(Handshake {
            protocol,
            info_hash,
            peer_id,
        })
    }
}

impl AsyncEncoder for Handshake {
    async fn encode<S: AsyncWrite + Unpin>(&self, stream: &mut S) -> Result<()> {
        let len = self
            .protocol
            .len()
            .try_into()
            .expect("protocol string too long");
        stream.write_u8(len).await?;
        stream.write_all(self.protocol.as_bytes()).await?;
        stream.write_all(&[0; 8]).await?;
        stream.write_all(&self.info_hash.0).await?;
        stream.write_all(&self.peer_id.0).await?;
        stream.flush().await?;
        Ok(())
    }
}

impl crate::codec::TransportMessage for Handshake {
    fn transport_bytes(&self) -> usize {
        1 + // pstr len
            self.protocol.bytes().len() + // pstr bytes
            8 + // reserved
            20 + // info hash
            20 // peer id
    }
}
