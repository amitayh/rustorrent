use std::net::{Ipv4Addr, SocketAddr};

use bencoding::parser::Parser;
use log::info;
use tokio::{fs::File, net::TcpListener};
use tracker::response::PeerId;

use crate::torrent::Torrent;

mod bencoding;
mod client;
mod crypto;
mod torrent;
mod tracker;

struct Config {
    clinet_id: PeerId,
    port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let config = Config {
        clinet_id: PeerId::random(),
        port: 6881,
    };

    let args: Vec<_> = std::env::args().collect();
    let mut file = File::open(&args[1]).await?;
    let mut parser = Parser::new();
    tokio::io::copy(&mut file, &mut parser).await?;
    let value = parser.result()?;
    let torrent = Torrent::try_from(value)?;
    let _ = tracker::request(&torrent, &config, None).await?;

    let address = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), config.port);
    info!("starting server...");
    let listener = TcpListener::bind(&address).await?;
    info!("listening on {}", &address);

    loop {
        let (_socket, addr) = listener.accept().await?;
        info!("new client connected {}", addr);
        // Clone the handle to the hash map.
        //let db = db.clone();

        //tokio::spawn(async move {
        //    process(socket, db).await;
        //});
    }
}
