use crate::peer::stats::GlobalStats;

#[derive(Debug)]
pub enum Notification {
    DownloadComplete,
    Stats(GlobalStats),
}
