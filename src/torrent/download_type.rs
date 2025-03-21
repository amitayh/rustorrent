use std::path::PathBuf;

use anyhow::{Error, Result, anyhow};
use size::Size;

use crate::{bencoding::Value, crypto::Md5};

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
    pub path: PathBuf,
    pub length: Size,
    pub md5sum: Option<Md5>,
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
