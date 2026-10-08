//! Reads over `blob` and `blob_reference`.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use rusqlite::{Row, params, params_from_iter};

use crate::db::Tx;
use coracle_lib::events::EventId;

use crate::db::sql::{event_id_from_sql, placeholders};
use crate::model::{Blob, BlobHash};

/// The blob columns, in the order [`to_blob`] reads them.
const COLUMNS: &str = "b.sha256, b.url, b.mime_type, b.size, b.dim, b.blurhash, b.alt, b.blake3,
     b.imeta, b.stored_bytes, b.complete";

/// One blob by hash.
pub fn get(tx: &Tx<'_>, sha256: &BlobHash) -> Result<Option<Blob>> {
    let blob = tx
        .prepare_cached(&format!("SELECT {COLUMNS} FROM blob b WHERE b.sha256 = ?1"))?
        .query_row(params![sha256], to_blob)
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .with_context(|| format!("loading blob {sha256}"))?;

    Ok(blob)
}

/// Whether every byte of a blob is held and hashes to its address.
pub fn is_complete(tx: &Tx<'_>, sha256: &BlobHash) -> Result<bool> {
    let complete = tx
        .prepare_cached("SELECT COALESCE((SELECT complete FROM blob WHERE sha256 = ?1), 0)")?
        .query_row(params![sha256], |row| row.get::<_, bool>(0))
        .with_context(|| format!("checking blob {sha256}"))?;

    Ok(complete)
}

/// Every blob a stored event references.
pub fn list_for_event(tx: &Tx<'_>, event_id: &EventId) -> Result<Vec<Blob>> {
    let mut prepared = tx.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM blob b
         JOIN blob_reference r ON r.sha256 = b.sha256
         WHERE r.event_id = ?1
         ORDER BY b.sha256 ASC"
    ))?;

    let blobs = prepared
        .query_map(params![event_id.to_hex()], to_blob)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .with_context(|| format!("listing blobs for {event_id}"))?;

    Ok(blobs)
}

/// Every event that references a hash, which is who the blob is kept for and
/// whose permissions it carries.
pub fn events_referencing(tx: &Tx<'_>, sha256: &BlobHash) -> Result<Vec<EventId>> {
    let mut prepared = tx.prepare_cached(
        "SELECT event_id FROM blob_reference WHERE sha256 = ?1 ORDER BY event_id ASC",
    )?;

    let events = prepared
        .query_map(params![sha256], |row| {
            event_id_from_sql(&row.get::<_, String>("event_id")?, 0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .with_context(|| format!("listing the events referencing blob {sha256}"))?;

    Ok(events)
}

/// Every blob referenced by any of `event_ids`, grouped by the event.
///
/// One query rather than one per event, because the view asks this of a whole
/// page at a time. An event with no blobs is absent from the map rather than
/// present and empty.
pub fn list_for_events(tx: &Tx<'_>, event_ids: &[EventId]) -> Result<HashMap<EventId, Vec<Blob>>> {
    if event_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders = placeholders(1, event_ids.len());
    let mut prepared = tx.prepare(&format!(
        "SELECT r.event_id, {COLUMNS} FROM blob b
         JOIN blob_reference r ON r.sha256 = b.sha256
         WHERE r.event_id IN ({placeholders})
         ORDER BY b.sha256 ASC"
    ))?;

    // Ordered by hash, so that each event's blobs arrive in `list_for_event` order.
    let mut blobs: HashMap<EventId, Vec<Blob>> = HashMap::new();

    for (event_id, blob) in prepared
        .query_map(
            params_from_iter(event_ids.iter().map(EventId::to_hex)),
            |row| {
                Ok((
                    event_id_from_sql(&row.get::<_, String>("event_id")?, 0)?,
                    to_blob(row)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("listing blobs for a page of events")?
    {
        blobs.entry(event_id).or_default().push(blob);
    }

    Ok(blobs)
}

/// Every hash the table has a record for.
///
/// What a blob store sweep is settled against: bytes held for a hash that is
/// not here are referenced by nothing and are the sweep's to delete.
pub fn all_hashes(tx: &Tx<'_>) -> Result<HashSet<BlobHash>> {
    let mut prepared = tx.prepare_cached("SELECT sha256 FROM blob")?;

    let hashes = prepared
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()
        .context("listing every recorded blob hash")?;

    Ok(hashes)
}

/// The want list: blobs a stored event references and this device does not
/// hold, the most nearly complete first.
///
/// There is no per-blob decision to make — the anchoring event already passed
/// the accept policy, and its blobs are in scope for the same reason its text
/// is. See `docs/sync.md`.
pub fn wanted(tx: &Tx<'_>, limit: usize) -> Result<Vec<Blob>> {
    let mut prepared = tx.prepare_cached(&wanted_sql())?;

    let blobs = prepared
        .query_map(params![i64::try_from(limit).unwrap_or(i64::MAX)], to_blob)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("listing wanted blobs")?;

    Ok(blobs)
}

/// The want list read: whichever blob has the most bytes already on disk first.
fn wanted_sql() -> String {
    format!(
        "SELECT {COLUMNS} FROM blob b
         WHERE b.complete = 0
         ORDER BY b.stored_bytes DESC, b.sha256 ASC
         LIMIT ?1"
    )
}

fn to_blob(row: &Row<'_>) -> rusqlite::Result<Blob> {
    let imeta: String = row.get("imeta")?;

    Ok(Blob {
        sha256: row.get("sha256")?,
        url: row.get("url")?,
        mime_type: row.get("mime_type")?,
        size: row.get("size")?,
        dim: row.get("dim")?,
        blurhash: row.get("blurhash")?,
        alt: row.get("alt")?,
        blake3: row.get("blake3")?,
        imeta: serde_json::from_str(&imeta).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                8,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        stored_bytes: row.get("stored_bytes")?,
        complete: row.get("complete")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::Db;

    /// What the planner does with a statement, one line per step.
    fn plan(tx: &Tx<'_>, sql: &str, params: impl rusqlite::Params) -> String {
        tx.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map(params, |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join("\n")
    }

    /// The want list takes ten rows off the front of an order the index has to supply.
    #[test]
    fn the_want_list_index_is_walked_in_the_order_its_read_asks_for() {
        let mut db = Db::open_in_memory().unwrap();
        let tx = db.begin_write().unwrap();
        let plan = plan(&tx, &wanted_sql(), params![10]);

        assert!(plan.contains("blob_wanted"), "blob_wanted unused:\n{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "blob_wanted sorts:\n{plan}");
    }
}
