use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bencoding::value::Value;
use log::{info, warn};
use peer::SharedState;
use peer::connection::PeerConnection;
use peer::piece_selector::PieceSelector;
use size::Size;
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::{fs::File, net::TcpListener};
use tracker::Event;

use crate::codec::AsyncDecoder;
use crate::peer::message::Handshake;
use crate::peer::peer_id::PeerId;
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

    // Parse .torrent file
    let torrent = load_torrent(&path).await?;

    let temp_dir = {
        // Create temp dir
        let mut path = PathBuf::from("/tmp");
        path.push(format!("torrent-{}", hex::encode(torrent.info.info_hash.0)));
        tokio::fs::create_dir(&path).await?;
        path
    };

    let state = Arc::new(RwLock::new(SharedState::new(temp_dir)));

    // Run server
    let address = SocketAddr::new("::".parse()?, config.port);
    info!("starting server...");
    let listener = TcpListener::bind(&address).await?;
    info!("listening on {}", &address);

    /*
        // Listen for messages from peers
        let (tx, mut rx) = mpsc::channel(100);
        tokio::spawn(async move {
            while let Some(message) = rx.recv().await {
                info!("got message from peer: {:?}", message);
            }
        });
    */

    {
        // Choke / unchoke
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        tokio::spawn(async move {
            loop {
                interval.tick().await;
                info!("running choke algorithm...");
            }
        });
    }

    let handshake = Arc::new(Handshake::new(
        torrent.info.info_hash.clone(),
        config.clinet_id.clone(),
    ));
    {
        let handshake = Arc::clone(&handshake);
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            loop {
                let (socket, addr) = listener.accept().await?;
                let mut peer = PeerConnection::new(addr, socket, Arc::clone(&state)).await;
                peer.wait_for_handshake().await?;
                peer.send_handshake(&handshake).await?;
                tokio::spawn(async move { peer.process().await });
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
            //while let Ok((socket, addr)) = listener.accept().await {
            //    let mut peer = Peer::new(socket);
            //    peer.wait_for_handshake().await?;
            //    peer.send_handshake(&handshake).await?;
            //    {
            //        let mut foo = state.lock().await;
            //        foo.peer_connected(addr, peer);
            //    }
            //    tokio::spawn(async move {
            //        loop {
            //            let message = peer.wait_for_message().await?;
            //            tx.send(message).await?;
            //        }
            //    });
            //}
            //Ok::<(), std::io::Error>(())
        });
    }

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
    {
        // Request pieces
        let mut piece_selector = PieceSelector::new(
            torrent.info.piece_length,
            torrent.info.download_type.length(),
            Size::from_kibibytes(16),
        );
        tokio::spawn(async move {
            loop {
                info!("selecting pieces to request...");
                if let Some((addr, block)) = piece_selector.next().await {
                    info!("request {:?} from {}", block, addr);
                }
            }
        });
    }

    let len = response.peers.len();
    for (i, peer_info) in response.peers.into_iter().enumerate() {
        let handshake = Arc::clone(&handshake);
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let address = peer_info.address;
            info!("connecting to peer {}/{}: {}...", i + 1, len, address);
            match TcpStream::connect(address).await {
                Ok(socket) => {
                    info!("connected");
                    let mut peer = PeerConnection::new(address, socket, Arc::clone(&state)).await;
                    peer.send_handshake(&handshake).await?;
                    peer.wait_for_handshake().await?;

                    let peer_ids_match = peer_info
                        .peer_id
                        .as_ref()
                        .is_none_or(|peer_id| peer_id == &handshake.peer_id);
                    if !peer_ids_match {
                        warn!("peer id mismatch");
                        // disconnect peer
                    }
                    peer.process().await?;
                }
                Err(err) => log::warn!("unable to connect to {}: {:?}", address, err),
            }
            Ok::<(), anyhow::Error>(())
        });
    }

    tokio::signal::ctrl_c().await?;

    Ok(())
}
