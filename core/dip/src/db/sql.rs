//! Mapping between nostr's types and SQLite's.
//!
//! One mismatch, handled here rather than at every call site: ids, pubkeys and
//! signatures are fixed-size byte arrays in memory and lowercase hex in the
//! columns. Hex is what the wire, the logs and the test fixtures use, and the
//! store is a phone's.
//!
//! Time needs nothing. A timestamp binds and reads as itself because
//! `coracle-lib` counts seconds in `i64`, which is SQLite's integer.

use coracle_lib::events::EventId;
use coracle_lib::keys::PublicKey;
use rusqlite::Error::FromSqlConversionFailure;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, Type, ValueRef};

use crate::model::BlobHash;

/// A blob hash binds and reads as the hex the column holds.
///
/// An uppercase or truncated hash is not another spelling of a row, because
/// SQLite compares `TEXT` byte for byte. It is a row that is not there — a read
/// misses, a write fails its foreign key, and neither says why. Which is why
/// the column takes [`BlobHash`] and nothing else: the value is canonical
/// before it reaches a statement, and a legacy row that is not fails its read
/// rather than spreading.
///
/// Event ids used to need the same treatment. They are
/// [`EventId`](coracle_lib::events::EventId) now, which cannot hold a
/// non-canonical value in the first place.
impl ToSql for BlobHash {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(ValueRef::Text(
            self.as_str().as_bytes(),
        )))
    }
}

impl FromSql for BlobHash {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Self::parse(value.as_str()?).map_err(|error| FromSqlError::Other(error.into()))
    }
}

/// An event id coming back out of the `column`th column.
pub(crate) fn event_id_from_sql(hex: &str, column: usize) -> rusqlite::Result<EventId> {
    EventId::from_hex(hex).map_err(|error| conversion_failed(column, error))
}

/// `?1, ?2, …` for `count` parameters, the first of them numbered `from`.
///
/// For an `IN` over a set whose size is only known at runtime, which is what a
/// read batched over a page of events is. Empty in, empty out. A caller that can
/// be handed an empty set answers for it before building a statement, because
/// `IN ()` is not valid SQL.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blob_hash_round_trips_through_a_column() {
        let hash = BlobHash::parse(&"ab".repeat(32)).unwrap();
        let db = rusqlite::Connection::open_in_memory().unwrap();

        let read: BlobHash = db
            .query_row("SELECT ?1", rusqlite::params![hash], |row| row.get(0))
            .unwrap();

        assert_eq!(read, hash);
    }

    #[test]
    fn a_column_that_is_not_a_hash_fails_its_read() {
        // A row that names no file fails here rather than downstream.
        let db = rusqlite::Connection::open_in_memory().unwrap();

        assert!(
            db.query_row("SELECT 'AB'", [], |row| row.get::<_, BlobHash>(0))
                .is_err()
        );
    }

    #[test]
    fn placeholders_number_from_where_they_are_told() {
        assert_eq!(placeholders(1, 3), "?1, ?2, ?3");
        assert_eq!(placeholders(2, 2), "?2, ?3");
        assert_eq!(placeholders(1, 0), "");
    }
}
