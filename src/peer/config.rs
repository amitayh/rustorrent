use std::{path::PathBuf, time::Duration};

use size::Size;

use crate::peer::PeerId;

#[derive(Clone, Debug)]
pub struct Config {
    pub client_id: PeerId,
    pub port: u16,
    pub download_path: PathBuf,
    pub sweep_interval: Duration,
    pub keep_alive_interval: Duration,
    pub choking_interval: Duration,
    pub optimistic_choking_cycle: usize,
    pub block_size: Size,
    pub connect_timeout: Duration,
    pub idle_peer_timeout: Duration,
    pub block_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub channel_buffer: usize,
}

#[allow(dead_code)]
impl Config {
    pub fn with_keep_alive_interval(mut self, interval: Duration) -> Self {
        self.keep_alive_interval = interval;
        self
    }

    pub fn with_unchoking_interval(mut self, interval: Duration) -> Self {
        self.choking_interval = interval;
        self
    }

    pub fn with_optimistic_unchoking_cycle(mut self, n: usize) -> Self {
        self.optimistic_choking_cycle = n;
        self
    }
}

impl Config {
    pub fn new(download_path: PathBuf) -> Self {
        Self {
            client_id: PeerId::random(),
            port: 6881,
            download_path,
            sweep_interval: Duration::from_secs(5),
            keep_alive_interval: Duration::from_secs(120),
            choking_interval: Duration::from_secs(10),
            optimistic_choking_cycle: 3,
            block_size: Size::from_kibibytes(16),
            connect_timeout: Duration::from_secs(5),
            idle_peer_timeout: Duration::from_secs(30),
            block_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_secs(10),
            channel_buffer: 16,
        }
    }
}
