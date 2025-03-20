use std::time::Duration;

use size::Size;

pub struct Config {
    pub keep_alive_interval: Duration,
    pub choking_interval: Duration,
    pub optimistic_choking_cycle: usize,
    pub block_size: Size,
    pub block_timeout: Duration,
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
            keep_alive_interval: Duration::from_secs(120),
            choking_interval: Duration::from_secs(10),
            optimistic_choking_cycle: 3,
            block_size: Size::from_kibibytes(16),
            block_timeout: Duration::from_secs(5),
        }
    }
}
