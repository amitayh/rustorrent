use std::net::SocketAddr;

use tokio::net::TcpStream;

use crate::message::{Block, Message};

use super::stats::Stats;

#[derive(Debug)]
pub enum Event {
    KeepAliveTick,
    ChokeTick,
    Message(SocketAddr, Message),
    Stats(SocketAddr, Stats),
    BlockTimeout(SocketAddr, Block),
    PieceCompleted(usize),
    PieceInvalid(usize),
    Connect(SocketAddr),
    AcceptConnection(SocketAddr, TcpStream),
    Disconnect(SocketAddr),
}
