use std::fmt::Debug;
use std::io::ErrorKind;
use std::io::{Result, SeekFrom};
use std::net::SocketAddr;
use std::path::PathBuf;

use log::{info, warn};
use size::Size;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::Instant;

use crate::codec::{AsyncDecoder, AsyncEncoder};
use crate::peer::message::Block;
use crate::peer::message::{Handshake, Message};
use crate::peer::transfer_rate::TransferRate;
use crate::peer::{Event, PeerEvent};

pub struct Connection {
    addr: SocketAddr,
    socket: TcpStream,
    file_path: PathBuf,
    tx: Sender<PeerEvent>,
    rx: Receiver<Command>,
}

impl Connection {
    pub fn new(
        addr: SocketAddr,
        socket: TcpStream,
        file_path: PathBuf,
        tx: Sender<PeerEvent>,
        rx: Receiver<Command>,
    ) -> Self {
        Self {
            addr,
            socket,
            file_path,
            tx,
            rx,
        }
    }

    pub async fn wait_for_handshake(&mut self) -> Result<()> {
        let handshake = Handshake::decode(&mut self.socket).await?;
        info!("< got handshake {:?}", handshake);
        Ok(())
    }

    pub async fn send<T: AsyncEncoder + Debug>(&mut self, message: &T) -> Result<()> {
        message.encode(&mut self.socket).await?;
        info!("> sent message: {:?}", message);
        Ok(())
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                Some(command) = self.rx.recv() => match command {
                    Command::Send(message) => {
                        self.send(&message).await?;
                    }
                    Command::Upload(block) => {
                        let mut file = File::open(&self.file_path).await?;
                        file.seek(SeekFrom::Start(block.offset as u64)).await?;
                        let mut reader = file.take(block.length as u64);
                        let transfer_begin = Instant::now();
                        self.send(&Message::Piece(block)).await?;
                        let bytes = tokio::io::copy(&mut reader, &mut self.socket).await? as usize;
                        let duration = Instant::now() - transfer_begin;
                        if bytes != block.length {
                            warn!("peer {} requested block {:?}, actually transferred {} bytes",
                                self.addr, block, bytes);
                        }
                        let transfer_rate = TransferRate(Size::from_bytes(bytes), duration);
                        self.tx.send(PeerEvent(
                            self.addr, Event::Uploaded(block, transfer_rate))).await?;
                    }
                },
                message = Message::decode(&mut self.socket) => {
                    match message {
                        Ok(message) => {
                            info!("< got message: {:?}", &message);
                            if let Message::Piece(block) = message {
                                let mut data = vec![0; block.length];
                                let transfer_begin = Instant::now();
                                self.socket.read_exact(&mut data).await?;
                                let duration = Instant::now() - transfer_begin;
                                let size = Size::from_bytes(block.length);
                                let transfer_rate = TransferRate(size, duration);
                                self.tx.send(PeerEvent(
                                    self.addr, Event::Downloaded(
                                        block, data, transfer_rate))).await?;
                            } else {
                                self.tx.send(PeerEvent(self.addr, Event::Message(message))).await?;
                            }
                        }
                        Err(err) => {
                            warn!("failed to decode message: {}", err);
                            if err.kind() == ErrorKind::UnexpectedEof {
                                // Disconnected
                                break;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

pub enum Command {
    Send(Message),
    Upload(Block),
}
