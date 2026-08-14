//! Mapping between nostr's types and SQLite's.
//!
//! One mismatch, handled here rather than at every call site: ids, pubkeys and
//! signatures are fixed-size byte arrays in memory and lowercase hex in the
//! columns. Hex is what the wire, the logs and the test fixtures use, and the
//! store is a phone's.
//!
//! Time needs nothing. `coracle-lib` counts seconds in `i64`, which is SQLite's
//! integer, so a timestamp binds and reads as itself.

use coracle_lib::keys::PublicKey;
use rusqlite::Error::FromSqlConversionFailure;
use rusqlite::types::Type;

/// `?1, ?2, …` for `count` parameters, the first of them numbered `from`.
///
/// For an `IN` over a set whose size is only known at runtime, which is what a
/// read batched over a page of events is. Empty in, empty out — and `IN ()` is
/// not valid SQL, so a caller that can be handed an empty set answers for it
/// before building a statement.
pub(crate) fn placeholders(from: usize, count: usize) -> String {
    (from..from + count)
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A fixed-size byte array — an id or a signature — coming back out of the
/// `column`th column.
pub(crate) fn bytes_from_sql<const N: usize>(
    hex: &str,
    column: usize,
) -> rusqlite::Result<[u8; N]> {
    let decoded = hex::decode(hex).map_err(|error| conversion_failed(column, error))?;

    decoded.try_into().map_err(|_| {
        conversion_failed(column, format!("expected {N} bytes, got {}", hex.len() / 2))
    })
}

/// A pubkey coming back out of the `column`th column. Fails on anything that is
/// not a point on the curve, which the column's `TEXT` type cannot enforce.
pub(crate) fn pubkey_from_sql(hex: &str, column: usize) -> rusqlite::Result<PublicKey> {
    PublicKey::from_hex(hex).map_err(|error| conversion_failed(column, error))
}

fn conversion_failed(
    column: usize,
    error: impl Into<Box<dyn std::error::Error + Send + Sync>>,
) -> rusqlite::Error {
    FromSqlConversionFailure(column, Type::Text, error.into())
}
