use std::io::{Error, ErrorKind, Result};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::codec::{AsyncDecoder, AsyncEncoder};
use crate::core::{PeerId, Sha1};

const PROTOCOL: &str = "BitTorrent protocol";

/// The handshake is a required message and must be the first message transmitted by the client. It
/// is (49+len(pstr)) bytes long.
///
/// _handshake: <pstrlen><pstr><reserved><info\_hash><peer\_id>_
///
/// * **pstrlen**: string length of <pstr>, as a single raw byte
/// * **pstr**: string identifier of the protocol
/// * **reserved**: eight (8) reserved bytes. All current implementations use all zeroes. Each bit
///   in these bytes can be used to change the behavior of the protocol. _An email from Bram
///   suggests that trailing bits should be used first, so that leading bits may be used to change
///   the meaning of trailing bits._
/// * **info\_hash**: 20-byte SHA1 hash of the info key in the metainfo file. This is the same
///   info\_hash that is transmitted in tracker requests.
/// * **peer\_id**: 20-byte string used as a unique ID for the client. This is usually the same
///   peer\_id that is transmitted in tracker requests (but not always e.g. an anonymity option in
///   Azureus).
///
/// In version 1.0 of the BitTorrent protocol, pstrlen = 19, and pstr = "BitTorrent protocol".
///
/// The initiator of a connection is expected to transmit their handshake immediately. The
/// recipient may wait for the initiator's handshake, if it is capable of serving multiple torrents
/// simultaneously (torrents are uniquely identified by their info_hash). However, the recipient
/// must respond as soon as it sees the info\_hash part of the handshake (the peer id will
/// presumably be sent after the recipient sends its own handshake). The tracker's NAT-checking
/// feature does not send the peer\_id field of the handshake._
///
/// If a client receives a handshake with an info\_hash that it is not currently serving, then the
/// client must drop the connection.
///
/// If the initiator of the connection receives a handshake in which the peer\_id does not match
/// the expected peer_id, then the initiator is expected to drop the connection._ Note that the
/// initiator presumably received the peer information from the tracker, which includes the
/// peer\_id that was registered by the peer. The peer\_id from the tracker and in the handshake
/// are expected to match.
#[derive(Debug, PartialEq, Clone)]
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

    pub fn is_standard_protocol(&self) -> bool {
        self.protocol == PROTOCOL
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
