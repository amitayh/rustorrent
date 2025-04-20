use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use log::warn;
use tokio::sync::Mutex;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinSet;

use crate::client::Download;
use crate::client::Notification;
use crate::command::Command;
use crate::event::Event;
use crate::peer::connection::Connection;
use crate::storage::FileReader;
use crate::storage::FileWriter;
use crate::tracker::Tracker;

/// Executes commands for managing peer connections, file I/O, and tracker communication
/// in a BitTorrent download.
pub struct CommandExecutor {
    /// The download configuration and metadata
    download: Arc<Download>,
    /// Active peer connections mapped by their socket addresses
    peers: HashMap<SocketAddr, Connection>,
    /// Reader for accessing downloaded file data
    reader: Arc<FileReader>,
    /// Writer for saving downloaded pieces to disk
    writer: Arc<Mutex<FileWriter>>,
    /// Tracker connection for peer discovery and stats reporting
    tracker: Tracker,
    /// Channel for sending events to the event handler
    events: Sender<Event>,
    /// Channel for sending notifications about download progress
    notifications: Sender<Notification>,
}

impl CommandExecutor {
    pub fn new(
        download: Arc<Download>,
        events: Sender<Event>,
        notifications: Sender<Notification>,
    ) -> Self {
        let peers = HashMap::new();
        let reader = Arc::new(FileReader::new(Arc::clone(&download)));
        let writer = Arc::new(Mutex::new(FileWriter::new(
            Arc::clone(&download),
            events.clone(),
        )));
        let tracker = Tracker::spawn(Arc::clone(&download), events.clone());
        Self {
            peers,
            download,
            reader,
            writer,
            tracker,
            events,
            notifications,
        }
    }

    pub fn execute(&mut self, command: Command) -> ExecutionResult {
        match command {
            Command::EstablishConnection(addr, socket) if !self.peers.contains_key(&addr) => {
                let conn = Connection::spawn(
                    addr,
                    socket,
                    self.events.clone(),
                    Arc::clone(&self.download),
                );
                self.peers.insert(addr, conn);
            }

            Command::Send(addr, message) => {
                let peer = self.peers.get(&addr).expect("invalid peer");
                peer.send(message);
            }

            Command::Broadcast(message) => {
                for peer in self.peers.values() {
                    peer.send(message.clone());
                }
            }

            Command::Upload(addr, block) => {
                let peer = self.peers.get(&addr).expect("invalid peer");
                let reader = Arc::clone(&self.reader);
                let tx = peer.tx.clone();
                tokio::spawn(async move { reader.read(block, tx).await });
            }

            Command::IntegrateBlock(block_data) => {
                let writer = Arc::clone(&self.writer);
                tokio::spawn(async move { writer.lock().await.write(block_data).await });
            }

            Command::RemovePeer(addr) => {
                let peer = self.peers.remove(&addr).expect("invalid peer");
                peer.abort();
            }

            Command::UpdateStats(stats) => {
                self.tracker
                    .update_progress(stats.downloaded, stats.uploaded);
                self.send_notification(if stats.download_complete() {
                    Notification::DownloadComplete
                } else {
                    Notification::Stats(stats)
                });
            }

            Command::Shutdown => {
                self.send_notification(Notification::ShuttingDown);
                return ExecutionResult::Stop;
            }

            command => warn!("unhandled command: {:?}", command),
        }
        ExecutionResult::Continue
    }

    pub async fn shutdown(mut self) {
        let mut join_set = JoinSet::new();
        join_set.spawn(async move { self.tracker.shutdown().await });
        for (_, peer) in self.peers.drain() {
            join_set.spawn(async move { peer.shutdown().await });
        }
        while let Some(result) = join_set.join_next().await {
            if let Err(err) = result {
                warn!("error encountered while shutting down: {:?}", err);
            }
        }
    }

    fn send_notification(&self, notification: Notification) {
        if let Err(err) = self.notifications.try_send(notification) {
            warn!("failed sending notification: {:?}", err);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionResult {
    Continue,
    Stop,
}
