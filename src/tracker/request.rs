use size::Size;

use crate::{crypto::Sha1, peer::peer_id::PeerId};

#[allow(dead_code)]
struct TrackerRequest {
    info_hash: Sha1,
    peer_id: PeerId,
    /// The port number that the client is listening on. Ports reserved for BitTorrent are
    /// typically 6881-6889. Clients may choose to give up if it cannot establish a port within
    /// this range.
    port: u16,
    /// The total amount uploaded (since the client sent the 'started' event to the tracker).
    uploaded: Size,
    /// The total amount downloaded (since the client sent the 'started' event to the tracker).
    downloaded: Size,
    /// The number of bytes needed to download to be 100% complete and get all the included files
    /// in the torrent.
    left: Size,
    mode: ResponseMode,
    event: Option<Event>,
}

#[allow(dead_code)]
enum ResponseMode {
    Normal,
    /// Indicates that the tracker can omit peer id field in peers dictionary.
    NormalNoPeerId,
    /// The peers list is replaced by a peers string with 6 bytes per peer. The first four bytes
    /// are the host (in network byte order), the last two bytes are the port (again in network
    /// byte order). It should be noted that some trackers only support compact responses (for
    /// saving bandwidth) and either refuse requests without "compact=1" or simply send a compact
    /// response unless the request contains "compact=0" (in which case they will refuse the
    /// request).
    Compact,
}

pub enum Event {
    /// The first request to the tracker must include the event key with this value.
    Started,
    /// Must be sent to the tracker if the client is shutting down gracefully.
    Stopped,
    /// Must be sent to the tracker when the download completes. However, must not be sent if the
    /// download was already 100% complete when the client started. Presumably, this is to allow
    /// the tracker to increment the "completed downloads" metric based solely on this event.
    Completed,
}

impl From<&Event> for &str {
    fn from(value: &Event) -> Self {
        match value {
            Event::Started => "started",
            Event::Stopped => "stopped",
            Event::Completed => "completed",
        }
    }
}
