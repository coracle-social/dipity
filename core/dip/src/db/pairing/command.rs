//! Writes over `pair_secret` and `disclosure`.

use anyhow::{Context, Result};
use coracle_lib::keys::PublicKey;
use rusqlite::params;

use crate::db::Tx;
use crate::model::DISCLOSURE_WINDOW_SECONDS;

/// Store a pair secret for a pubkey. The first pairing wins: a later encounter
/// derives the same secret, and overwriting it would only risk desyncing the
/// two devices if one side ever missed a session.
pub fn save_secret(tx: &Tx<'_>, pubkey: &PublicKey, secret: &[u8; 32], at: i64) -> Result<bool> {
    let written = tx
        .prepare_cached(
            "INSERT OR IGNORE INTO pair_secret (pubkey, secret, updated_at) VALUES (?1, ?2, ?3)",
        )?
        .execute(params![pubkey.to_hex(), hex::encode(secret), at])
        .with_context(|| format!("storing a pair secret for {pubkey}"))?;

    Ok(written > 0)
}

/// Record that this device disclosed its identity, naming no recipient: the
/// dialer discloses before the peer has named itself, so the budget counts the
/// act rather than who received it.
///
/// Rows outside the current window are dropped on the way past. Nothing reads
/// them, and a standing count of how many strangers the user has met is the
/// kind of thing `docs/privacy.md` keeps off the device.
pub fn record_disclosure(tx: &Tx<'_>, at: i64) -> Result<()> {
    tx.prepare_cached("DELETE FROM disclosure WHERE disclosed_at < ?1")?
        .execute(params![at - DISCLOSURE_WINDOW_SECONDS])
        .context("pruning spent disclosures")?;

    tx.prepare_cached("INSERT INTO disclosure (disclosed_at) VALUES (?1)")?
        .execute(params![at])
        .context("recording a disclosure")?;

    Ok(())
}
