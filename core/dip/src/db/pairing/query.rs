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

/// How many distinct pubkeys this device first disclosed to at or after
/// `cutoff`. The disclosure budget asks this of the current window.
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
    fn disclosures_count_distinct_pubkeys_in_the_window() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();

        command::record_disclosure(&tx, &author(1), 100).unwrap();
        command::record_disclosure(&tx, &author(2), 200).unwrap();
        // A repeat is not a new pubkey.
        command::record_disclosure(&tx, &author(1), 300).unwrap();

        assert_eq!(disclosures_since(&tx, 0).unwrap(), 2);
        assert_eq!(disclosures_since(&tx, 150).unwrap(), 1);
    }
}
