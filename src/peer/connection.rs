use std::fmt::Debug;
use std::io::ErrorKind;
use std::io::{Result, SeekFrom};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::anyhow;
use log::{info, warn};
use size::Size;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::Instant;

use crate::codec::{AsyncDecoder, AsyncEncoder, TransportMessage};
use crate::message::{Block, BlockData};
use crate::message::{Handshake, Message};
use crate::peer::Event;
use crate::peer::stats::Stats;
use crate::peer::transfer_rate::TransferRate;

pub struct Connection {
    socket: TcpStream,
    file_path: PathBuf,
    piece_size: Size,
    tx: Sender<Event>,
    rx: Receiver<Command>,
    stats: Stats,
}

impl Connection {
    pub fn new(
        socket: TcpStream,
        file_path: PathBuf,
        piece_size: Size,
        tx: Sender<Event>,
        rx: Receiver<Command>,
    ) -> Self {
        Self {
            socket,
            file_path,
            piece_size,
            tx,
            rx,
            stats: Stats::default(),
        }
    }

    pub async fn wait_for_handshake(&mut self) -> anyhow::Result<()> {
        let handshake = Handshake::decode(&mut self.socket).await?;
        info!("< got handshake {:?}", handshake);
        if !handshake.is_standard_protocol() {
            return Err(anyhow!(
                "invalid handshake protocol: {}",
                handshake.protocol
            ));
        }
        Ok(())
    }

    pub async fn send<T: AsyncEncoder + TransportMessage + Debug>(
        &mut self,
        message: &T,
    ) -> Result<()> {
        let direction = format!(
            "{} -> {}",
            self.socket.local_addr()?,
            self.socket.peer_addr()?,
        );
        info!(target: &direction, "sending message: {:?}", message);
        let transfer_begin = Instant::now();
        message.encode(&mut self.socket).await?;
        let duration = Instant::now() - transfer_begin;
        let size = Size::from_bytes(message.transport_bytes());
        self.stats.upload += TransferRate(size, duration);
        Ok(())
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        let mut update_stats = tokio::time::interval(Duration::from_secs(1));
        loop {
            let addr = self.socket.peer_addr()?;
            tokio::select! {
                _ = update_stats.tick() => {
                    self.tx.send(Event::Stats(addr, self.stats.clone())).await?;
                },
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
                        let message = Message::Piece(BlockData {
                            piece: block.piece,
                            offset: block.offset,
                            data,
                        });
                        self.send(&message).await?;
                        //let duration = Instant::now() - transfer_begin;
                        //let size = Size::from_bytes(message.transport_bytes());
                        //let transfer_rate = TransferRate(size, duration);
                        //self.tx.send(PeerEvent(
                        //    self.addr, Event::Uploaded(block, transfer_rate))).await?;
                    }
                    Command::Shutdown => {
                        break;
                    }
                },
                // TODO: measure download speed
                message = decode_message(&mut self.socket) =>  match message {
                    Ok((message, transfer_rate)) => {
                        self.stats.download += transfer_rate;
                        if !matches!(message, Message::KeepAlive) {
                            info!(target: &self.log_target()?, "got message: {:?}", &message);
                        }
                        self.tx.send(Event::Message(addr, message)).await?;
                    }
                    Err(err) => {
                        warn!(target: &self.log_target()?, "failed to decode message: {}", err);
                        if err.kind() == ErrorKind::UnexpectedEof {
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn log_target(&self) -> std::io::Result<String> {
        let local_addr = self.socket.local_addr()?;
        let peer_addr = self.socket.peer_addr()?;
        Ok(format!("{} <- {}", local_addr, peer_addr))
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

#[derive(Clone, Debug)]
pub enum Command {
    Send(Message),
    Upload(Block),
    Shutdown,
}
