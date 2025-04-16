use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use log::warn;
use tokio::sync::Mutex;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinSet;

use crate::action::Action;
use crate::event::Event;
use crate::peer::Download;
use crate::peer::Notification;
use crate::peer::connection::Connection;
use crate::storage::FileReader;
use crate::storage::FileWriter;
use crate::tracker::Tracker;

pub struct ActionRunner {
    download: Arc<Download>,
    peers: HashMap<SocketAddr, Connection>,
    reader: Arc<FileReader>,
    writer: Arc<Mutex<FileWriter>>,
    tracker: Tracker,
    events: Sender<Event>,
    notifications: Sender<Notification>,
}

impl ActionRunner {
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

    pub fn run(&mut self, action: Action) -> anyhow::Result<bool> {
        match action {
            Action::EstablishConnection(addr, socket) if !self.peers.contains_key(&addr) => {
                let conn = Connection::spawn(
                    addr,
                    socket,
                    self.events.clone(),
                    Arc::clone(&self.download),
                );
                self.peers.insert(addr, conn);
            }

            Action::Send(addr, message) => {
                let peer = self.peers.get(&addr).expect("invalid peer");
                peer.send(message);
            }

            Action::Broadcast(message) => {
                for peer in self.peers.values() {
                    peer.send(message.clone());
                }
            }

            Action::Upload(addr, block) => {
                let peer = self.peers.get(&addr).expect("invalid peer");
                let reader = Arc::clone(&self.reader);
                let tx = peer.tx.clone();
                tokio::spawn(async move { reader.read(block, tx).await });
            }

            Action::IntegrateBlock(block_data) => {
                let writer = Arc::clone(&self.writer);
                tokio::spawn(async move { writer.lock().await.write(block_data).await });
            }

            Action::RemovePeer(addr) => {
                let peer = self.peers.remove(&addr).expect("invalid peer");
                peer.abort();
            }

            Action::UpdateStats(stats) => {
                self.tracker
                    .update_progress(stats.downloaded, stats.uploaded)?;
                let result = self.notifications.try_send(if stats.download_complete() {
                    Notification::DownloadComplete
                } else {
                    Notification::Stats(stats)
                });
                if result.is_err() {
                    warn!("failed sending notification");
                }
            }

            Action::Shutdown => {
                return Ok(false);
            }

            action => warn!("unhandled action: {:?}", action),
        }
        Ok(true)
    }

    pub async fn shutdown(mut self) -> Result<()> {
        let mut join_set = JoinSet::new();
        join_set.spawn(async move { self.tracker.shutdown().await });
        for (_, peer) in self.peers.drain() {
            join_set.spawn(async move { peer.shutdown().await });
        }
        while let Some(result) = join_set.join_next().await {
            if let Err(e) = result {
                warn!("error encountered while shutting down: {:?}", e);
            }
        }
        Ok(())
    }
}
