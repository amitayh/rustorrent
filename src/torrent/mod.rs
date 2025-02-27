/*
use std::path::PathBuf;

use crate::bencoding::value::Value;

// https://wiki.theory.org/BitTorrentSpecification#Byte_Strings

#[derive(Debug, PartialEq)]
pub struct Torrent {
    // The URL of the tracker.
    // TODO: use a real URL type?
    announce: String,
    info: Info,
}

/*
impl TryFrom<Value> for Torrent {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let mut entries = value.get_dictionary().unwrap();
        let announce = entries
            .remove("announce")
            .and_then(Value::get_string)
            .unwrap();

        let info = entries.remove("info").unwrap();
        return Ok(Torrent {
            announce,
            info: Info::try_from(info).unwrap(),
        });
    }
}
*/

type Md5 = [u8; 16];

type Sha1 = [u8; 20];

#[derive(Debug, PartialEq)]
pub struct Info {
    info_hash: Sha1,

    // piece length maps to the number of bytes in each piece the file is split into. For the
    // purposes of transfer, files are split into fixed-size pieces which are all the same length
    // except for possibly the last one which may be truncated. piece length is almost always a
    // power of two, most commonly 2 18 = 256 K (BitTorrent prior to version 3.2 uses 2 20 = 1 M as
    // default).
    piece_length: usize,

    pieces: Vec<Sha1>,

    download_type: DownloadType,
}

/*
impl TryFrom<Value> for Info {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
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
    }
}
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
    path: PathBuf,
    length: usize,
    mdsum: Option<Md5>,
}

/*
#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn valid_torrent_metainfo() {
        let piece1 = [1; 20];
        let piece2 = [2; 20];
        let mut pieces = String::new();
        pieces.reserve(40);
        pieces.extend(std::str::from_utf8(&piece1));
        pieces.extend(std::str::from_utf8(&piece2));

        let contents = Value::Dictionary(HashMap::from([
            (
                "announce".to_string(),
                Value::string("udp://tracker.opentrackr.org:1337/announce"),
            ),
            (
                "info".to_string(),
                Value::Dictionary(HashMap::from([
                    ("piece length".to_string(), Value::Integer(1234)),
                    ("pieces".to_string(), Value::String(pieces)),
                ])),
            ),
        ]));

        assert_eq!(
            Torrent::try_from(contents),
            Ok(Torrent {
                announce: "udp://tracker.opentrackr.org:1337/announce".to_string(),
                info: Info {
                    info_hash: [0; 20],
                    piece_length: 1234,
                    pieces: vec![piece1, piece2],
                    download_type: DownloadType::MultiFile {
                        directory_name: "".to_string(),
                        files: vec![]
                    }
                }
            })
        );
    }
}
*/
*/
