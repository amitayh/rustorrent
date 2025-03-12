#![allow(dead_code)]
use std::fmt::Debug;
use std::io::{Error, ErrorKind};
use std::{io::Result, net::SocketAddr};

use log::{info, warn};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, Receiver, Sender};

use crate::peer::message::Handshake;
use crate::peer::message::Message;
use crate::peer::peer_id::PeerId;
use crate::{
    codec::{AsyncDecoder, AsyncEncoder},
    torrent::Info,
};

struct PeerMessage(SocketAddr, Message);

struct Peer {
    peer_id: PeerId,
    listener: TcpListener,
    torrent_info: Info,
    tx: Sender<PeerMessage>,
    rx: Receiver<PeerMessage>,
    temp_has_file: bool,
}

impl Peer {
    fn new(listener: TcpListener, torrent_info: Info) -> Self {
        let (tx, rx) = mpsc::channel(128);
        Self {
            peer_id: PeerId::random(),
            listener,
            torrent_info,
            tx,
            rx,
            temp_has_file: false,
        }
    }

    fn temp_has_file(&mut self) {
        self.temp_has_file = true;
    }

    fn address(&self) -> Result<SocketAddr> {
        self.listener.local_addr()
    }

    fn handshake(&self) -> Handshake {
        Handshake::new(self.torrent_info.info_hash.clone(), self.peer_id.clone())
    }

    async fn start(&mut self) -> Result<()> {
        loop {
            tokio::select! {
                Some(PeerMessage(addr, message)) = self.rx.recv() => {
                   self.handle_message(addr, message);
                }
                Ok((socket, addr)) = self.listener.accept() => {
                    info!("got new connection from {}", &addr);
                    let mut handler = ConnectionHandler::new(addr, socket, self.tx.clone());
                    handler.wait_for_handshake().await?;
                    handler.send(self.handshake()).await?;
                    if self.temp_has_file {
                        handler.send(Message::Have { piece: 0 }).await?;
                    }
                    tokio::spawn(async move { handler.start().await });
                }
            }
        }
    }

    async fn connect(&self, addr: SocketAddr) -> Result<()> {
        let socket = TcpStream::connect(addr).await?;
        let mut handler = ConnectionHandler::new(addr, socket, self.tx.clone());
        handler.send(self.handshake()).await?;
        handler.wait_for_handshake().await?;
        tokio::spawn(async move { handler.start().await });
        Ok(())
    }

    async fn handle_message(&mut self, addr: SocketAddr, message: Message) {
        match message {
            _ => todo!(),
        }
    }
}

struct ConnectionHandler {
    addr: SocketAddr,
    socket: TcpStream,
    tx: Sender<PeerMessage>,
}

impl ConnectionHandler {
    fn new(addr: SocketAddr, socket: TcpStream, tx: Sender<PeerMessage>) -> Self {
        Self { addr, socket, tx }
    }

    pub async fn wait_for_handshake(&mut self) -> Result<()> {
        let handshake = Handshake::decode(&mut self.socket).await?;
        info!("[{}] < got handshake {:?}", &self.addr, handshake);
        Ok(())
    }

    pub async fn send(&mut self, message: impl AsyncEncoder + Debug) -> Result<()> {
        message.encode(&mut self.socket).await?;
        info!("[{}] > sent message: {:?}", &self.addr, message);
        Ok(())
    }

    pub async fn start(&mut self) -> Result<()> {
        loop {
            match Message::decode(&mut self.socket).await {
                Ok(message) => {
                    info!("[{}] < got message: {:?}", &self.addr, &message);
                    self.tx
                        .send(PeerMessage(self.addr, message))
                        .await
                        .map_err(|err| Error::new(ErrorKind::Other, err))?;
                }
                Err(err) => warn!("[{}] failed to decode message: {}", &self.addr, err),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::{task::JoinSet, time::timeout};

    use super::*;

    #[tokio::test]
    async fn one_seeder_one_leecher() {
        env_logger::init();

        let torrent_info = Info::load("assets/alice_in_wonderland.txt").await.unwrap();

        let mut seeder = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mut peer = Peer::new(listener, torrent_info.clone());
            peer.temp_has_file();
            peer
        };
        let seeder_addr = seeder.address().unwrap();
        let mut leecher = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            Peer::new(listener, torrent_info)
        };

        let mut set = JoinSet::new();
        set.spawn(async move { seeder.start().await });
        set.spawn(async move {
            leecher.connect(seeder_addr).await.unwrap();
            leecher.start().await
        });

        let result = timeout(Duration::from_secs(1), set.join_all()).await;

        // assert leecher has file
        assert!(result.is_ok());
    }
}
