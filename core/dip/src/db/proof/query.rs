//! Reads over `proof`.

use anyhow::{Context, Result};
use coracle_lib::keys::PublicKey;
use rusqlite::{Row, params, params_from_iter};

use crate::db::Tx;
use crate::db::sql::{bytes_from_sql, pubkey_from_sql};
use crate::model::Proof;

/// The signature over an event naming a particular recipient.
pub fn get(tx: &Tx<'_>, event_id: &str, recipient_pubkey: &PublicKey) -> Result<Option<Proof>> {
    let proof = tx
        .prepare_cached(
            "SELECT event_id, recipient_pubkey, sig FROM proof
             WHERE event_id = ?1 AND recipient_pubkey = ?2",
        )?
        .query_row(params![event_id, recipient_pubkey.to_hex()], to_proof)
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .with_context(|| format!("loading the signature over {event_id} for {recipient_pubkey}"))?;

    Ok(proof)
}

/// Whether a signature over this event names this recipient.
///
/// The forwarding question: an event can only travel a second hop from a device
/// holding one of these.
pub fn exists(tx: &Tx<'_>, event_id: &str, recipient_pubkey: &PublicKey) -> Result<bool> {
    let exists = tx
        .prepare_cached(
            "SELECT EXISTS (
                 SELECT 1 FROM proof WHERE event_id = ?1 AND recipient_pubkey = ?2
             )",
        )?
        .query_row(params![event_id, recipient_pubkey.to_hex()], |row| {
            row.get::<_, bool>(0)
        })
        .with_context(|| format!("checking for a signature over {event_id}"))?;

    Ok(exists)
}

/// Every signature stored over an event.
pub fn list_for_event(tx: &Tx<'_>, event_id: &str) -> Result<Vec<Proof>> {
    let mut prepared = tx.prepare_cached(
        "SELECT event_id, recipient_pubkey, sig FROM proof
         WHERE event_id = ?1
         ORDER BY recipient_pubkey ASC",
    )?;

    let proofs = prepared
        .query_map(params![event_id], to_proof)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .with_context(|| format!("listing signatures over {event_id}"))?;

    Ok(proofs)
}

/// Which of `event_ids` this device holds a signature for, naming `recipient`.
///
/// One query rather than one per event, because the forwarding side asks this
/// of every event it is about to offer a peer.
pub fn forwardable(
    tx: &Tx<'_>,
    event_ids: &[String],
    recipient_pubkey: &PublicKey,
) -> Result<Vec<String>> {
    if event_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = (2..2 + event_ids.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");

    let mut prepared = tx.prepare(&format!(
        "SELECT event_id FROM proof
         WHERE recipient_pubkey = ?1 AND event_id IN ({placeholders})"
    ))?;

    let params = std::iter::once(recipient_pubkey.to_hex()).chain(event_ids.iter().cloned());

    let ids = prepared
        .query_map(params_from_iter(params), |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()
        .context("listing forwardable events")?;

    Ok(ids)
}

fn to_proof(row: &Row<'_>) -> rusqlite::Result<Proof> {
    Ok(Proof {
        event_id: row.get("event_id")?,
        recipient_pubkey: pubkey_from_sql(&row.get::<_, String>("recipient_pubkey")?, 1)?,
        sig: bytes_from_sql(&row.get::<_, String>("sig")?, 2)?,
    })
}
