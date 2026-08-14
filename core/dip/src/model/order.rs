//! What a result set is ordered by.

/// What a result set is ordered by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Order {
    /// The author's claimed time, which is what NIP-01 pagination walks and
    /// what a filter's `limit` is measured against. The default, because a
    /// query with no local criteria is answering the wire.
    #[default]
    CreatedAt,
    /// The local arrival time, which is what a reader wants: a note handed over
    /// today is new to them whatever its author stamped it.
    SeenAt,
}
