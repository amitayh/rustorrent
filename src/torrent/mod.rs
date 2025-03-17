use std::{
    io::Write,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use anyhow::{Error, Result, anyhow};
use sha1::Digest;
use size::{KiB, Size};
use tokio::io::AsyncReadExt;
use url::Url;

use crate::{
    bencoding::value::Value,
    crypto::{Md5, Sha1},
};

// https://wiki.theory.org/BitTorrentSpecification#Byte_Strings

const SHA1_LEN: usize = 20;

#[derive(Debug, PartialEq)]
pub struct Torrent {
    pub announce: Url,
    pub info: Info,
}

impl TryFrom<Value> for Torrent {
    type Error = Error;

    fn try_from(mut value: Value) -> Result<Self> {
        let announce: String = value.remove_entry("announce")?.try_into()?;
        let announce = Url::parse(&announce)?;
        let info = value.remove_entry("info")?.try_into()?;
        Ok(Torrent { announce, info })
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct Info {
    pub info_hash: Sha1,
    pub piece_length: Size,
    pub pieces: Vec<Sha1>,
    pub download_type: DownloadType,
}

impl Info {
    fn build_pieces(pieces: &[u8]) -> Result<Vec<Sha1>> {
        if pieces.len() % SHA1_LEN != 0 {
            return Err(anyhow!(
                "invalid length {}. must be a multiple of SHA1_LEN",
                pieces.len()
            ));
        }
        let mut all = Vec::with_capacity(pieces.len() / SHA1_LEN);
        for i in (0..pieces.len()).step_by(SHA1_LEN) {
            let mut bytes = [0; SHA1_LEN];
            bytes.copy_from_slice(&pieces[i..(i + SHA1_LEN)]);
            all.push(Sha1(bytes));
        }
        Ok(all)
    }

    #[allow(dead_code)]
    pub async fn load(path: impl AsRef<Path>) -> Result<Info> {
        let mut file = tokio::fs::File::open(&path).await?;
        let file_size = file.metadata().await?.len();
        // TODO: make piece length dynamic by file size
        let piece_length = (32 * KiB) as usize;
        let num_pieces = ((file_size as f64) / (piece_length as f64)).ceil() as usize;
        let mut pieces: Vec<u8> = Vec::with_capacity(num_pieces * 20);

        let mut file_hasher = md5::Context::new();
        for piece in 0..num_pieces {
            let mut offset = piece * piece_length;
            let piece_end = (offset + piece_length).min(file_size as usize);
            let mut piece_hasher = sha1::Sha1::new();
            let mut buf = [0; 4096];
            while offset < piece_end {
                let len = file.read(&mut buf).await?;
                piece_hasher.write_all(&buf[0..len])?;
                file_hasher.write_all(&buf[0..len])?;
                offset += len;
            }
            let sha1 = piece_hasher.finalize();
            pieces.extend_from_slice(&sha1);
        }

        let md5::Digest(digest) = file_hasher.compute();
        let md5sum = hex::encode(digest);

        let file_name = path
            .as_ref()
            .file_name()
            .map(|name| name.as_bytes().to_vec())
            .unwrap_or_default();

        let value = Value::dictionary()
            .with_entry("piece length", Value::Integer(piece_length as i64))
            .with_entry("pieces", Value::String(pieces))
            .with_entry("name", Value::String(file_name))
            .with_entry("length", Value::Integer(file_size as i64))
            .with_entry("md5sum", Value::String(md5sum.into_bytes()));

        Info::try_from(value)
    }
}

impl TryFrom<Value> for Info {
    type Error = Error;

    fn try_from(mut value: Value) -> Result<Self> {
        let info_hash = Sha1::from(&value);
        let piece_length = value.remove_entry("piece length")?.try_into()?;
        let pieces: Vec<_> = value.remove_entry("pieces")?.try_into()?;
        let pieces = Info::build_pieces(&pieces)?;
        let download_type = value.try_into()?;
        Ok(Info {
            info_hash,
            piece_length,
            pieces,
            download_type,
        })
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum DownloadType {
    SingleFile {
        name: String,
        length: Size,
        md5sum: Option<Md5>,
    },
    MultiFile {
        directory_name: String,
        files: Vec<File>,
    },
}

impl DownloadType {
    pub fn length(&self) -> Size {
        match self {
            Self::SingleFile { length, .. } => *length,
            Self::MultiFile { files, .. } => files
                .iter()
                .fold(Size::from_bytes(0), |total, file| total + file.length),
        }
    }
}

impl TryFrom<Value> for DownloadType {
    type Error = Error;

    fn try_from(mut value: Value) -> Result<Self> {
        let name = value.remove_entry("name")?.try_into()?;
        if let Some(length) = value.try_remove_entry("length")? {
            let length = length.try_into()?;
            let md5sum = match value.try_remove_entry("md5sum") {
                Ok(Some(value)) => Some(value.try_into()?),
                _ => None,
            };
            return Ok(DownloadType::SingleFile {
                name,
                length,
                md5sum,
            });
        }

        if let Some(files) = value.try_remove_entry("files")? {
            let files: Vec<Value> = files.try_into()?;
            let mut result = Vec::with_capacity(files.len());
            for file in files {
                result.push(file.try_into()?);
            }
            return Ok(DownloadType::MultiFile {
                directory_name: name,
                files: result,
            });
        }

        Err(anyhow!("invalid metainfo"))
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct File {
    path: PathBuf,
    length: Size,
    md5sum: Option<Md5>,
}

impl TryFrom<Value> for File {
    type Error = Error;

    fn try_from(mut value: Value) -> Result<Self> {
        let length = value.remove_entry("length")?.try_into()?;
        let parts: Vec<Value> = value.remove_entry("path")?.try_into()?;
        let mut path = PathBuf::with_capacity(parts.len());
        for part in parts {
            let part: String = part.try_into()?;
            path.push(part);
        }
        let md5sum = match value.try_remove_entry("md5sum") {
            Ok(Some(value)) => Some(value.try_into()?),
            _ => None,
        };
        Ok(File {
            length,
            path,
            md5sum,
        })
    }
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
                    .with_entry("pieces", Value::String(pieces))
                    .with_entry("name", Value::string("image.iso"))
                    .with_entry("length", Value::Integer(5678))
                    .with_entry("md5sum", Value::string("5d41402abc4b2a76b9719d911017c592")),
            );

        let torrent = Torrent::try_from(metainfo).expect("invalid metainfo");

        assert_eq!(
            torrent.announce.to_string(),
            "udp://tracker.opentrackr.org:1337/announce".to_string()
        );
        assert_eq!(torrent.info.piece_length, Size::from_bytes(1234));
        assert_eq!(torrent.info.pieces, vec![Sha1(piece1), Sha1(piece2)]);
        if let DownloadType::SingleFile {
            name,
            length,
            md5sum,
        } = torrent.info.download_type
        {
            assert_eq!(name, "image.iso");
            assert_eq!(length, Size::from_bytes(5678));
            assert!(md5sum.is_some());
        } else {
            panic!("unexpected download type");
        };
    }

    #[test]
    fn multi_file_torrent_metainfo() {
        let metainfo = Value::dictionary()
            .with_entry(
                "announce",
                Value::string("udp://tracker.opentrackr.org:1337/announce"),
            )
            .with_entry(
                "info",
                Value::dictionary()
                    .with_entry("piece length", Value::Integer(46))
                    .with_entry("pieces", Value::string(""))
                    .with_entry("name", Value::string("root"))
                    .with_entry(
                        "files",
                        Value::list()
                            .with_value(
                                Value::dictionary()
                                    .with_entry("length", Value::Integer(12))
                                    .with_entry(
                                        "path",
                                        Value::list()
                                            .with_value(Value::string("dir"))
                                            .with_value(Value::string("file1")),
                                    )
                                    .with_entry(
                                        "md5sum",
                                        Value::string("b4c7f37a5f303a1a3a4c7206f46504db"),
                                    ),
                            )
                            .with_value(
                                Value::dictionary()
                                    .with_entry("length", Value::Integer(34))
                                    .with_entry(
                                        "path",
                                        Value::list()
                                            .with_value(Value::string("dir"))
                                            .with_value(Value::string("file2")),
                                    )
                                    .with_entry(
                                        "md5sum",
                                        Value::string("f25a2fc72690b780b2a14e140ef6a9e0"),
                                    ),
                            ),
                    ),
            );

        let torrent = Torrent::try_from(metainfo).expect("invalid metainfo");

        if let DownloadType::MultiFile {
            directory_name,
            files,
        } = torrent.info.download_type
        {
            assert_eq!(directory_name, "root");
            assert_eq!(files.len(), 2);

            assert_eq!(files[0].length, Size::from_bytes(12));
            assert_eq!(files[0].path, PathBuf::from("dir/file1"));
            assert!(files[0].md5sum.is_some());

            assert_eq!(files[1].length, Size::from_bytes(34));
            assert_eq!(files[1].path, PathBuf::from("dir/file2"));
            assert!(files[1].md5sum.is_some());
        } else {
            panic!("unexpected download type");
        };
    }

    #[tokio::test]
    async fn load_torrent_info_from_file() {
        let info = Info::load("assets/alice_in_wonderland.txt").await.unwrap();

        assert_eq!(
            info,
            Info {
                info_hash: Sha1::from_hex("e90cf5ec83e174d7dcb94821560dac201ae1f663").unwrap(),
                piece_length: Size::from_kibibytes(32),
                pieces: vec![
                    Sha1::from_hex("8fdfb566405fc084761b1fe0b6b7f8c6a37234ed").unwrap(),
                    Sha1::from_hex("2494039151d7db3e56b3ec021d233742e3de55a6").unwrap(),
                    Sha1::from_hex("af99be061f2c5eee12374055cf1a81909d276db5").unwrap(),
                    Sha1::from_hex("3c12e1fcba504fedc13ee17ea76b62901dc8c9f7").unwrap(),
                    Sha1::from_hex("d5facb89cbdc2e3ed1a1cd1050e217ec534f1fad").unwrap(),
                    Sha1::from_hex("d5d2b296f52ab11791aad35a7d493833d39c6786").unwrap()
                ],
                download_type: DownloadType::SingleFile {
                    name: "alice_in_wonderland.txt".to_string(),
                    length: Size::from_bytes(174357),
                    md5sum: Some(Md5::from_hex("9a930de3cfc64468c05715237a6b4061").unwrap())
                },
            }
        );
    }
}
