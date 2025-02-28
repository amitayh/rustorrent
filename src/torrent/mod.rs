use std::path::PathBuf;

use anyhow::{Error, Result, anyhow};
use sha1::Digest;

use crate::bencoding::value::{TypeMismatch, Value, ValueType};

// https://wiki.theory.org/BitTorrentSpecification#Byte_Strings

#[derive(Debug, PartialEq)]
pub struct Torrent {
    // The URL of the tracker.
    // TODO: use a real URL type?
    pub announce: String,
    pub info: Info,
}

impl TryFrom<Value> for Torrent {
    type Error = Error;

    fn try_from(mut value: Value) -> Result<Self> {
        let announce = value.remove_entry("announce")?.try_into()?;
        let info = value.remove_entry("info")?.try_into()?;
        Ok(Torrent { announce, info })
    }
}

impl TryFrom<Value> for Vec<u8> {
    type Error = Error;

    fn try_from(value: Value) -> Result<Self> {
        match value {
            Value::String(bytes) => Ok(bytes),
            _ => Err(Error::new(TypeMismatch::new(ValueType::String, value))),
        }
    }
}

impl TryFrom<Value> for String {
    type Error = Error;

    fn try_from(value: Value) -> Result<Self> {
        match value {
            Value::String(bytes) => String::from_utf8(bytes).map_err(Error::new),
            _ => Err(Error::new(TypeMismatch::new(ValueType::String, value))),
        }
    }
}

impl TryFrom<Value> for usize {
    type Error = Error;

    fn try_from(value: Value) -> Result<Self> {
        match value {
            Value::Integer(integer) => integer.try_into().map_err(Error::new),
            _ => Err(Error::new(TypeMismatch::new(ValueType::Integer, value))),
        }
    }
}

impl TryFrom<&Value> for Sha1 {
    type Error = Error;

    fn try_from(value: &Value) -> Result<Self> {
        let mut hasher = sha1::Sha1::new();
        value.encode(&mut hasher)?;
        let sha1 = hasher.finalize();
        Ok(Sha1(sha1.into()))
    }
}

impl Value {
    fn remove_entry(&mut self, key: &str) -> Result<Value> {
        match self {
            Value::Dictionary(entries) => {
                entries.remove(key).ok_or(anyhow!("key missing: {:?}", key))
            }
            _ => Err(Error::new(TypeMismatch::new(
                ValueType::Dictionary,
                self.clone(),
            ))),
        }
    }
}

type Md5 = [u8; 16];

#[derive(PartialEq)]
pub struct Sha1([u8; 20]);

impl std::fmt::Debug for Sha1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sha1(")?;
        for byte in self.0 {
            write!(f, "{:02x}", byte)?;
        }
        write!(f, ")")
    }
}

#[derive(Debug, PartialEq)]
pub struct Info {
    pub info_hash: Sha1,

    // piece length maps to the number of bytes in each piece the file is split into. For the
    // purposes of transfer, files are split into fixed-size pieces which are all the same length
    // except for possibly the last one which may be truncated. piece length is almost always a
    // power of two, most commonly 2 18 = 256 K (BitTorrent prior to version 3.2 uses 2 20 = 1 M as
    // default).
    pub piece_length: usize,
    pub pieces: Vec<Sha1>,
    //pub download_type: DownloadType,
}

impl Info {
    fn build_pieces(pieces: &[u8]) -> Result<Vec<Sha1>> {
        if pieces.len() % 20 != 0 {
            return Err(anyhow!(
                "invalid length {}. must be a multiple of 20",
                pieces.len()
            ));
        }
        let mut all = Vec::with_capacity(pieces.len() / 20);
        for i in (0..pieces.len()).step_by(20) {
            let mut sha = [0; 20];
            sha.copy_from_slice(&pieces[i..(i + 20)]);
            all.push(Sha1(sha));
        }
        Ok(all)
    }
}

impl TryFrom<Value> for Info {
    type Error = Error;

    fn try_from(mut value: Value) -> Result<Self> {
        let info_hash = Sha1::try_from(&value)?;
        let piece_length = value.remove_entry("piece length")?.try_into()?;
        let pieces: Vec<_> = value.remove_entry("pieces")?.try_into()?;
        let pieces = Info::build_pieces(&pieces)?;
        Ok(Info {
            info_hash,
            piece_length,
            pieces,
        })
    }
}

#[allow(dead_code)]
#[derive(Debug, PartialEq)]
pub enum DownloadType {
    SingleFile {
        name: String,
        length: usize,
        md5sum: Option<Md5>,
    },
    MultiFile {
        directory_name: String,
        files: Vec<File>,
    },
}

#[derive(Debug, PartialEq)]
pub struct File {
    path: PathBuf,
    length: usize,
    mdsum: Option<Md5>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_torrent_metainfo() {
        let piece1 = [1; 20];
        let piece2 = [2; 20];
        let mut pieces = Vec::with_capacity(40);
        pieces.extend_from_slice(&piece1);
        pieces.extend_from_slice(&piece2);

        let metainfo = Value::dictionary()
            .with_entry(
                "announce",
                Value::string("udp://tracker.opentrackr.org:1337/announce"),
            )
            .with_entry(
                "info",
                Value::dictionary()
                    .with_entry("piece length", Value::Integer(1234))
                    .with_entry("pieces", Value::String(pieces)),
            );

        let torrent = Torrent::try_from(metainfo).expect("invalid metainfo");

        assert_eq!(
            torrent.announce,
            "udp://tracker.opentrackr.org:1337/announce".to_string()
        );
        assert_eq!(torrent.info.piece_length, 1234);
        assert_eq!(torrent.info.pieces, vec![Sha1(piece1), Sha1(piece2)]);
    }
}
