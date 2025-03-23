use std::net::SocketAddr;

use tokio::net::TcpStream;

use crate::message::{Block, Message};

#[derive(Debug)]
pub enum Event {
    KeepAliveTick,
    ChokeTick,
    Message(SocketAddr, Message),
    BlockTimeout(SocketAddr, Block),
    Connect(SocketAddr),
    AcceptConnection(SocketAddr, TcpStream),
    Disconnect(SocketAddr),
}
