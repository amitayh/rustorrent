use std::{path::PathBuf, time::Duration};

use size::Size;

use crate::core::PeerId;

/// Configuration settings for the peer
#[derive(Clone, Debug)]
pub struct Config {
    // Identity and Network Settings
    /// Unique identifier for this peer in the swarm
    pub client_id: PeerId,
    /// Path where downloaded files will be stored
    pub download_path: PathBuf,
    /// Port number to listen for incoming connections
    pub port: u16,

    // Scheduler Settings
    /// Maximum number of concurrent block requests per peer
    pub max_concurrent_requests_per_peer: usize,
    /// Size of data blocks for piece transfers
    pub block_size: Size,

    // Choker Settings
    /// Interval between choking algorithm runs
    pub choking_interval: Duration,
    /// Number of choking cycles between optimistic unchoking attempts
    pub optimistic_choking_cycle: usize,

    // Sweeper Settings
    /// Interval between sweeps for idle peers and abandoned blocks
    pub sweep_interval: Duration,
    /// Time after which an idle peer is disconnected
    pub idle_peer_timeout: Duration,
    /// Time after which a block request is considered abandoned
    pub block_timeout: Duration,

    // Event System Settings
    /// Interval between keep-alive messages
    pub keep_alive_interval: Duration,
    /// Interval between statistics updates
    pub update_stats_interval: Duration,
    /// Size of the event queue buffer
    pub events_buffer: usize,
    /// Size of the channel buffer for peer communication
    pub channel_buffer: usize,

    // Connection Settings
    /// Timeout for establishing new connections
    pub connect_timeout: Duration,
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

    pub fn with_block_size(mut self, size: Size) -> Self {
        self.block_size = size;
        self
    }

    pub fn with_max_concurrent_requests_per_peer(mut self, n: usize) -> Self {
        self.max_concurrent_requests_per_peer = n;
        self
    }
}

impl Config {
    pub fn new(download_path: PathBuf) -> Self {
        let keep_alive_interval = Duration::from_secs(120);
        Self {
            client_id: PeerId::random(),
            port: 6881,
            download_path,
            sweep_interval: Duration::from_secs(5),
            keep_alive_interval,
            choking_interval: Duration::from_secs(10),
            update_stats_interval: Duration::from_secs(1),
            optimistic_choking_cycle: 3,
            block_size: Size::from_kibibytes(16),
            connect_timeout: Duration::from_secs(10),
            idle_peer_timeout: keep_alive_interval * 2,
            block_timeout: Duration::from_secs(30),
            events_buffer: 128,
            channel_buffer: 16,
            max_concurrent_requests_per_peer: 10,
        }
    }
}
