use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use log::{error, info};
use mainline::{Dht as MainlineDht, Id};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::client::Download;
use crate::core::GracefulShutdown;
use crate::event::Event;

pub struct Dht {
    graceful_shutdown: GracefulShutdown<Result<()>>,
}

impl Dht {
    pub fn spawn(download: Arc<Download>, events_tx: mpsc::Sender<Event>) -> Self {
        let cancellation_token = CancellationToken::new();
        let token_clone = cancellation_token.clone();
        let join_handle = tokio::spawn(async move {
            let info_hash = Id::from(download.torrent.info.info_hash.0);
            let port = download.config.port;

            info!("starting DHT node...");
            let dht = MainlineDht::client().map_err(|e| anyhow::anyhow!("{}", e))?;
            let async_dht = dht.as_async();

            if async_dht.bootstrapped().await {
                info!("DHT node bootstrapped successfully");
            } else {
                error!("DHT node failed to bootstrap");
                return Ok(());
            }

            // Announce ourselves on the DHT
            match async_dht.announce_peer(info_hash, Some(port)).await {
                Ok(_) => info!("announced on DHT for info_hash {:?}", info_hash),
                Err(err) => error!("failed to announce on DHT: {:?}", err),
            }

            // Get peers from DHT
            let mut stream = async_dht.get_peers(info_hash);

            loop {
                tokio::select! {
                    result = stream.next() => {
                        match result {
                            Some(peers) => {
                                info!("DHT discovered {} peer(s)", peers.len());
                                for peer in peers {
                                    let addr = SocketAddr::from(peer);
                                    if events_tx.send(Event::ConnectionRequested(addr)).await.is_err() {
                                        info!("events channel closed, DHT shutting down");
                                        return Ok(());
                                    }
                                }
                            }
                            None => {
                                info!("DHT get_peers stream ended");
                                break;
                            }
                        }
                    }
                    _ = token_clone.cancelled() => {
                        info!("DHT shutting down...");
                        break;
                    }
                }
            }

            Ok(())
        });
        let graceful_shutdown = GracefulShutdown::new(join_handle, cancellation_token);
        Self { graceful_shutdown }
    }

    pub async fn shutdown(self) -> Result<()> {
        self.graceful_shutdown.shutdown().await?
    }
}
