//! Reads over `spending`.

use anyhow::{Context, Result};
use rusqlite::params;

use crate::db::Tx;
use crate::db::sql::pubkey_from_sql;
use crate::model::{Charge, Meter};

/// Every charge at or after `cutoff`, oldest first.
pub fn since(tx: &Tx<'_>, cutoff: i64) -> Result<Vec<Charge>> {
    let mut prepared = tx.prepare_cached(
        "SELECT pubkey, meter, pooled, spent_at, bytes FROM spending
         WHERE spent_at >= ?1 ORDER BY spent_at ASC",
    )?;

    let charges = prepared
        .query_map(params![cutoff], |row| {
            Ok(Charge {
                pubkey: pubkey_from_sql(&row.get::<_, String>(0)?, 0)?,
                meter: if row.get::<_, String>(1)? == Meter::Blob.as_str() {
                    Meter::Blob
                } else {
                    Meter::Event
                },
                pooled: row.get(2)?,
                at: row.get(3)?,
                bytes: u64::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("reading the quota ledger")?;

    Ok(charges)
}
