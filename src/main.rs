use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;

use log::info;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::{fs::File, net::TcpListener};

use crate::bencoding::parser::Parser;
use crate::peer::PeerId;
use crate::peer::message::{Handshake, Message};
use crate::torrent::Torrent;

mod bencoding;
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
    let mut parser = Parser::new();
    tokio::io::copy(&mut file, &mut parser).await?;
    let value = parser.result()?;
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

    let state = Arc::new(Mutex::new(HashSet::new()));

    tokio::spawn(async move {
        while let Ok((_socker, addr)) = listener.accept().await {
            info!("new peer connected {}", addr);
            {
                let mut peers = state.lock().await;
                peers.insert(addr);
            }
            // Clone the handle to the hash map.
            //let db = db.clone();

            //tokio::spawn(async move {
            //    process(socket, db).await;
            //});
        }
    });

    let path = std::env::args().nth(1).expect("file must be provided");
    let torrent = load_torrent(&path).await?;
    let response = tracker::request(&torrent, &config, None).await?;
    for peer in response.peers {
        //tokio::spawn(async move {
        info!("connecting to peer {}...", peer.address);
        match TcpStream::connect(peer.address).await {
            Ok(mut stream) => {
                info!("connected");
                let handshake =
                    Handshake::new(torrent.info.info_hash.clone(), config.clinet_id.clone());
                stream.writable().await?;
                handshake.write(&mut stream).await?;
                info!("sent handshake");

                stream.readable().await?;
                let handshake = Handshake::read(&mut stream).await?;
                info!("got handshake {:?}", handshake);

                loop {
                    stream.readable().await?;
                    let message = Message::read(&mut stream).await?;
                    info!("got message {:?}", message);
                }
            }
            Err(err) => log::warn!("unable to connect to {}: {:?}", peer.address, err),
        }
        //});
    }

    Ok(())
}
