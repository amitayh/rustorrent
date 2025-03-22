use std::net::SocketAddr;

use bencoding::Value;
use log::info;
use peer::Peer;
use tokio::{fs::File, net::TcpListener};
use tracker::Event;

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

    // Parse .torrent file
    let torrent = load_torrent(&path).await?;

    //let temp_dir = {
    //    // Create temp dir
    //    let mut path = PathBuf::from("/tmp");
    //    path.push(format!("torrent-{}", hex::encode(torrent.info.info_hash.0)));
    //    tokio::fs::create_dir(&path).await?;
    //    path
    //};

    let response = tracker::request(&torrent, &config, Some(Event::Started)).await?;

    {
        // Reannounce
        let mut interval = tokio::time::interval(response.interval);
        tokio::spawn(async move {
            loop {
                interval.tick().await;
                info!("refreshing peers list...");
            }
        });
    }

    // Run server
    let address = SocketAddr::new("::".parse()?, config.port);
    info!("starting server...");
    let listener = TcpListener::bind(&address).await?;
    info!("listening on {}", &address);
    let (mut client, tx) = Peer::new(
        listener,
        torrent.info,
        path.into(),
        peer::Config::default(),
        false,
    )
    .await;

    let len = response.peers.len();
    for (i, peer_info) in response.peers.into_iter().enumerate() {
        let addr = peer_info.address;
        info!("connecting to peer {}/{}: {}...", i + 1, len, addr);
        tx.send(peer::Event::Connect(addr)).await?;
    }
    client.start().await?;

    tokio::signal::ctrl_c().await?;

    Ok(())
}
