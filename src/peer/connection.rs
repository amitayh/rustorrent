use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use futures::SinkExt;
use log::{debug, error, info, warn};
use size::Size;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};
use tokio_stream::StreamExt;
use tokio_util::codec::Framed;
use tokio_util::sync::CancellationToken;

use crate::codec::{AsyncDecoder, AsyncEncoder, TransportMessage};
use crate::message::MessageCodec;
use crate::message::{Handshake, Message};
use crate::peer::Download;
use crate::peer::Event;
use crate::peer::stats::PeerStats;
use crate::peer::transfer_rate::TransferRate;

use super::Config;

pub struct Connection {
    pub tx: Sender<Message>,
    join_handle: JoinHandle<anyhow::Result<()>>,
    cancellation_token: CancellationToken,
    addr: SocketAddr,
}

impl Connection {
    pub fn spawn(
        addr: SocketAddr,
        socket: Option<TcpStream>,
        events_tx: Sender<Event>,
        download: Arc<Download>,
    ) -> Self {
        let (tx, rx) = mpsc::channel(download.config.channel_buffer);
        let cancellation_token = CancellationToken::new();
        let token_clone = cancellation_token.clone();
        let handshake = Handshake::new(
            download.torrent.info.info_hash.clone(),
            download.config.client_id.clone(),
        );
        let join_handle = tokio::spawn(async move {
            let result = tokio::select! {
                _ = token_clone.cancelled() => Ok(()),
                result = run(
                    addr,
                    socket,
                    handshake,
                    events_tx.clone(),
                    rx,
                    &download.config,
                ) => result,
            };
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
            addr,
        }
    }

    pub fn send(&self, message: Message) {
        match self.tx.try_send(message) {
            Ok(_) => (),
            Err(TrySendError::Full(_)) => {
                warn!("[{}] peer unresponsive, shutting down", &self.addr);
                self.cancellation_token.cancel();
            }
            Err(TrySendError::Closed(_)) => {
                error!("[{}] sending message to disconnected peer", &self.addr);
            }
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
    events_tx: Sender<Event>,
    mut rx: Receiver<Message>,
    config: &Config,
) -> anyhow::Result<()> {
    let (mut socket, handshake_direction) =
        connect_if_needed(addr, socket, config.connect_timeout).await?;

    exchange_handshakes(&mut socket, handshake, handshake_direction).await?;

    // The largest message we should accept is a `Piece` message, which should
    // always have size of `config.block_size` + 9
    let max_size = (config.block_size.bytes() as usize) + 9;
    let mut messages = Framed::new(socket, MessageCodec::new(max_size));
    let mut update_stats = tokio::time::interval(Duration::from_secs(1));
    let mut stats = PeerStats::default();
    let mut running = true;

    while running {
        let start = Instant::now();
        tokio::select! {
            _ = update_stats.tick() => {
                events_tx.send(Event::Stats(addr, stats.clone())).await?;
            },
            Some(message) = rx.recv() => {
                debug!("[{}] > sending {:?}", &addr, &message);
                let message_size = Size::from_bytes(message.transport_bytes());
                messages.send(message).await?;
                let elapsed = Instant::now() - start;
                stats.upload += TransferRate(message_size, elapsed);
            },
            Some(message) = messages.next() => match message {
                Ok(message) => {
                    debug!("[{}] < got {:?}", &addr, message);
                    let elapsed = Instant::now() - start;
                    let message_size = Size::from_bytes(message.transport_bytes());
                    stats.download += TransferRate(message_size, elapsed);
                    let event = Event::Message(addr, message);
                    events_tx.send(event).await?;
                }
                Err(err) => {
                    warn!("[{}] failed to decode message: {}", &addr, err);
                    if err.kind() == ErrorKind::UnexpectedEof {
                        info!("[{}] socket closed, shutting down...", &addr);
                        running = false;
                    }
                }
            },
        }
    }

    messages.flush().await?;
    Ok(())
}

async fn connect_if_needed(
    addr: SocketAddr,
    socket: Option<TcpStream>,
    connect_timeout: Duration,
) -> anyhow::Result<(TcpStream, HandshakeDirection)> {
    match socket {
        Some(socket) => {
            info!("accepted connection from {}", &addr);
            Ok((socket, HandshakeDirection::PeerToClient))
        }
        None => {
            info!("connecting to {}...", &addr);
            let socket = timeout(connect_timeout, TcpStream::connect(addr)).await??;
            info!("connected to {}", &addr);
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
