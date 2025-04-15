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

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct GlobalStats {
    pub total_pieces: usize,
    pub completed_pieces: usize,
    pub connected_peers: usize,
    pub uploaded: usize,
    pub downloaded: usize,
    pub upload_rate: TransferRate,
    pub download_rate: TransferRate,
}

impl GlobalStats {
    pub fn new(total_pieces: usize, completed_pieces: usize) -> Self {
        Self {
            total_pieces,
            completed_pieces,
            connected_peers: 0,
            uploaded: 0,
            downloaded: 0,
            upload_rate: TransferRate::EMPTY,
            download_rate: TransferRate::EMPTY,
        }
    }

    pub fn download_complete(&self) -> bool {
        self.completed_pieces == self.total_pieces
    }

    pub fn completed_percentage(&self) -> f64 {
        100f64 * (self.completed_pieces as f64) / (self.total_pieces as f64)
    }
}
