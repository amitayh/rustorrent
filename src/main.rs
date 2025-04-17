use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bencoding::Value;
use log::{info, warn};
use peer::{Client, Config, Download, Notification};
use tokio::{fs::File, net::TcpListener};

use crate::codec::AsyncDecoder;
use crate::torrent::Torrent;

mod bencoding;
mod codec;
mod command;
mod crypto;
mod event;
mod message;
mod peer;
mod scheduler;
mod storage;
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

    let path = std::env::args().nth(1).expect("file must be provided");

    // Parse .torrent file
    let torrent = load_torrent(&path).await?;
    let download = Arc::new(Download {
        torrent,
        config: Config::new("/tmp/foo".into()),
    });

    // Run server
    let address = SocketAddr::new("::".parse()?, download.config.port);
    info!("starting server...");
    let listener = TcpListener::bind(&address).await?;
    info!("listening on {}", &address);
    let mut client = Client::spawn(listener, download, false).await;

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("received ctrl-c signal, initiating shutdown...");
                break;
            }
            Some(notification) = client.notifications.recv() => match notification {
                Notification::DownloadComplete => {
                    info!("download complete!");
                },
                Notification::Stats(stats) => {
                    println!(
                        "Downloaded {}/{} pieces ({:.2}%) | Down: {} | Up: {} | Peers: {}",
                        stats.completed_pieces,
                        stats.total_pieces,
                        stats.completed_percentage(),
                        stats.download_rate,
                        stats.upload_rate,
                        stats.connected_peers
                    );
                }
            }
        }
    }

    let result = tokio::time::timeout(Duration::from_secs(10), client.shutdown()).await;
    if result.is_err() {
        warn!("shutdown timed out after 10 seconds");
    }

    Ok(())
}
