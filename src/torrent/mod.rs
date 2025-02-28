use std::path::PathBuf;

use anyhow::{Error, Result, anyhow};
use size::Size;
use url::Url;

use crate::{
    bencoding::value::Value,
    crypto::{Md5, Sha1},
};

// https://wiki.theory.org/BitTorrentSpecification#Byte_Strings

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

#[derive(Debug, PartialEq)]
pub struct Info {
    pub info_hash: Sha1,
    pub piece_length: Size,
    pub pieces: Vec<Sha1>,
    pub download_type: DownloadType,
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
        let download_type = value.try_into()?;
        Ok(Info {
            info_hash,
            piece_length,
            pieces,
            download_type,
        })
    }
}

#[derive(Debug, PartialEq)]
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

#[derive(Debug, PartialEq)]
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
}
