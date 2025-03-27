use std::fmt::Debug;
use std::io::ErrorKind;
use std::io::Result;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::anyhow;
use futures::SinkExt;
use log::{info, warn};
use size::Size;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::Instant;
use tokio_stream::StreamExt;
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;

use crate::codec::{AsyncDecoder, AsyncEncoder, TransportMessage};
use crate::message::MessageCodec;
use crate::message::{Handshake, Message};
use crate::peer::Event;
use crate::peer::stats::Stats;
use crate::peer::transfer_rate::TransferRate;

pub struct Connection {
    addr: SocketAddr,
    messages: Framed<TcpStream, MessageCodec>,
    tx: Sender<Event>,
    rx: Receiver<Command>,
    cancellation_token: CancellationToken,
    stats: Stats,
}

impl Connection {
    pub fn new(
        socket: TcpStream,
        tx: Sender<Event>,
        rx: Receiver<Command>,
        cancellation_token: CancellationToken,
    ) -> Self {
        let addr = socket.peer_addr().expect("missing addr");
        let messages = Framed::new(socket, MessageCodec);
        Self {
            addr,
            messages,
            tx,
            rx,
            cancellation_token,
            stats: Stats::default(),
        }
    }

    pub async fn wait_for_handshake(&mut self) -> anyhow::Result<()> {
        let handshake = Handshake::decode(self.messages.get_mut()).await?;
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
            self.messages.get_ref().local_addr()?,
            self.messages.get_ref().peer_addr()?,
        );
        info!(target: &direction, "sending message: {:?}", message);
        let transfer_begin = Instant::now();
        message.encode(self.messages.get_mut()).await?;
        self.messages.get_mut().flush().await?;
        let duration = Instant::now() - transfer_begin;
        let size = Size::from_bytes(message.transport_bytes());
        self.stats.upload += TransferRate(size, duration);
        Ok(())
    }

    pub async fn start(&mut self) -> anyhow::Result<()> {
        let start = Instant::now();
        let mut update_stats = tokio::time::interval(Duration::from_secs(1));
        let mut running = true;
        while running {
            tokio::select! {
                _ = update_stats.tick() => {
                    self.tx.send(Event::Stats(self.addr, self.stats.clone())).await?;
                },
                Some(command) = self.rx.recv() => match command {
                    Command::Send(message) => {
                        info!("> sending {:?}", &message);
                        let message_size = Size::from_bytes(message.transport_bytes());
                        self.messages.send(message).await?;
                        let elapsed = Instant::now() - start;
                        let TransferRate(size, duration) = &mut self.stats.upload;
                        *size += message_size;
                        *duration = elapsed;
                    }
                    Command::Shutdown => {
                        info!("shutting down...");
                        running = false;
                    }
                },
                Some(message) = self.messages.next() => match message {
                    Ok(message) => {
                        info!("< got {:?}", message);
                        let elapsed = Instant::now() - start;
                        let message_size = Size::from_bytes(message.transport_bytes());
                        let TransferRate(size, duration) = &mut self.stats.download;
                        *size += message_size;
                        *duration = elapsed;
                        let event = Event::Message(self.addr, message);
                        self.tx.send(event).await.expect("unable to send");
                    }
                    Err(err) => {
                        warn!("failed to decode message: {}", err);
                        if err.kind() == ErrorKind::UnexpectedEof {
                            info!("socket closed, shutting down...");
                            running = false;
                        }
                    }

                },
                _ = self.cancellation_token.cancelled() => {
                    info!("shutting down...");
                    running = false;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum Command {
    Send(Message),
    Shutdown,
}
