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

use crate::client::Download;
use crate::core::{AsyncDecoder, AsyncEncoder, TransferRate, TransportMessage};
use crate::event::Event;
use crate::message::{Handshake, Message, MessageCodec};
use crate::peer::stats::PeerStats;

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
        let join_handle = tokio::spawn(async move {
            let result = run(addr, socket, events_tx.clone(), rx, token_clone, &download).await;
            if let Err(err) = result {
                warn!("[{}] error encountered during run: {}", addr, err);
            }
            info!("peer {} disconnected", addr);
            events_tx
                .send(Event::Disconnected(addr))
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
    events_tx: Sender<Event>,
    mut rx: Receiver<Message>,
    cancellation_token: CancellationToken,
    download: &Download,
) -> anyhow::Result<()> {
    let handshake = Handshake::new(
        download.torrent.info.info_hash.clone(),
        download.config.client_id.clone(),
    );
    // The largest message we should accept is a `Piece` message, which should
    // always have size of `config.block_size` + 9
    let config = &download.config;
    let max_size = (config.block_size.bytes() as usize) + 9;
    let socket = establish_connection(addr, socket, handshake, config.connect_timeout).await?;
    let mut messages = Framed::new(socket, MessageCodec::new(max_size));
    let mut update_stats = tokio::time::interval(config.update_stats_interval);
    let mut stats = PeerStats::default();

    loop {
        let start = Instant::now();
        tokio::select! {
            _ = cancellation_token.cancelled() => {
                break;
            }
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
                    let event = Event::MessageReceived(addr, message);
                    events_tx.send(event).await?;
                }
                Err(err) => {
                    warn!("[{}] failed to decode message: {}", &addr, err);
                    if err.kind() == ErrorKind::UnexpectedEof {
                        info!("[{}] socket closed, shutting down...", &addr);
                        break;
                    }
                }
            },
            _ = update_stats.tick() => {
                events_tx.send(Event::StatsUpdated(addr, stats.clone())).await?;
            },
        }
    }

    messages.flush().await?;
    Ok(())
}

/// Establish connection with a peer and exchange handshakes.
async fn establish_connection(
    addr: SocketAddr,
    socket: Option<TcpStream>,
    handshake: Handshake,
    connect_timeout: Duration,
) -> anyhow::Result<TcpStream> {
    let (socket, handshake_got) = match socket {
        Some(mut socket) => {
            info!("accepted connection from {}", &addr);
            // Wait for handshake from peer
            let handshake_got = Handshake::decode(&mut socket).await?;
            // Send handshake second
            handshake.encode(&mut socket).await?;
            (socket, handshake_got)
        }
        None => {
            info!("connecting to {}...", &addr);
            let mut socket = timeout(connect_timeout, TcpStream::connect(addr)).await??;
            // Send handshake first
            handshake.encode(&mut socket).await?;
            // Wait for handshake from peer
            let handshake_got = Handshake::decode(&mut socket).await?;
            info!("connected to {}", &addr);
            (socket, handshake_got)
        }
    };
    if !handshake_got.is_standard_protocol() {
        return Err(anyhow!(
            "invalid handshake protocol: {}",
            handshake_got.protocol
        ));
    }
    if handshake.info_hash != handshake_got.info_hash {
        return Err(anyhow!("info hash mismatch"));
    }
    Ok(socket)
}
