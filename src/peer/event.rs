use std::net::SocketAddr;

use tokio::{net::TcpStream, time::Instant};

use crate::message::Message;
use crate::peer::stats::Stats;

#[derive(Debug)]
pub enum Event {
    KeepAliveTick,
    ChokeTick,
    SweepTick(Instant),
    Message(SocketAddr, Message),
    Stats(SocketAddr, Stats),
    PieceCompleted(usize),
    PieceInvalid(usize),
    Connect(SocketAddr),
    AcceptConnection(SocketAddr, TcpStream),
    Disconnect(SocketAddr),
}
