//! Reads over `recipient_signature`.

use anyhow::{Context, Result};
use coracle_lib::events::EventId;
use coracle_lib::keys::PublicKey;
use rusqlite::{Row, params, params_from_iter};

use crate::db::Tx;
use crate::db::sql::{bytes_from_sql, event_id_from_sql, placeholders, pubkey_from_sql};
use crate::model::RecipientSignature;

/// The signature over an event naming a particular recipient.
pub fn get(
    tx: &Tx<'_>,
    event_id: &EventId,
    recipient_pubkey: &PublicKey,
) -> Result<Option<RecipientSignature>> {
    let signature = tx
        .prepare_cached(
            "SELECT event_id, author_pubkey, recipient_pubkey, sig FROM recipient_signature
             WHERE event_id = ?1 AND recipient_pubkey = ?2",
        )?
        .query_row(
            params![event_id.to_hex(), recipient_pubkey.to_hex()],
            to_signature,
        )
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .with_context(|| format!("loading the signature over {event_id} for {recipient_pubkey}"))?;

    Ok(signature)
}

/// Whether a signature over this event names this recipient.
///
/// The forwarding question: an event can only travel a second hop from a device
/// holding one of these.
pub fn exists(tx: &Tx<'_>, event_id: &EventId, recipient_pubkey: &PublicKey) -> Result<bool> {
    let exists = tx
        .prepare_cached(
            "SELECT EXISTS (
                 SELECT 1 FROM recipient_signature WHERE event_id = ?1 AND recipient_pubkey = ?2
             )",
        )?
        .query_row(
            params![event_id.to_hex(), recipient_pubkey.to_hex()],
            |row| row.get::<_, bool>(0),
        )
        .with_context(|| format!("checking for a signature over {event_id}"))?;

    Ok(exists)
}

/// Every signature stored over an event.
pub fn list_for_event(tx: &Tx<'_>, event_id: &EventId) -> Result<Vec<RecipientSignature>> {
    let mut prepared = tx.prepare_cached(
        "SELECT event_id, author_pubkey, recipient_pubkey, sig FROM recipient_signature
         WHERE event_id = ?1
         ORDER BY recipient_pubkey ASC",
    )?;

    let signatures = prepared
        .query_map(params![event_id.to_hex()], to_signature)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .with_context(|| format!("listing signatures over {event_id}"))?;

    Ok(signatures)
}

/// Which of `event_ids` this device holds a signature for, naming `recipient`.
///
/// One query rather than one per event, because the forwarding side asks this
/// of every event it is about to offer a peer.
pub fn forwardable(
    tx: &Tx<'_>,
    event_ids: &[EventId],
    recipient_pubkey: &PublicKey,
) -> Result<Vec<EventId>> {
    if event_ids.is_empty() {
        return Ok(Vec::new());
    }

    // Numbered from 2, since the recipient binds first.
    let placeholders = placeholders(2, event_ids.len());
    let mut prepared = tx.prepare(&format!(
        "SELECT event_id FROM recipient_signature
         WHERE recipient_pubkey = ?1 AND event_id IN ({placeholders})"
    ))?;

    let params =
        std::iter::once(recipient_pubkey.to_hex()).chain(event_ids.iter().map(EventId::to_hex));

    let ids = prepared
        .query_map(params_from_iter(params), |row| {
            event_id_from_sql(&row.get::<_, String>(0)?, 0)
        })?
        .collect::<rusqlite::Result<Vec<EventId>>>()
        .context("listing forwardable events")?;

    Ok(ids)
}

fn to_signature(row: &Row<'_>) -> rusqlite::Result<RecipientSignature> {
    Ok(RecipientSignature {
        event_id: event_id_from_sql(&row.get::<_, String>("event_id")?, 0)?,
        author_pubkey: pubkey_from_sql(&row.get::<_, String>("author_pubkey")?, 1)?,
        recipient_pubkey: pubkey_from_sql(&row.get::<_, String>("recipient_pubkey")?, 2)?,
        sig: bytes_from_sql(&row.get::<_, String>("sig")?, 3)?,
    })
}
