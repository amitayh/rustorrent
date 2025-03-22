use std::net::SocketAddr;

use tokio::net::TcpStream;

use crate::message::Message;

#[derive(Debug)]
pub enum Event {
    KeepAliveTick,
    ChokeTick,
    Message(SocketAddr, Message),
    Connect(SocketAddr),
    AcceptConnection(SocketAddr, TcpStream),
    Disconnect(SocketAddr),
}
