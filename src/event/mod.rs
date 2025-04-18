mod handler;
mod sweeper;

use std::net::SocketAddr;

use tokio::{net::TcpStream, time::Instant};

use crate::message::Message;
use crate::peer::stats::PeerStats;

pub use handler::*;

/// Events that can occur in the peer system
#[derive(Debug)]
pub enum Event {
    /// Periodic tick to send keep-alive messages to peers
    KeepAliveTicked,
    /// Periodic tick to run the choking algorithm
    ChokeTicked,
    /// Periodic tick to collect peer statistics
    StatsTicked,
    /// Periodic tick to sweep for idle peers and abandoned blocks, with current timestamp
    SweepTicked(Instant),
    /// Received a BitTorrent protocol message from a peer
    MessageReceived(SocketAddr, Message),
    /// Received updated statistics for a peer
    StatsUpdated(SocketAddr, PeerStats),
    /// A piece was successfully downloaded and verified
    PieceCompleted(usize),
    /// A downloaded piece failed hash verification
    PieceVerificationFailed(usize),
    /// Initiate connection to a peer
    ConnectionRequested(SocketAddr),
    /// Accept an incoming connection from a peer
    ConnectionAccepted(SocketAddr, TcpStream),
    /// A peer connection was terminated
    Disconnected(SocketAddr),
    /// Shut down the entire system
    ShutdownRequested,
}
