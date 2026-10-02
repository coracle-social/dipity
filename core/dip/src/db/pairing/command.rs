//! Writes over `pair_secret` and `disclosure_bucket`.

use anyhow::{Context, Result};
use coracle_lib::keys::PublicKey;
use rusqlite::params;

use crate::db::Tx;
use crate::model::DisclosureBucket;

/// Store a pair secret for a pubkey, replacing whatever was held for it.
///
/// The session calls this only for a peer it did not recognize, so what it
/// replaces is a secret the two devices no longer share.
pub fn save_secret(tx: &Tx<'_>, pubkey: &PublicKey, secret: &[u8; 32], at: i64) -> Result<()> {
    tx.prepare_cached(
        "INSERT INTO pair_secret (pubkey, secret, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (pubkey) DO UPDATE SET secret = excluded.secret, updated_at = excluded.updated_at",
    )?
    .execute(params![pubkey.to_hex(), hex::encode(secret), at])
    .with_context(|| format!("storing a pair secret for {pubkey}"))?;

    Ok(())
}

/// Write the disclosure bucket's level after a disclosure.
pub fn save_bucket(tx: &Tx<'_>, bucket: DisclosureBucket) -> Result<()> {
    tx.prepare_cached(
        "INSERT INTO disclosure_bucket (id, tokens, updated_at) VALUES (1, ?1, ?2)
         ON CONFLICT (id) DO UPDATE SET tokens = excluded.tokens, updated_at = excluded.updated_at",
    )?
    .execute(params![bucket.tokens, bucket.at])
    .context("writing the disclosure bucket")?;

    Ok(())
}

/// Remove every pair secret and the disclosure bucket. Part of
/// [`wipe`](crate::db::command::wipe).
pub fn clear(tx: &Tx<'_>) -> Result<()> {
    tx.execute_batch("DELETE FROM pair_secret; DELETE FROM disclosure_bucket;")
        .context("clearing the pairing tables")
}
