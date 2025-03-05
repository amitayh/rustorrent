use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;

use bencoding::value::Value;
use log::{info, warn};
use peer::Peer;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::{fs::File, net::TcpListener};

use crate::codec::{AsyncDecoder, AsyncEncoder};
use crate::peer::PeerId;
use crate::peer::message::{Handshake, Message};
use crate::torrent::Torrent;

mod bencoding;
mod codec;
mod crypto;
mod peer;
mod torrent;
mod tracker;

struct Config {
    clinet_id: PeerId,
    port: u16,
}

async fn load_torrent(path: &str) -> anyhow::Result<Torrent> {
    let mut file = File::open(path).await?;
    let value = Value::decode(&mut file).await?;
    Torrent::try_from(value)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let config = Config {
        clinet_id: PeerId::random(),
        port: 6881,
    };
    let path = std::env::args().nth(1).expect("file must be provided");
    let torrent = load_torrent(&path).await?;
    let handshake = Arc::new(Handshake::new(
        torrent.info.info_hash.clone(),
        config.clinet_id.clone(),
    ));

    let address = SocketAddr::new("::".parse().unwrap(), config.port);
    info!("starting server...");
    let listener = TcpListener::bind(&address).await?;
    info!("listening on {}", &address);

    let h1 = Arc::clone(&handshake);
    tokio::spawn(async move {
        while let Ok((socket, addr)) = listener.accept().await {
            info!("new peer connected {}", addr);
            let mut peer = Peer::new(socket);
            peer.wait_for_handshake().await?;
            peer.send_handshake(&h1).await?;
            tokio::spawn(async move {
                peer.handle().await.unwrap();
            });
        }
        Ok::<(), std::io::Error>(())
    });

    let response = tracker::request(&torrent, &config, None).await?;
    for peer_info in response.peers {
        let h2 = Arc::clone(&handshake);
        tokio::spawn(async move {
            info!("connecting to peer {}...", peer_info.address);
            match TcpStream::connect(peer_info.address).await {
                Ok(socket) => {
                    info!("connected");
                    let mut peer = Peer::new(socket);
                    peer.send_handshake(&h2).await.unwrap();
                    peer.wait_for_handshake().await.unwrap();

                    let peer_ids_match = peer_info
                        .peer_id
                        .as_ref()
                        .map_or(true, |peer_id| peer_id == &h2.peer_id);
                    if !peer_ids_match {
                        warn!("peer id mismatch");
                        // disconnect peer
                    }

                    tokio::spawn(async move {
                        peer.handle().await.unwrap();
                    });
                }
                Err(err) => log::warn!("unable to connect to {}: {:?}", peer_info.address, err),
            }
        });
    }

    tokio::signal::ctrl_c().await?;

    Ok(())
}
