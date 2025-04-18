use std::{
    cmp::Ordering,
    fmt::Display,
    ops::{Add, AddAssign},
    time::Duration,
};

use size::Size;

#[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
pub struct TransferRate(pub Size, pub Duration);

impl Display for TransferRate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let TransferRate(size, duration) = self;
        let seconds = duration.as_secs_f64();
        let size_per_second = size / seconds;
        write!(f, "{}/s", size_per_second)
    }
}

impl TransferRate {
    pub const EMPTY: Self = Self(Size::from_const(0), Duration::ZERO);

    fn bps(&self) -> f64 {
        (self.0.bytes() as f64) / self.1.as_secs_f64()
    }
}

impl Add for TransferRate {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        let mut sum = self;
        sum += rhs;
        sum
    }
}

impl AddAssign for TransferRate {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
        self.1 += rhs.1;
    }
}

impl PartialOrd for TransferRate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TransferRate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.bps().total_cmp(&other.bps())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add() {
        let a = TransferRate(Size::from_kibibytes(10), Duration::from_secs(1));
        let b = TransferRate(Size::from_kibibytes(20), Duration::from_secs(1));

        assert_eq!(
            a + b,
            TransferRate(Size::from_kibibytes(30), Duration::from_secs(2))
        );
    }

    #[test]
    fn ordering() {
        let rate_10_kbps = TransferRate(Size::from_kibibytes(10), Duration::from_secs(1));
        let rate_20_kbps = TransferRate(Size::from_kibibytes(20), Duration::from_secs(1));
        let rate_30_kbps = TransferRate(Size::from_kibibytes(30), Duration::from_secs(1));
        let mut rates = vec![&rate_10_kbps, &rate_30_kbps, &rate_20_kbps];
        rates.sort();

        assert_eq!(rates, vec![&rate_10_kbps, &rate_20_kbps, &rate_30_kbps]);
    }
}
