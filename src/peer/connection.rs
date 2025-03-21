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

use crate::codec::{AsyncDecoder, AsyncEncoder, TransportMessage};
use crate::message::Block;
use crate::message::{Handshake, Message};
use crate::peer::transfer_rate::TransferRate;
use crate::peer::{Event, PeerEvent};

pub struct Connection {
    addr: SocketAddr,
    socket: TcpStream,
    file_path: PathBuf,
    piece_size: Size,
    tx: Sender<PeerEvent>,
    rx: Receiver<Command>,
}

impl Connection {
    pub fn new(
        addr: SocketAddr,
        socket: TcpStream,
        file_path: PathBuf,
        piece_size: Size,
        tx: Sender<PeerEvent>,
        rx: Receiver<Command>,
    ) -> Self {
        Self {
            addr,
            socket,
            file_path,
            piece_size,
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
        info!("> sending message: {:?}", message);
        message.encode(&mut self.socket).await?;
        Ok(())
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                Some(command) = self.rx.recv() => match command {
                    Command::Send(message) => {
                        // TODO: measure upload speed
                        self.send(&message).await?;
                    }
                    Command::Upload(block) => {
                        let mut data = vec![0; block.length];
                        let mut file = File::open(&self.file_path).await?;
                        let offset = block.global_offset(self.piece_size.bytes() as usize);
                        file.seek(SeekFrom::Start(offset as u64)).await?;
                        file.read_exact(&mut data).await?;
                        //let transfer_begin = Instant::now();
                        let message = Message::Piece {
                            piece: block.piece,
                            offset: block.offset,
                            data,
                        };
                        self.send(&message).await?;
                        //let duration = Instant::now() - transfer_begin;
                        //let size = Size::from_bytes(message.transport_bytes());
                        //let transfer_rate = TransferRate(size, duration);
                        //self.tx.send(PeerEvent(
                        //    self.addr, Event::Uploaded(block, transfer_rate))).await?;
                    }
                },
                // TODO: measure download speed
                message = decode_message(&mut self.socket) => {
                    match message {
                        Ok((message, transfer_rate)) => {
                            info!("< got message: {:?} ({})", &message, &transfer_rate);
                            self.tx.send(PeerEvent(
                                self.addr, Event::Message(message, transfer_rate))).await?;
                        }
                        Err(err) => {
                            warn!("failed to decode message: {}", err);
                            if err.kind() == ErrorKind::UnexpectedEof {
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

async fn decode_message(socket: &mut TcpStream) -> std::io::Result<(Message, TransferRate)> {
    let transfer_begin = Instant::now();
    let message = Message::decode(socket).await?;
    let duration = Instant::now() - transfer_begin;
    let size = Size::from_bytes(message.transport_bytes());
    let transfer_rate = TransferRate(size, duration);
    Ok((message, transfer_rate))
}

pub enum Command {
    Send(Message),
    Upload(Block),
}
