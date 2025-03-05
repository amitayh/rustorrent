use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;

use bencoding::value::Value;
use log::{info, warn};
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

    let address = SocketAddr::new("::".parse().unwrap(), config.port);
    info!("starting server...");
    let listener = TcpListener::bind(&address).await?;
    info!("listening on {}", &address);

    tokio::spawn(async move {
        while let Ok((mut socket, addr)) = listener.accept().await {
            info!("new peer connected {}", addr);
            let handshake = Handshake::decode(&mut socket).await?;
            info!("< got handshake {:?}", handshake);

            handshake.encode(&mut socket).await?;
            info!("> sent handshake");

            loop {
                let message = Message::decode(&mut socket).await?;
                info!("< got message {:?}", message);
            }
        }
        Ok::<(), std::io::Error>(())
    });

    let path = std::env::args().nth(1).expect("file must be provided");
    let torrent = load_torrent(&path).await?;
    let response = tracker::request(&torrent, &config, None).await?;
    for peer in response.peers {
        //tokio::spawn(async move {
        info!("connecting to peer {}...", peer.address);
        match TcpStream::connect(peer.address).await {
            Ok(mut socket) => {
                info!("connected");
                let handshake =
                    Handshake::new(torrent.info.info_hash.clone(), config.clinet_id.clone());
                handshake.encode(&mut socket).await?;
                info!("> sent handshake");

                let handshake = Handshake::decode(&mut socket).await?;
                info!("< got handshake {:?}", handshake);

                let peer_ids_match = peer
                    .peer_id
                    .as_ref()
                    .map_or(true, |peer_id| peer_id == &handshake.peer_id);
                if !peer_ids_match {
                    warn!("peer id mismatch");
                    // disconnect peer
                }

                loop {
                    let message = Message::decode(&mut socket).await?;
                    info!("< got message {:?}", message);
                }
            }
            Err(err) => log::warn!("unable to connect to {}: {:?}", peer.address, err),
        }
        //});
    }

    Ok(())
}
