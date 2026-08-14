//! Mapping between nostr's types and SQLite's.
//!
//! One mismatch, handled here rather than at every call site: ids, pubkeys and
//! signatures are fixed-size byte arrays in memory and lowercase hex in the
//! columns. Hex is what the wire, the logs and the test fixtures use, and the
//! store is a phone's.
//!
//! Time needs nothing. `coracle-lib` counts seconds in `i64`, which is SQLite's
//! integer, so a timestamp binds and reads as itself.

use anyhow::{Result, bail};
use coracle_lib::events::EventId;
use coracle_lib::keys::PublicKey;
use rusqlite::Error::FromSqlConversionFailure;
use rusqlite::types::Type;

/// A blob hash in the form the columns hold it: lowercase, and checked.
///
/// SQLite compares `TEXT` byte for byte, so an uppercase hash is not another
/// spelling of a row, it is a row that is not there — a read misses, a write
/// fails its foreign key, and neither says why.
///
/// Event ids used to need this too. They are
/// [`EventId`](coracle_lib::events::EventId) now, which cannot hold a
/// non-canonical value in the first place, so this is left covering blob
/// hashes — sha256 of a file, not an event, and no business of `coracle-lib`'s.
///
/// Anything that is not 64 hex characters is an error rather than a miss: a
/// caller asking after a malformed hash has a bug, and answering "not here"
/// hides it.
pub(crate) fn hex_key(key: &str) -> Result<String> {
    if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{key} is not 32 bytes of hex");
    }

    Ok(key.to_ascii_lowercase())
}

/// An event id coming back out of the `column`th column.
pub(crate) fn event_id_from_sql(hex: &str, column: usize) -> rusqlite::Result<EventId> {
    EventId::from_hex(hex).map_err(|error| conversion_failed(column, error))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_comes_back_lowercase() {
        let lower = "ab".repeat(32);
        let upper = lower.to_ascii_uppercase();

        assert_eq!(hex_key(&lower).unwrap(), lower);
        assert_eq!(hex_key(&upper).unwrap(), lower);
        assert_eq!(hex_key("aB".repeat(32).as_str()).unwrap(), lower);
    }

    #[test]
    fn a_key_that_is_not_32_bytes_of_hex_is_an_error() {
        // Not a miss. A caller naming a row that cannot exist has a bug, and
        // `Ok(None)` is how it would go unnoticed.
        assert!(hex_key("").is_err());
        assert!(hex_key(&"ab".repeat(31)).is_err());
        assert!(hex_key(&"ab".repeat(33)).is_err());
        assert!(hex_key(&format!("{}zz", "ab".repeat(31))).is_err());
        assert!(hex_key(&"é".repeat(64)).is_err());
    }

    #[test]
    fn placeholders_number_from_where_they_are_told() {
        assert_eq!(placeholders(1, 3), "?1, ?2, ?3");
        assert_eq!(placeholders(2, 2), "?2, ?3");
        assert_eq!(placeholders(1, 0), "");
    }
}
