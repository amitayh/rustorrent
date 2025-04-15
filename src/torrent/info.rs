use anyhow::{Error, Result, anyhow};

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

    pub fn total_pieces(&self) -> usize {
        self.pieces.len()
    }

    pub fn piece_size(&self, piece: usize) -> usize {
        let piece_start = self.piece_offset(piece);
        let piece_end = (piece_start + self.piece_size).min(self.total_size());
        piece_end - piece_start
    }

    pub fn piece_offset(&self, piece: usize) -> usize {
        self.piece_size * piece
    }

    pub fn total_size(&self) -> usize {
        match &self.download_type {
            DownloadType::SingleFile { size, .. } => *size,
            DownloadType::MultiFile { files, .. } => files.iter().map(|file| file.size).sum(),
        }
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
