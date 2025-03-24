use std::ops::AddAssign;

use super::transfer_rate::TransferRate;

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub upload: TransferRate,
    pub download: TransferRate,
}

impl AddAssign for Stats {
    fn add_assign(&mut self, rhs: Self) {
        self.upload += rhs.upload;
        self.download += rhs.download;
    }
}
