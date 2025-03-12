#![allow(dead_code)]
use std::{io::Result, net::SocketAddr};

use log::info;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};

use crate::peer::message::Handshake;
use crate::peer::peer_id::PeerId;
use crate::{
    codec::{AsyncDecoder, AsyncEncoder},
    torrent::Info,
};

struct Peer {
    peer_id: PeerId,
    listener: TcpListener,
    torrent_info: Info,
}

impl Peer {
    fn new(listener: TcpListener, torrent_info: Info) -> Self {
        Self {
            peer_id: PeerId::random(),
            listener,
            torrent_info,
        }
    }

    fn address(&self) -> Result<SocketAddr> {
        self.listener.local_addr()
    }

    fn handshake(&self) -> Handshake {
        Handshake::new(self.torrent_info.info_hash.clone(), self.peer_id.clone())
    }

    async fn start(&self) -> Result<()> {
        loop {
            let (mut socket, _addr) = self.listener.accept().await?;
            wait_for_handshake(&mut socket).await?;
            send_handshake(&mut socket, &self.handshake()).await?;
            tokio::spawn(async move {
                process(socket).await.unwrap();
            });
        }
    }

    async fn connect(&self, addr: SocketAddr) -> Result<()> {
        let mut socket = TcpStream::connect(addr).await?;
        send_handshake(&mut socket, &self.handshake()).await?;
        wait_for_handshake(&mut socket).await?;
        process(socket).await?;
        Ok(())
    }
}

async fn process(mut socket: TcpStream) -> Result<()> {
    socket.read_u8().await?;
    Ok(())
}

async fn wait_for_handshake(socket: &mut TcpStream) -> Result<()> {
    let handshake = Handshake::decode(socket).await?;
    info!("< got handshake {:?}", handshake);
    Ok(())
}

pub async fn send_handshake(socket: &mut TcpStream, handshake: &Handshake) -> Result<()> {
    handshake.encode(socket).await?;
    info!("> sent handshake");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use tokio::{task::JoinSet, time::timeout};

    use super::*;

    #[tokio::test]
    async fn one_seeder_one_leecher() {
        env_logger::init();

        let torrent_info = Info::load("assets/alice_in_wonderland.txt").await.unwrap();

        let seeder = {
            // TODO: make seeder have the entire file
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            Arc::new(Peer::new(listener, torrent_info.clone()))
        };
        let leecher = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            Arc::new(Peer::new(listener, torrent_info))
        };

        let mut set = JoinSet::new();
        {
            let seeder = Arc::clone(&seeder);
            set.spawn(async move { seeder.start().await });
        }
        {
            let leecher = Arc::clone(&leecher);
            set.spawn(async move { leecher.start().await });
        }
        set.spawn(async move {
            let seeder_address = seeder.address().unwrap();
            leecher.connect(seeder_address).await
        });

        let result = timeout(Duration::from_secs(1), set.join_all()).await;

        // assert leecher has file
        assert!(result.is_ok());
    }
}
