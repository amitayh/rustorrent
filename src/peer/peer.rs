#![allow(dead_code)]
use std::{io::Result, net::SocketAddr};

use log::info;
use tokio::net::{TcpListener, TcpStream};

use crate::peer::message::Handshake;
use crate::{
    codec::{AsyncDecoder, AsyncEncoder},
    torrent::Info,
};

struct Peer {
    listener: TcpListener,
    torrent_info: Info,
}

impl Peer {
    fn new(listener: TcpListener, torrent_info: Info) -> Self {
        Self {
            listener,
            torrent_info,
        }
    }

    async fn start(&self) -> Result<()> {
        loop {
            let (mut socket, addr) = self.listener.accept().await?;
            wait_for_handshake(&mut socket).await?;
            //send_handshake(&mut socket, handshake).await?;
        }
    }

    fn address(&self) -> Result<SocketAddr> {
        self.listener.local_addr()
    }
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
    use std::time::Duration;

    use size::Size;
    use tokio::{io::AsyncWriteExt, net::TcpStream, task::JoinSet, time::timeout};

    use crate::crypto::Sha1;

    use super::*;

    #[tokio::test]
    async fn one_seeder_one_leecher() {
        env_logger::init();

        let torrent_info = Info {
            info_hash: Sha1([0; 20]),
            piece_length: Size::from_bytes(0),
            pieces: vec![],
            download_type: crate::torrent::DownloadType::SingleFile {
                name: "".to_string(),
                length: Size::from_bytes(0),
                md5sum: None,
            },
        };

        let seeder = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            Peer::new(listener, torrent_info.clone())
        };
        let leecher = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            Peer::new(listener, torrent_info)
        };

        let mut set = JoinSet::new();
        set.spawn(async move { seeder.start().await });
        set.spawn(async move { leecher.start().await });

        let result = timeout(Duration::from_secs(1), set.join_all()).await;

        // assert leecher has file
        assert!(result.is_ok());
    }
}
