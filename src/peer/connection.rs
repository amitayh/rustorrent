use std::fmt::Debug;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::anyhow;
use futures::SinkExt;
use log::debug;
use log::{info, warn};
use size::Size;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_stream::StreamExt;
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;

use crate::codec::{AsyncDecoder, AsyncEncoder, TransportMessage};
use crate::crypto::Sha1;
use crate::message::MessageCodec;
use crate::message::{Handshake, Message};
use crate::peer::Config;
use crate::peer::Event;
use crate::peer::stats::Stats;
use crate::peer::transfer_rate::TransferRate;

pub struct Connection {
    pub tx: Sender<Message>,
    join_handle: JoinHandle<anyhow::Result<()>>,
    cancellation_token: CancellationToken,
}

impl Connection {
    pub fn spawn(
        addr: SocketAddr,
        socket: Option<TcpStream>,
        events_tx: Sender<Event>,
        info_hash: Sha1,
        config: &Config,
    ) -> Self {
        let (tx, rx) = mpsc::channel(config.channel_buffer);
        let cancellation_token = CancellationToken::new();
        let token_clone = cancellation_token.clone();
        let handshake = Handshake::new(info_hash, config.clinet_id.clone());
        let block_size = config.block_size;
        let join_handle = tokio::spawn(async move {
            let result = run(
                addr,
                socket,
                handshake,
                block_size,
                events_tx.clone(),
                rx,
                token_clone,
            )
            .await;
            if let Err(err) = result {
                warn!("[{}] error encountered run: {}", addr, err);
            }
            info!("peer {} disconnected", addr);
            events_tx
                .send(Event::Disconnect(addr))
                .await
                .expect("channel should be open");
            Ok(())
        });
        Self {
            tx,
            join_handle,
            cancellation_token,
        }
    }

    pub async fn send(&self, message: Message) {
        if self.tx.send(message).await.is_err() {
            warn!("channel already closed");
        }
    }

    pub fn abort(self) {
        self.join_handle.abort();
    }

    pub async fn shutdown(self) -> anyhow::Result<()> {
        self.cancellation_token.cancel();
        self.join_handle.await??;
        Ok(())
    }
}

async fn run(
    addr: SocketAddr,
    socket: Option<TcpStream>,
    handshake: Handshake,
    block_size: Size,
    events_tx: Sender<Event>,
    mut rx: Receiver<Message>,
    cancellation_token: CancellationToken,
) -> anyhow::Result<()> {
    let (mut socket, handshake_direction) = connect_if_needed(addr, socket).await?;
    exchange_handshakes(&mut socket, handshake, handshake_direction).await?;

    let max_size = (block_size.bytes() as usize) + 9;
    let mut messages = Framed::new(socket, MessageCodec::new(max_size));
    let mut update_stats = tokio::time::interval(Duration::from_secs(1));
    let mut stats = Stats::default();
    let mut running = true;

    while running {
        let start = Instant::now();
        tokio::select! {
            _ = update_stats.tick() => {
                events_tx.send(Event::Stats(addr, stats.clone())).await?;
            },
            Some(message) = rx.recv() => {
                debug!("[{}] > sending {:?}", addr, &message);
                let message_size = Size::from_bytes(message.transport_bytes());
                messages.send(message).await?;
                let elapsed = Instant::now() - start;
                stats.upload += TransferRate(message_size, elapsed);
            },
            Some(message) = messages.next() => match message {
                Ok(message) => {
                    debug!("[{}] < got {:?}", addr, message);
                    let elapsed = Instant::now() - start;
                    let message_size = Size::from_bytes(message.transport_bytes());
                    stats.download += TransferRate(message_size, elapsed);
                    let event = Event::Message(addr, message);
                    events_tx.send(event).await?;
                }
                Err(err) => {
                    warn!("[{}] failed to decode message: {}", addr, err);
                    if err.kind() == ErrorKind::UnexpectedEof {
                        info!("[{}] socket closed, shutting down...", addr);
                        running = false;
                    }
                }

            },
            _ = cancellation_token.cancelled() => {
                info!("[{}] shutting down...", addr);
                running = false;
            }
        }
    }

    messages.flush().await?;
    Ok(())
}

async fn connect_if_needed(
    addr: SocketAddr,
    socket: Option<TcpStream>,
) -> anyhow::Result<(TcpStream, HandshakeDirection)> {
    match socket {
        Some(socket) => {
            info!("accepted connection from {}", addr);
            Ok((socket, HandshakeDirection::PeerToClient))
        }
        None => {
            info!("connecting to {}...", addr);
            let socket = TcpStream::connect(addr).await?;
            Ok((socket, HandshakeDirection::ClientToPeer))
        }
    }
}

async fn exchange_handshakes(
    socket: &mut TcpStream,
    handshake: Handshake,
    direction: HandshakeDirection,
) -> anyhow::Result<()> {
    if direction == HandshakeDirection::ClientToPeer {
        // Send handshake first
        handshake.encode(socket).await?;
    }
    // Wait for handshhake from peer
    let handshake_got = Handshake::decode(socket).await?;
    if direction == HandshakeDirection::PeerToClient {
        // Send handshake second
        handshake.encode(socket).await?;
    }
    if handshake.info_hash != handshake_got.info_hash {
        return Err(anyhow!("info hash mismatch"));
    }
    if !handshake_got.is_standard_protocol() {
        return Err(anyhow!(
            "invalid handshake protocol: {}",
            handshake_got.protocol
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HandshakeDirection {
    ClientToPeer,
    PeerToClient,
}
