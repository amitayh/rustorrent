use std::net::SocketAddr;

use tokio::{net::TcpStream, time::Instant};

use crate::message::Message;
use crate::peer::stats::PeerStats;

#[derive(Debug)]
pub enum Event {
    KeepAliveTick,
    ChokeTick,
    StatsTick,
    SweepTick(Instant),
    Message(SocketAddr, Message),
    Stats(SocketAddr, PeerStats),
    PieceCompleted(usize),
    PieceInvalid(usize),
    Connect(SocketAddr),
    AcceptConnection(SocketAddr, TcpStream),
    Disconnect(SocketAddr),
    Shutdown,
}
