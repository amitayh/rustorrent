use std::path::PathBuf;

use anyhow::{Error, Result, anyhow};

use crate::{bencoding::Value, core::Md5};

#[derive(Debug, PartialEq, Clone)]
pub enum DownloadType {
    SingleFile {
        name: String,
        size: usize,
        md5sum: Option<Md5>,
    },
    MultiFile {
        directory_name: String,
        files: Vec<File>,
    },
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
                size: length,
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
    pub size: usize,
    pub md5sum: Option<Md5>,
}

impl TryFrom<Value> for File {
    type Error = Error;

    fn try_from(mut value: Value) -> Result<Self> {
        let size = value.remove_entry("length")?.try_into()?;
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
        Ok(File { size, path, md5sum })
    }
}
