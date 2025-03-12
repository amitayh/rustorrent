use std::time::Duration;

pub struct Config {
    pub unchoking_interval: Duration,
    pub optimistic_unchoking_interval: Duration,
}

impl Config {
    pub fn with_unchoking_interval(mut self, interval: Duration) -> Self {
        self.unchoking_interval = interval;
        self
    }

    pub fn with_optimistic_unchoking_interval(mut self, interval: Duration) -> Self {
        self.optimistic_unchoking_interval = interval;
        self
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            unchoking_interval: Duration::from_secs(10),
            optimistic_unchoking_interval: Duration::from_secs(30),
        }
    }
}
