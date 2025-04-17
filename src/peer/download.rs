use crate::peer::Config;
use crate::torrent::Torrent;

/// Represents an active download of a torrent
///
/// Contains the torrent metadata and configuration settings needed to manage
/// the download process. This includes information about pieces, files, and
/// parameters like block sizes and timeouts.
#[derive(Debug)]
pub struct Download {
    /// The torrent being downloaded, containing metadata about pieces and files
    pub torrent: Torrent,
    /// Configuration settings for the download like block sizes and timeouts
    pub config: Config,
}
