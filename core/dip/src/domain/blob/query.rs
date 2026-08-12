//! Reads over `blob`.

use anyhow::{Context, Result};
use rusqlite::{Row, params};

use crate::db::Tx;

use super::model::{Blob, BlobRole};

/// The blob columns, in the order [`to_blob`] reads them.
const COLUMNS: &str = "sha256, event_id, role, url, mime_type, size, dim, blurhash, alt, blake3,
     imeta, stored_bytes, chunks, complete, accessed_at";

/// One blob by hash.
pub fn get(tx: &Tx<'_>, sha256: &str) -> Result<Option<Blob>> {
    let blob = tx
        .prepare_cached(&format!("SELECT {COLUMNS} FROM blob WHERE sha256 = ?1"))?
        .query_row(params![sha256], to_blob)
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .with_context(|| format!("loading blob {sha256}"))?;

    Ok(blob)
}

/// Whether every byte of a blob is held and verified.
pub fn is_complete(tx: &Tx<'_>, sha256: &str) -> Result<bool> {
    let complete = tx
        .prepare_cached("SELECT COALESCE((SELECT complete FROM blob WHERE sha256 = ?1), 0)")?
        .query_row(params![sha256], |row| row.get::<_, bool>(0))
        .with_context(|| format!("checking blob {sha256}"))?;

    Ok(complete)
}

/// Every blob a stored event references.
pub fn list_for_event(tx: &Tx<'_>, event_id: &str) -> Result<Vec<Blob>> {
    let mut prepared = tx.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM blob WHERE event_id = ?1 ORDER BY sha256 ASC"
    ))?;

    let blobs = prepared
        .query_map(params![event_id], to_blob)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .with_context(|| format!("listing blobs for {event_id}"))?;

    Ok(blobs)
}

/// The want list: blobs a stored event references and this device does not
/// hold, previews first.
///
/// There is no per-blob decision to make — the anchoring event already passed
/// the accept policy, so its blobs are in scope for the same reason its text
/// is. See `docs/sync.md`.
pub fn wanted(tx: &Tx<'_>, limit: usize) -> Result<Vec<Blob>> {
    let mut prepared = tx.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM blob
         WHERE complete = 0
         ORDER BY CASE role WHEN 'preview' THEN 0 ELSE 1 END, stored_bytes DESC, sha256 ASC
         LIMIT ?1"
    ))?;

    let blobs = prepared
        .query_map(params![i64::try_from(limit).unwrap_or(i64::MAX)], to_blob)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("listing wanted blobs")?;

    Ok(blobs)
}

/// How many bytes of held blobs a role accounts for, which is what the cache
/// ceiling is measured against.
pub fn stored_bytes(tx: &Tx<'_>, role: BlobRole) -> Result<i64> {
    let bytes = tx
        .prepare_cached("SELECT COALESCE(SUM(stored_bytes), 0) FROM blob WHERE role = ?1")?
        .query_row(params![role.as_str()], |row| row.get(0))
        .context("summing stored blob bytes")?;

    Ok(bytes)
}

/// Held blobs of a role, least recently read first — eviction order.
///
/// Only originals are evicted; previews are kept as long as the events that
/// reference them, which is what makes a feed still render offline.
pub fn least_recently_used(tx: &Tx<'_>, role: BlobRole, limit: usize) -> Result<Vec<Blob>> {
    let mut prepared = tx.prepare_cached(&format!(
        "SELECT {COLUMNS} FROM blob
         WHERE role = ?1 AND complete = 1
         ORDER BY COALESCE(accessed_at, 0) ASC, sha256 ASC
         LIMIT ?2"
    ))?;

    let blobs = prepared
        .query_map(
            params![role.as_str(), i64::try_from(limit).unwrap_or(i64::MAX)],
            to_blob,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("listing eviction candidates")?;

    Ok(blobs)
}

fn to_blob(row: &Row<'_>) -> rusqlite::Result<Blob> {
    let role: String = row.get("role")?;
    let imeta: String = row.get("imeta")?;

    Ok(Blob {
        sha256: row.get("sha256")?,
        event_id: row.get("event_id")?,
        role: BlobRole::parse(&role).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                format!("unknown blob role {role}").into(),
            )
        })?,
        url: row.get("url")?,
        mime_type: row.get("mime_type")?,
        size: row.get("size")?,
        dim: row.get("dim")?,
        blurhash: row.get("blurhash")?,
        alt: row.get("alt")?,
        blake3: row.get("blake3")?,
        imeta: serde_json::from_str(&imeta).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                10,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        stored_bytes: row.get("stored_bytes")?,
        chunks: row.get("chunks")?,
        complete: row.get("complete")?,
        accessed_at: row.get("accessed_at")?,
    })
}
