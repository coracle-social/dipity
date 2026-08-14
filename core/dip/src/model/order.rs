//! What a result set is ordered by.

/// What a result set is ordered by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Order {
    /// The author's claimed time.
    #[default]
    CreatedAt,
    /// The local arrival time.
    SeenAt,
}
