use std::{net::SocketAddr, sync::Arc};

use tokio::{
    sync::{Mutex, Notify, mpsc::Sender},
    task::JoinHandle,
};

use crate::peer::message::Block;

use super::{Assignment, BlockAssignerConfig};

pub struct PeerHandle {
    join_handle: JoinHandle<()>,
    notify: Arc<Notify>,
}

impl PeerHandle {
    pub fn spawn(
        assignment: Arc<Mutex<Assignment>>,
        config: Arc<BlockAssignerConfig>,
        tx: Sender<(SocketAddr, Block)>,
        addr: SocketAddr,
    ) -> Self {
        let notify = Arc::new(Notify::new());
        let join_handle = {
            let notify = Arc::clone(&notify);
            tokio::spawn(async move {
                let mut timeout = None;
                loop {
                    if let Some(block) = assignment.lock().await.assign(&addr) {
                        tx.send((addr, block)).await.unwrap();
                        let assignment = Arc::clone(&assignment);
                        let config = Arc::clone(&config);
                        let notify = Arc::clone(&notify);
                        timeout = Some(tokio::spawn(async move {
                            tokio::time::sleep(config.block_timeout).await;
                            assignment.lock().await.release(&addr, block);
                            notify.notify_waiters();
                        }));
                    }
                    notify.notified().await;
                    if let Some(timeout) = timeout.take() {
                        timeout.abort();
                    }
                }
            })
        };
        Self {
            join_handle,
            notify,
        }
    }

    pub fn notify(&self) {
        self.notify.notify_waiters()
    }
}

impl Drop for PeerHandle {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
