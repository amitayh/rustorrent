use std::time::Duration;

use size::Size;

use crate::peer::PeerId;

#[derive(Clone)]
pub struct Config {
    pub clinet_id: PeerId,
    pub keep_alive_interval: Duration,
    pub choking_interval: Duration,
    pub sweep_interval: Duration,
    pub optimistic_choking_cycle: usize,
    pub block_size: Size,
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

impl Default for Config {
    fn default() -> Self {
        Self {
            clinet_id: PeerId::random(),
            keep_alive_interval: Duration::from_secs(120),
            choking_interval: Duration::from_secs(10),
            sweep_interval: Duration::from_secs(5),
            optimistic_choking_cycle: 3,
            block_size: Size::from_kibibytes(16),
            idle_peer_timeout: Duration::from_secs(30),
            block_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_secs(10),
            channel_buffer: 16,
        }
    }
}
