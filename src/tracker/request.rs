use url::Url;
use url::form_urlencoded::byte_serialize;

use crate::core::{PeerId, Sha1};

#[allow(dead_code)]
#[derive(Debug)]
pub struct TrackerRequest {
    pub announce: Url,
    pub info_hash: Sha1,
    pub peer_id: PeerId,
    /// The port number that the client is listening on. Ports reserved for BitTorrent are
    /// typically 6881-6889. Clients may choose to give up if it cannot establish a port within
    /// this range.
    pub port: u16,
    /// The total amount uploaded (since the client sent the 'started' event to the tracker).
    pub uploaded: usize,
    /// The total amount downloaded (since the client sent the 'started' event to the tracker).
    pub downloaded: usize,
    /// The number of bytes needed to download to be 100% complete and get all the included files
    /// in the torrent.
    pub left: usize,
    pub mode: Mode,
    pub event: Option<Event>,
    pub tracker_id: Option<String>,
}

impl From<TrackerRequest> for Url {
    fn from(request: TrackerRequest) -> Self {
        let mut query = format!(
            "info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&left={}",
            url_encode(&request.info_hash.0),
            url_encode(&request.peer_id.0),
            request.port,
            request.uploaded,
            request.downloaded,
            request.left,
        );
        if let Some(event) = &request.event {
            query.push_str("&event=");
            query.push_str(event.into());
        }
        if let Some(id) = &request.tracker_id {
            query.push_str("&trackerid=");
            query.push_str(&url_encode(id.as_bytes()));
        }
        if request.mode == Mode::NormalNoPeerId {
            query.push_str("&no_peer_id=1");
        } else if request.mode == Mode::Compact {
            query.push_str("&compact=1");
        }
        // TODO: preseve old query if exists
        let mut url = request.announce;
        url.set_query(Some(&query));
        url
    }
}

// TODO: prevent temp string
fn url_encode(bytes: &[u8]) -> String {
    String::from_iter(byte_serialize(bytes))
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
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

#[derive(Debug, Clone, Copy)]
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
