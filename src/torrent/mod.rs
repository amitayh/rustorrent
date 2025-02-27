// use std::path::PathBuf;

use crate::bencoding::value::Value;

// https://wiki.theory.org/BitTorrentSpecification#Byte_Strings

#[derive(Debug, PartialEq)]
pub struct Torrent {
    // The URL of the tracker.
    // TODO: use a real URL type?
    announce: String,
    info: Info,
}

impl TryFrom<Value> for Torrent {
    type Error = String;

    fn try_from(mut value: Value) -> Result<Self, Self::Error> {
        let announce = value.remove_entry("announce")?.try_into()?;
        let info = value.remove_entry("info")?.try_into()?;
        return Ok(Torrent { announce, info });
    }
}

impl TryFrom<Value> for Vec<u8> {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::String(bytes) => Ok(bytes),
            _ => Err("type mismatch".to_string()),
        }
    }
}

impl TryFrom<Value> for String {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::String(bytes) => match String::from_utf8(bytes) {
                Ok(string) => Ok(string),
                Err(_) => Err("not valid utf8".to_string()),
            },
            _ => Err("type mismatch".to_string()),
        }
    }
}

impl TryFrom<Value> for usize {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Integer(integer) => match integer.try_into() {
                Ok(usize) => Ok(usize),
                Err(_) => Err("not valid usize".to_string()),
            },
            _ => Err("type mismatch".to_string()),
        }
    }
}

impl Value {
    fn remove_entry(&mut self, key: &str) -> Result<Value, String> {
        match self {
            Value::Dictionary(entries) => {
                entries.remove(key).ok_or(format!("key missing: {:?}", key))
            }
            _ => Err("type mismatch".to_string()),
        }
    }
}

type Md5 = [u8; 16];

#[derive(PartialEq)]
pub struct Sha1([u8; 20]);

impl std::fmt::Debug for Sha1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hex: String = self.0.iter().map(|byte| format!("{:02x}", byte)).collect();
        write!(f, "Sha1({})", hex)
    }
}

#[derive(Debug, PartialEq)]
pub struct Info {
    //info_hash: Sha1,

    // piece length maps to the number of bytes in each piece the file is split into. For the
    // purposes of transfer, files are split into fixed-size pieces which are all the same length
    // except for possibly the last one which may be truncated. piece length is almost always a
    // power of two, most commonly 2 18 = 256 K (BitTorrent prior to version 3.2 uses 2 20 = 1 M as
    // default).
    piece_length: usize,
    pieces: Vec<Sha1>,
    //download_type: DownloadType,
}

impl Info {
    fn build_pieces(pieces: &[u8]) -> Result<Vec<Sha1>, String> {
        if pieces.len() % 20 != 0 {
            return Err("invalid length".to_string());
        }
        let mut all = Vec::new();
        all.reserve(pieces.len() / 20);
        for i in (0..pieces.len()).step_by(20) {
            let mut sha = [0; 20];
            sha.copy_from_slice(&pieces[i..(i + 20)]);
            all.push(Sha1(sha));
        }
        Ok(all)
    }
}

impl TryFrom<Value> for Info {
    type Error = String;

    fn try_from(mut value: Value) -> Result<Self, Self::Error> {
        let piece_length = value.remove_entry("piece length")?.try_into()?;
        let pieces: Vec<u8> = value.remove_entry("pieces")?.try_into()?;
        let pieces = Info::build_pieces(&pieces)?;
        Ok(Info {
            piece_length,
            pieces,
        })
    }
}
/*
        let mut entries = value.get_dictionary().unwrap();
        let piece_length = entries
            .remove("piece length")
            .unwrap()
            .get_integer()
            .unwrap()
            .try_into()
            .unwrap();

        let pieces = entries.get("pieces").unwrap().get_str().unwrap();
        assert!(pieces.len() % 20 == 0, "Invalid length");
        let mut all = Vec::new();
        all.reserve(pieces.len() / 20);
        for i in (0..pieces.len()).step_by(20) {
            let mut sha = [0; 20];
            sha.copy_from_slice(pieces[i..(i + 20)].as_bytes());
            all.push(sha);
        }

        Ok(Info {
            info_hash: [0; 20],
            piece_length,
            pieces: all,
            download_type: DownloadType::MultiFile {
                directory_name: "".to_string(),
                files: vec![],
            },
        })
*/

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
    //path: PathBuf,
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
        let mut pieces = Vec::new();
        pieces.reserve(40);
        pieces.extend_from_slice(&piece1);
        pieces.extend_from_slice(&piece2);

        let contents = Value::dictionary()
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

        assert_eq!(
            Torrent::try_from(contents),
            Ok(Torrent {
                announce: "udp://tracker.opentrackr.org:1337/announce".to_string(),
                info: Info {
                    //    info_hash: [0; 20],
                    piece_length: 1234,
                    pieces: vec![Sha1(piece1), Sha1(piece2)],
                    //    download_type: DownloadType::MultiFile {
                    //        directory_name: "".to_string(),
                    //        files: vec![]
                    //    }
                }
            })
        );
    }
}
