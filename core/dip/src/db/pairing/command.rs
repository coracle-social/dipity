//! Writes over `pair_secret` and `disclosure`.

use anyhow::{Context, Result};
use coracle_lib::keys::PublicKey;
use rusqlite::params;

use crate::db::Tx;

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

/// Record that this device disclosed its identity to `pubkey`. First-time only,
/// so the budget counts new pubkeys rather than every encounter.
pub fn record_disclosure(tx: &Tx<'_>, pubkey: &PublicKey, at: i64) -> Result<bool> {
    let written = tx
        .prepare_cached("INSERT OR IGNORE INTO disclosure (pubkey, disclosed_at) VALUES (?1, ?2)")?
        .execute(params![pubkey.to_hex(), at])
        .with_context(|| format!("recording a disclosure to {pubkey}"))?;

    Ok(written > 0)
}
