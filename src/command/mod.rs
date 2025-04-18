mod executor;

use std::fmt::Formatter;
use std::net::SocketAddr;

use tokio::net::TcpStream;

use crate::message::{Block, BlockData, Message};
use crate::peer::stats::GlobalStats;

pub use executor::*;

pub enum Command {
    EstablishConnection(SocketAddr, Option<TcpStream>),
    Send(SocketAddr, Message),
    Broadcast(Message),
    Upload(SocketAddr, Block),
    IntegrateBlock(BlockData),
    RemovePeer(SocketAddr),
    UpdateStats(GlobalStats),
    Shutdown,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EstablishConnection(addr, socket) => {
                write!(f, "EstablishConnection({:?}, {:?}", addr, socket)
            }
            Self::Send(addr, message) => write!(f, "Send({:?}, {:?})", addr, message),
            Self::Broadcast(message) => write!(f, "Broadcast({:?})", message),
            Self::Upload(addr, block) => write!(f, "Upload({:?}, {:?})", addr, block),
            Self::IntegrateBlock(block) => write!(
                f,
                "IntegrateBlock {{ piece: {}, offset: {}, data: <{} bytes> }}",
                block.piece,
                block.offset,
                block.data.len()
            ),
            Self::RemovePeer(addr) => write!(f, "RemovePeer({:?})", addr),
            Self::UpdateStats(stats) => write!(f, "UpdateStats({:?})", stats),
            Self::Shutdown => write!(f, "Shutdown"),
        }
    }
}

impl PartialEq for Command {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::EstablishConnection(addr1, _), Self::EstablishConnection(addr2, _)) => {
                addr1 == addr2
            }
            (Self::Send(addr1, message1), Self::Send(addr2, message2)) => {
                addr1 == addr2 && message1 == message2
            }
            (Self::Broadcast(message1), Self::Broadcast(message2)) => message1 == message2,
            (Self::Upload(addr1, block1), Self::Upload(addr2, block2)) => {
                addr1 == addr2 && block1 == block2
            }
            (Self::IntegrateBlock(block1), Self::IntegrateBlock(block2)) => block1 == block2,
            (Self::RemovePeer(addr1), Self::RemovePeer(addr2)) => addr1 == addr2,
            (Self::UpdateStats(stats1), Self::UpdateStats(stats2)) => stats1 == stats2,
            (Self::Shutdown, Self::Shutdown) => true,
            _ => false,
        }
    }
}

impl Eq for Command {}
