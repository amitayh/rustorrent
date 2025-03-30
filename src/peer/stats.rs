use std::ops::AddAssign;

use super::transfer_rate::TransferRate;

#[derive(Debug, Clone, Default)]
pub struct PeerStats {
    pub upload: TransferRate,
    pub download: TransferRate,
}

impl AddAssign for PeerStats {
    fn add_assign(&mut self, rhs: Self) {
        self.upload += rhs.upload;
        self.download += rhs.download;
    }
}

#[derive(Debug, Clone)]
pub struct GlobalStats {
    pub total_pieces: usize,
    pub completed_pieces: usize,
    pub connected_peers: usize,
    pub upload_rate: TransferRate,
    pub download_rate: TransferRate,
}

impl GlobalStats {
    pub fn new(total_pieces: usize) -> Self {
        Self {
            total_pieces,
            completed_pieces: 0,
            connected_peers: 0,
            upload_rate: TransferRate::EMPTY,
            download_rate: TransferRate::EMPTY,
        }
    }

    pub fn completed(&self) -> f64 {
        100f64 * (self.completed_pieces as f64) / (self.total_pieces as f64)
    }
}
