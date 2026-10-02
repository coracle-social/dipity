//! Reads over `pair_secret` and `disclosure_bucket`.

use anyhow::{Context, Result};
use coracle_lib::keys::PublicKey;
use rusqlite::OptionalExtension;

use crate::db::Tx;
use crate::db::sql::{bytes_from_sql, pubkey_from_sql};
use crate::model::DisclosureBucket;

/// Every stored pair secret, as `(pubkey, secret)` pairs.
pub fn secrets(tx: &Tx<'_>) -> Result<Vec<(PublicKey, [u8; 32])>> {
    let mut prepared =
        tx.prepare_cached("SELECT pubkey, secret FROM pair_secret ORDER BY pubkey ASC")?;

    let secrets = prepared
        .query_map([], |row| {
            Ok((
                pubkey_from_sql(&row.get::<_, String>(0)?, 0)?,
                bytes_from_sql::<32>(&row.get::<_, String>(1)?, 1)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("listing pair secrets")?;

    Ok(secrets)
}

/// The disclosure bucket as last written, or full at `now` if nothing has been spent.
pub fn bucket(tx: &Tx<'_>, now: i64) -> Result<DisclosureBucket> {
    let stored = tx
        .prepare_cached("SELECT tokens, updated_at FROM disclosure_bucket WHERE id = 1")?
        .query_row([], |row| {
            Ok(DisclosureBucket {
                tokens: row.get(0)?,
                at: row.get(1)?,
            })
        })
        .optional()
        .context("reading the disclosure bucket")?;

    Ok(stored.unwrap_or_else(|| DisclosureBucket::full(now)))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::Db;
    use crate::db::pairing::command;
    use crate::fixtures::author;

    #[test]
    fn a_pubkey_holds_one_pair_secret_and_a_new_one_replaces_it() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        command::save_secret(&tx, &author(1), &[1u8; 32], 100).unwrap();
        command::save_secret(&tx, &author(1), &[2u8; 32], 200).unwrap();

        assert_eq!(secrets(&tx).unwrap(), vec![(author(1), [2u8; 32])]);
    }

    #[test]
    fn the_bucket_is_full_until_something_is_spent_and_then_holds_what_was_written() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        assert_eq!(bucket(&tx, 100).unwrap(), DisclosureBucket::full(100));

        let spent = DisclosureBucket::full(100).spend(12, 100);
        command::save_bucket(&tx, spent).unwrap();

        assert_eq!(bucket(&tx, 200).unwrap(), spent);
    }
}
