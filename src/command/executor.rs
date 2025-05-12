use std::sync::Arc;

use log::warn;
use tokio::sync::Mutex;
use tokio::sync::mpsc::Sender;

use crate::client::Download;
use crate::client::Notification;
use crate::command::Command;
use crate::event::Event;
use crate::peer::connection_manager::ConnectionManager;
use crate::storage::FileReader;
use crate::storage::FileWriter;
use crate::tracker::Tracker;

/// Executes commands for managing peer connections, file I/O, and tracker communication
/// in a BitTorrent download.
pub struct CommandExecutor {
    /// Active peer connections
    connection_manager: ConnectionManager,
    /// Reader for accessing downloaded file data
    reader: Arc<FileReader>,
    /// Writer for saving downloaded pieces to disk
    writer: Arc<Mutex<FileWriter>>,
    /// Tracker connection for peer discovery and stats reporting
    tracker: Tracker,
    /// Channel for sending notifications about download progress
    notifications: Sender<Notification>,
}

impl CommandExecutor {
    pub fn new(
        download: Arc<Download>,
        events: Sender<Event>,
        notifications: Sender<Notification>,
    ) -> Self {
        let reader = Arc::new(FileReader::new(Arc::clone(&download)));
        let writer = Arc::new(Mutex::new(FileWriter::new(
            Arc::clone(&download),
            events.clone(),
        )));
        let tracker = Tracker::spawn(Arc::clone(&download), events.clone());
        let connection_manager = ConnectionManager::new(download, events);
        Self {
            connection_manager,
            reader,
            writer,
            tracker,
            notifications,
        }
    }

    pub fn execute(&mut self, command: Command) -> ExecutionResult {
        match command {
            Command::EstablishConnection(addr, socket) => {
                self.connection_manager.start(addr, socket)
            }

            Command::Send(addr, message) => self.connection_manager.send(&addr, message),

            Command::Broadcast(message) => self.connection_manager.broadcast(message),

            Command::RemovePeer(addr) => self.connection_manager.remove(&addr),

            Command::Upload(addr, block) => {
                let reader = Arc::clone(&self.reader);
                let tx = self.connection_manager.peer_tx(&addr);
                tokio::spawn(async move { reader.read(block, tx).await });
            }

            Command::IntegrateBlock(block_data) => {
                let writer = Arc::clone(&self.writer);
                tokio::spawn(async move { writer.lock().await.write(block_data).await });
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
        }
        ExecutionResult::Continue
    }

    pub async fn shutdown(self) {
        if let Err(err) = self.tracker.shutdown().await {
            warn!("error encountered while shutting down tracker: {:?}", err);
        }
        self.connection_manager.shutdown().await;
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
