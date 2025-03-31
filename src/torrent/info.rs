use std::{io::Write, os::unix::ffi::OsStrExt, path::Path};

use anyhow::{Error, Result, anyhow};
use sha1::Digest;
use size::KiB;
use tokio::io::AsyncReadExt;

use crate::torrent::DownloadType;
use crate::{bencoding::Value, crypto::Sha1};

const SHA1_LEN: usize = 20;

#[derive(Debug, PartialEq, Clone)]
pub struct Info {
    pub info_hash: Sha1,
    pub piece_size: usize,
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

    pub fn total_size(&self) -> usize {
        match &self.download_type {
            DownloadType::SingleFile { size, .. } => *size,
            DownloadType::MultiFile { files, .. } => files.iter().map(|file| file.size).sum(),
        }
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
            piece_size: piece_length,
            pieces,
            download_type,
        })
    }
}
