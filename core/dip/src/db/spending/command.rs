//! Writes over `spending`.

use anyhow::{Context, Result};
use rusqlite::params;

use crate::db::Tx;
use crate::model::Charge;

/// Record one charge, and drop every row older than `cutoff` on the way past.
pub fn record(tx: &Tx<'_>, charge: &Charge, cutoff: i64) -> Result<()> {
    tx.prepare_cached("DELETE FROM spending WHERE spent_at < ?1")?
        .execute(params![cutoff])
        .context("pruning spent charges")?;

    tx.prepare_cached(
        "INSERT INTO spending (pubkey, meter, pooled, spent_at, bytes) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![
        charge.pubkey.to_hex(),
        charge.meter.as_str(),
        charge.pooled,
        charge.at,
        i64::try_from(charge.bytes).unwrap_or(i64::MAX),
    ])
    .context("recording a charge")?;

    Ok(())
}

/// Remove every charge. Part of [`wipe`](crate::db::command::wipe).
pub fn clear(tx: &Tx<'_>) -> Result<()> {
    tx.execute_batch("DELETE FROM spending;")
        .context("clearing the quota ledger")
}
