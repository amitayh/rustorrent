use tokio::time::{Interval, interval};

use crate::client::Config;
use crate::event::Event;

pub struct Timers {
    keep_alive: Interval,
    choke: Interval,
    sweep: Interval,
    stats: Interval,
}

impl Timers {
    pub fn new(config: &Config) -> Self {
        Self {
            keep_alive: interval(config.keep_alive_interval),
            choke: interval(config.choking_interval),
            sweep: interval(config.sweep_interval),
            stats: interval(config.update_stats_interval),
        }
    }

    pub async fn tick(&mut self) -> Event {
        tokio::select! {
            _ = self.keep_alive.tick() => Event::KeepAliveTicked,
            _ = self.choke.tick() => Event::ChokeTicked,
            _ = self.stats.tick() => Event::StatsTicked,
            now = self.sweep.tick() => Event::SweepTicked(now),
        }
    }
}
