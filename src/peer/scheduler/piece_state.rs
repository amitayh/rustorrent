#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PieceState {
    Orphan,
    Available,
    Active,
}
