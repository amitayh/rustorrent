use std::time::Duration;

pub struct Config {
    pub choking_interval: Duration,
    pub optimistic_choking_cycle: usize,
}

impl Config {
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
            choking_interval: Duration::from_secs(10),
            optimistic_choking_cycle: 3,
        }
    }
}
