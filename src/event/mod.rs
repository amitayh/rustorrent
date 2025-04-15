mod action;
mod handler;

use std::net::SocketAddr;

use tokio::{net::TcpStream, time::Instant};

use crate::message::Message;
use crate::peer::stats::PeerStats;

pub use action::*;
pub use handler::*;

/// Events that can occur in the peer system
#[derive(Debug)]
pub enum Event {
    /// Periodic tick to send keep-alive messages to peers
    KeepAliveTick,
    /// Periodic tick to run the choking algorithm
    ChokeTick,
    /// Periodic tick to collect peer statistics
    StatsTick,
    /// Periodic tick to sweep for idle peers and abandoned blocks, with current timestamp
    SweepTick(Instant),
    /// Received a BitTorrent protocol message from a peer
    Message(SocketAddr, Message),
    /// Received updated statistics for a peer
    Stats(SocketAddr, PeerStats),
    /// A piece was successfully downloaded and verified
    PieceCompleted(usize),
    /// A downloaded piece failed hash verification
    PieceInvalid(usize),
    /// Initiate connection to a peer
    Connect(SocketAddr),
    /// Accept an incoming connection from a peer
    AcceptConnection(SocketAddr, TcpStream),
    /// A peer connection was terminated
    Disconnect(SocketAddr),
    /// Shut down the entire system
    Shutdown,
}
