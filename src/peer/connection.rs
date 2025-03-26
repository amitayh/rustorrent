use std::fmt::Debug;
use std::io::ErrorKind;
use std::io::Result;
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

use crate::codec::{AsyncDecoder, AsyncEncoder, TransportMessage};
use crate::message::MessageCodec;
use crate::message::{Handshake, Message};
use crate::peer::Event;
use crate::peer::stats::Stats;
use crate::peer::transfer_rate::TransferRate;

pub struct Connection {
    messages: Framed<TcpStream, MessageCodec>,
    tx: Sender<Event>,
    rx: Receiver<Command>,
    stats: Stats,
}

impl Connection {
    pub fn new(socket: TcpStream, tx: Sender<Event>, rx: Receiver<Command>) -> Self {
        Self {
            messages: Framed::new(socket, MessageCodec),
            tx,
            rx,
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
        let mut update_stats = tokio::time::interval(Duration::from_secs(1));
        loop {
            let addr = self.messages.get_ref().peer_addr()?;
            tokio::select! {
                _ = update_stats.tick() => {
                    self.tx.send(Event::Stats(addr, self.stats.clone())).await?;
                },
                Some(command) = self.rx.recv() => match command {
                    Command::Send(message) => {
                        info!(target: &self.log_target()?, "sending message: {:?}", &message);
                        self.messages.send(message).await?;
                    }
                    Command::Shutdown => {
                        break;
                    }
                },
                Some(message) = self.messages.next() => match message {
                    Ok(message) => {
                        info!(target: &self.log_target()?, "got {:?}", message);
                        self.tx.send(Event::Message(addr, message)).await.expect("unable to send");
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
        let local_addr = self.messages.get_ref().local_addr()?;
        let peer_addr = self.messages.get_ref().peer_addr()?;
        Ok(format!("{} <- {}", local_addr, peer_addr))
    }
}

//async fn decode_message(socket: &mut TcpStream) -> std::io::Result<(Message, TransferRate)> {
//    let transfer_begin = Instant::now();
//    let message = Message::decode(socket).await?;
//    let duration = Instant::now() - transfer_begin;
//    let size = Size::from_bytes(message.transport_bytes());
//    let transfer_rate = TransferRate(size, duration);
//    Ok((message, transfer_rate))
//}

#[derive(Clone, Debug)]
pub enum Command {
    Send(Message),
    Shutdown,
}
