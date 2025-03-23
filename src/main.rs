use std::net::SocketAddr;

use bencoding::Value;
use log::info;
use peer::Peer;
use tokio::{fs::File, net::TcpListener};

use crate::codec::AsyncDecoder;
use crate::peer::PeerId;
use crate::torrent::Torrent;

mod bencoding;
mod codec;
mod crypto;
mod message;
mod peer;
mod torrent;
mod tracker;

async fn load_torrent(path: &str) -> anyhow::Result<Torrent> {
    let mut file = File::open(path).await?;
    let value = Value::decode(&mut file).await?;
    Torrent::try_from(value)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let config = tracker::Config {
        clinet_id: PeerId::random(),
        port: 6881,
    };
    let path = std::env::args().nth(1).expect("file must be provided");

    // Parse .torrent file
    let torrent = load_torrent(&path).await?;

    // Run server
    let address = SocketAddr::new("::".parse()?, config.port);
    info!("starting server...");
    let listener = TcpListener::bind(&address).await?;
    info!("listening on {}", &address);
    let mut client = Peer::new(
        listener,
        torrent,
        "/tmp/foo".into(),
        peer::Config::default(),
        false,
    )
    .await;

    client.start().await?;

    Ok(())
}
