//! Writes over `spending`.

use anyhow::{Context, Result};
use coracle_lib::keys::PublicKey;
use rusqlite::params;

use crate::db::Tx;

/// Record one accepted event's bytes against the peer it came from.
pub fn record(tx: &Tx<'_>, pubkey: &PublicKey, at: i64, bytes: usize) -> Result<()> {
    tx.prepare_cached("INSERT INTO spending (pubkey, at, bytes) VALUES (?1, ?2, ?3)")?
        .execute(params![pubkey.to_hex(), at, bytes as i64])
        .with_context(|| format!("recording spending from {pubkey}"))?;

    Ok(())
}
