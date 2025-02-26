use crate::torrent::Torrent;

struct Peer;

struct Download {
    torrent: Torrent,
    peers: Vec<Peer>,
}

struct Client {
    downloads: Vec<Download>,
}

/// All of the remaining messages in the protocol take the form of <length prefix><message
/// ID><payload>. The length prefix is a four byte big-endian value. The message ID is a single
/// decimal byte. The payload is message dependent.
enum Message {
    /// keep-alive: <len=0000>
    ///
    /// The keep-alive message is a message with zero bytes, specified with the length prefix set
    /// to zero. There is no message ID and no payload. Peers may close a connection if they
    /// receive no messages (keep-alive or any other message) for a certain period of time, so a
    /// keep-alive message must be sent to maintain the connection alive if no command have been
    /// sent for a given amount of time. This amount of time is generally two minutes.
    KeepAlive,

    /// choke: <len=0001><id=0>
    ///
    /// The choke message is fixed-length and has no payload.
    Choke,

    /// unchoke: <len=0001><id=1>
    ///
    /// The unchoke message is fixed-length and has no payload.
    Unchoke,

    /// interested: <len=0001><id=2>
    ///
    /// The interested message is fixed-length and has no payload.
    Interested,

    /// not interested: <len=0001><id=3>
    ///
    /// The not interested message is fixed-length and has no payload.
    NotInterested,

    /// have: <len=0005><id=4><piece index>
    ///
    /// The have message is fixed length. The payload is the zero-based index of a piece that has
    /// just been successfully downloaded and verified via the hash.
    Have { piece_index: usize },

    /// bitfield: <len=0001+X><id=5><bitfield>
    ///
    /// The bitfield message may only be sent immediately after the handshaking sequence is
    /// completed, and before any other messages are sent. It is optional, and need not be sent if
    /// a client has no pieces.
    ///
    /// The bitfield message is variable length, where X is the length of the bitfield. The payload
    /// is a bitfield representing the pieces that have been successfully downloaded. The high bit
    /// in the first byte corresponds to piece index 0. Bits that are cleared indicated a missing
    /// piece, and set bits indicate a valid and available piece. Spare bits at the end are set to
    /// zero.
    Bitfield(Vec<bool>), // TODO: use a bitset

    /// request: <len=0013><id=6><index><begin><length>
    ///
    /// The request message is fixed length, and is used to request a block. The payload contains
    /// the following information:
    ///
    ///  * index: integer specifying the zero-based piece index
    ///  * begin: integer specifying the zero-based byte offset within the piece
    ///  * length: integer specifying the requested length.
    Request {
        index: usize,
        begin: usize,
        length: usize,
    },

    /// piece: <len=0009+X><id=7><index><begin><block>
    ///
    /// The piece message is variable length, where X is the length of the block. The payload
    /// contains the following information:
    ///
    ///  * index: integer specifying the zero-based piece index
    ///  * begin: integer specifying the zero-based byte offset within the piece
    ///  * block: block of data, which is a subset of the piece specified by index.
    Piece {
        index: usize,
        begin: usize,
        // block?
    },

    /// cancel: <len=0013><id=8><index><begin><length>
    ///
    /// The cancel message is fixed length, and is used to cancel block requests. The payload is
    /// identical to that of the "request" message. It is typically used during "End Game" (see the
    /// Algorithms section below).
    Cancel {
        index: usize,
        begin: usize,
        length: usize,
    },

    /// port: <len=0003><id=9><listen-port>
    ///
    /// The port message is sent by newer versions of the Mainline that implements a DHT tracker.
    /// The listen port is the port this peer's DHT node is listening on. This peer should be
    /// inserted in the local routing table (if DHT tracker is supported).
    Port(u16),
}

mod message {
    const ID_CHOKE: u8 = 0;
    const ID_UNCHOKE: u8 = 1;
    const ID_INTERESTED: u8 = 2;
    const ID_NOT_INTERESTED: u8 = 3;
    const ID_HAVE: u8 = 4;
    const ID_BITFIELD: u8 = 5;
    const ID_REQUEST: u8 = 6;
    const ID_PIECE: u8 = 7;
    const ID_CANCEL: u8 = 8;
    const ID_PORT: u8 = 9;
}
