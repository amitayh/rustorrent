use std::io::{Result, SeekFrom};
use std::net::SocketAddr;
use std::sync::Arc;

use log::{info, warn};
use size::Size;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::sync::mpsc::Receiver;
use tokio::time::Instant;

use crate::codec::{AsyncDecoder, AsyncEncoder};
use crate::peer::message::{Handshake, Message};
use crate::peer::{PeerCommand, SharedState};

pub struct PeerConnection {
    addr: SocketAddr,
    socket: TcpStream,
    state: Arc<RwLock<SharedState>>,
    rx: Receiver<PeerCommand>,
}

impl PeerConnection {
    pub async fn new(addr: SocketAddr, socket: TcpStream, state: Arc<RwLock<SharedState>>) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        let peer = Self {
            addr,
            socket,
            state: Arc::clone(&state),
            rx,
        };
        state.write().await.peer_connected(addr, tx);
        peer
    }

    pub async fn wait_for_handshake(&mut self) -> Result<()> {
        let handshake = Handshake::decode(&mut self.socket).await?;
        info!("< got handshake {:?}", handshake);
        Ok(())
    }

    pub async fn send_handshake(&mut self, handshake: &Handshake) -> Result<()> {
        handshake.encode(&mut self.socket).await?;
        info!("> sent handshake");
        Ok(())
    }

    pub async fn process(&mut self) -> anyhow::Result<()> {
        tokio::select! {
            Some(command) = self.rx.recv() => {
                info!("< got command {:?}", command);
                match command {
                    PeerCommand::Send(message) => {
                        message.encode(&mut self.socket).await?;
                    }
                }
            }
            message = Message::decode(&mut self.socket) => {
                info!("< got messsage {:?}", message);
                match message {
                    Ok(Message::KeepAlive) => {
                        // No-op
                    }
                    Ok(Message::Choke) => {
                        let mut state = self.state.write().await;
                        state.peer_choked(&self.addr);
                    }
                    Ok(Message::Unchoke) => {
                        let mut state = self.state.write().await;
                        state.peer_unchoked(&self.addr);
                        Message::Unchoke.encode(&mut self.socket).await?;
                    }
                    Ok(Message::Interested) => {
                        let mut state = self.state.write().await;
                        state.peer_interested(&self.addr);
                    }
                    Ok(Message::NotInterested) => {
                        let mut state = self.state.write().await;
                        state.peer_not_interested(&self.addr);
                    }
                    // TODO: celan up duplication with bitfield
                    Ok(Message::Have { piece }) => {
                        let interested = {
                            let mut state = self.state.write().await;
                            state.peer_has_piece(self.addr, piece)
                        };
                        if interested {
                            Message::Interested.encode(&mut self.socket).await?;
                        }
                    }
                    Ok(Message::Bitfield(bitset)) => {
                        let interested = {
                            let mut state = self.state.write().await;
                            state.peer_has_pieces(&self.addr, bitset)
                        };
                        if interested {
                            Message::Interested.encode(&mut self.socket).await?;
                        }
                    }
                    Ok(Message::Request(block)) => {
                        // Upload to peer
                        let file_path = {
                            let state = self.state.read().await;
                            match state.file_path_for_upload(&self.addr, block.piece) {
                                Ok(path) => path,
                                Err(err) => {
                                    warn!("{}", err);
                                    return Ok(());
                                }
                            }
                        };
                        let mut file = File::open(file_path).await?;
                        file.seek(SeekFrom::Start(block.offset as u64)).await?;
                        let mut reader = file.take(block.length as u64);
                        let message = Message::Piece(block.clone());
                        let transfer_begin = Instant::now();
                        message.encode(&mut self.socket).await?;
                        let bytes = tokio::io::copy(&mut reader, &mut self.socket).await? as usize;
                        let duration = Instant::now() - transfer_begin;
                        let size = Size::from_bytes(bytes);
                        if bytes != block.length {
                            warn!("peer {} requested block {:?}, actually transferred {} bytes",
                                self.addr, block, bytes);
                        }

                        let mut state = self.state.write().await;
                        state.update_upload_rate(&self.addr, size, duration);
                    }
                    Ok(Message::Piece(block)) => {
                        // Download from peer
                        let file_path = {
                            let state = self.state.read().await;
                            state.file_path(block.piece)
                        };
                        // TODO: check block is in flight
                        let mut buf = vec![0; block.length];
                        let transfer_begin = Instant::now();
                        self.socket.read_exact(&mut buf).await?;
                        let duration = Instant::now() - transfer_begin;
                        let size = Size::from_bytes(block.length);
                        let mut file = OpenOptions::new().write(true).open(file_path).await?;
                        file.seek(SeekFrom::Start(block.offset as u64)).await?;
                        file.write_all(&buf).await?;

                        // TODO: update block was downloaded
                        let mut state = self.state.write().await;
                        state.update_download_rate(&self.addr, size, duration);
                    }
                    Ok(message) => warn!("unhandled message {:?}", message),
                    Err(err) => warn!("error decoding message: {}", err)
                }
            }
        };
        Ok(())
    }
}
