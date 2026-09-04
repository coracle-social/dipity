//! Reads over `pair_secret` and `disclosure`.

use anyhow::{Context, Result};
use coracle_lib::keys::PublicKey;
use rusqlite::params;

use crate::db::Tx;
use crate::db::sql::{bytes_from_sql, pubkey_from_sql};

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

/// How many times this device disclosed its identity at or after `cutoff`. The
/// disclosure budget asks this of the current window.
pub fn disclosures_since(tx: &Tx<'_>, cutoff: i64) -> Result<u32> {
    let count = tx
        .prepare_cached("SELECT COUNT(*) FROM disclosure WHERE disclosed_at >= ?1")?
        .query_row(params![cutoff], |row| row.get::<_, u32>(0))
        .context("counting disclosures")?;

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::Db;
    use crate::db::pairing::command;
    use crate::fixtures::author;
    use crate::model::DISCLOSURE_WINDOW_SECONDS;

    #[test]
    fn a_pair_secret_is_stored_once_per_pubkey() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();
        let secret = [1u8; 32];

        assert!(command::save_secret(&tx, &author(1), &secret, 100).unwrap());
        assert!(!command::save_secret(&tx, &author(1), &secret, 200).unwrap());

        assert_eq!(secrets(&tx).unwrap(), vec![(author(1), secret)]);
    }

    #[test]
    fn every_disclosure_in_the_window_counts() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();
        let base = DISCLOSURE_WINDOW_SECONDS;

        command::record_disclosure(&tx, base + 100).unwrap();
        command::record_disclosure(&tx, base + 200).unwrap();
        // A second disclosure to the same peer is a second AUTH event handed
        // over, and the ledger cannot tell them apart anyway.
        command::record_disclosure(&tx, base + 300).unwrap();

        assert_eq!(disclosures_since(&tx, 0).unwrap(), 3);
        assert_eq!(disclosures_since(&tx, base + 150).unwrap(), 2);
    }

    #[test]
    fn a_disclosure_older_than_the_window_is_pruned() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();
        let base = 2 * DISCLOSURE_WINDOW_SECONDS;

        command::record_disclosure(&tx, base - DISCLOSURE_WINDOW_SECONDS - 1).unwrap();
        assert_eq!(disclosures_since(&tx, 0).unwrap(), 1);

        command::record_disclosure(&tx, base).unwrap();
        assert_eq!(disclosures_since(&tx, 0).unwrap(), 1);
    }
}
