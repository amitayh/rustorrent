use std::net::{SocketAddr, TcpStream};

use crate::message::Message;

#[derive(Debug, PartialEq)]
pub enum Event {
    KeepAliveTick,
    ChokeTick,
    Message(SocketAddr, Message),
    Connect(SocketAddr),
    AcceptConnection(SocketAddr),
    Disconnect(SocketAddr),
}
