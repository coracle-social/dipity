//! Writes over `blob`.

use anyhow::{Context, Result};
use rusqlite::params;

use crate::db::Tx;
use crate::model::Blob;

use super::events::{self, BlobChange};

/// Record a blob a stored event references. Returns whether it was new.
///
/// The first event to reference a hash — by seen time, since that is the order
/// events arrive in — anchors it, and a later reference changes nothing. The
/// anchor is what the blob's permissions come from, so moving it on every
/// mention would let a later event widen who can fetch an earlier one's media.
///
/// # Errors
///
/// If the write fails, including when the anchoring event is not stored.
pub fn record(tx: &Tx<'_>, blob: &Blob) -> Result<bool> {
    let imeta = serde_json::to_string(&blob.imeta)
        .with_context(|| format!("serializing the imeta tag for blob {}", blob.sha256))?;

    let written = tx
        .prepare_cached(
            "INSERT OR IGNORE INTO blob (
                 sha256, event_id, role, url, mime_type, size, dim, blurhash, alt, blake3,
                 imeta, stored_bytes, chunks, complete, accessed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        )?
        .execute(params![
            blob.sha256,
            blob.event_id,
            blob.role.as_str(),
            blob.url,
            blob.mime_type,
            blob.size,
            blob.dim,
            blob.blurhash,
            blob.alt,
            blob.blake3,
            imeta,
            blob.stored_bytes,
            blob.chunks,
            blob.complete,
            blob.accessed_at,
        ])
        .with_context(|| format!("recording blob {}", blob.sha256))?;

    if written == 0 {
        return Ok(false);
    }

    events::notify(tx, BlobChange::Recorded(Box::new(blob.clone())));

    Ok(true)
}

/// Record how much of a blob is held, and which chunks verified.
///
/// The bitmap is what makes a transfer resumable: a BLE link drops mid-file
/// often enough that restarting from zero would mean never finishing a large
/// one. Called per group of chunks rather than per chunk.
pub fn record_progress(
    tx: &Tx<'_>,
    sha256: &str,
    stored_bytes: i64,
    chunks: Option<&[u8]>,
) -> Result<bool> {
    let written = tx
        .prepare_cached("UPDATE blob SET stored_bytes = ?2, chunks = ?3 WHERE sha256 = ?1")?
        .execute(params![sha256, stored_bytes, chunks])
        .with_context(|| format!("recording progress for blob {sha256}"))?;

    if written == 0 {
        return Ok(false);
    }

    events::notify(tx, BlobChange::Progressed(sha256.to_string(), stored_bytes));

    Ok(true)
}

/// Mark a blob whole: every chunk arrived and verified against the BLAKE3 root.
pub fn mark_complete(tx: &Tx<'_>, sha256: &str, stored_bytes: i64, at: i64) -> Result<bool> {
    let written = tx
        .prepare_cached(
            "UPDATE blob
             SET complete = 1, stored_bytes = ?2, size = COALESCE(size, ?2), accessed_at = ?3
             WHERE sha256 = ?1 AND complete = 0",
        )?
        .execute(params![sha256, stored_bytes, at])
        .with_context(|| format!("completing blob {sha256}"))?;

    if written == 0 {
        return Ok(false);
    }

    events::notify(tx, BlobChange::Completed(sha256.to_string()));

    Ok(true)
}

/// Note that a blob was read, which is what LRU eviction orders on.
pub fn touch(tx: &Tx<'_>, sha256: &str, at: i64) -> Result<bool> {
    let written = tx
        .prepare_cached("UPDATE blob SET accessed_at = ?2 WHERE sha256 = ?1")?
        .execute(params![sha256, at])
        .with_context(|| format!("touching blob {sha256}"))?;

    Ok(written > 0)
}

/// Move a blob's anchor to another stored event.
///
/// For the case where the anchoring event is deleted while something else still
/// references the hash: without this the record goes with it, by cascade, and
/// the bytes on disk are orphaned.
///
/// # Errors
///
/// If the write fails, including when the new anchor is not stored.
pub fn reanchor(tx: &Tx<'_>, sha256: &str, event_id: &str) -> Result<bool> {
    let written = tx
        .prepare_cached("UPDATE blob SET event_id = ?2 WHERE sha256 = ?1")?
        .execute(params![sha256, event_id])
        .with_context(|| format!("reanchoring blob {sha256} to {event_id}"))?;

    Ok(written > 0)
}

/// Forget a blob. Returns whether it was there.
///
/// Only the record goes; deleting the bytes is the blob store's job, and it
/// takes this notification as its cue.
pub fn remove(tx: &Tx<'_>, sha256: &str) -> Result<bool> {
    let removed = tx
        .prepare_cached("DELETE FROM blob WHERE sha256 = ?1")?
        .execute(params![sha256])
        .with_context(|| format!("removing blob {sha256}"))?;

    if removed == 0 {
        return Ok(false);
    }

    events::notify(tx, BlobChange::Removed(sha256.to_string()));

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::tags::{Tag, Tags};

    use crate::db::blob::query;
    use crate::db::event::command as event_command;
    use crate::db::open_in_memory;
    use crate::fixtures::{author, id, note, peer};
    use crate::model::BlobRole;

    /// Store an event to anchor blobs against, and return its id.
    fn store_event(tx: &Tx<'_>, content: &str) -> String {
        let event = note(author(1), 100, content, Tags::new());

        event_command::save(tx, &event, &peer(), 10).unwrap();

        id(&event)
    }

    #[test]
    fn the_first_event_to_reference_a_hash_anchors_it() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let first = store_event(&tx, "first");
        let second = store_event(&tx, "second");

        assert!(record(&tx, &Blob::new("hash", &first, BlobRole::Original)).unwrap());
        assert!(!record(&tx, &Blob::new("hash", &second, BlobRole::Original)).unwrap());

        assert_eq!(query::get(&tx, "hash").unwrap().unwrap().event_id, first);
    }

    /// The batched read has to answer exactly what the per-event one does,
    /// including the order within an event and the events that have nothing.
    #[test]
    fn a_batched_read_answers_what_the_per_event_one_does() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let first = store_event(&tx, "two blobs");
        let second = store_event(&tx, "one blob");
        let bare = store_event(&tx, "no blobs at all");

        // Recorded out of hash order, so grouping cannot be passing by accident.
        for (sha256, event_id) in [
            ("ccc", &first),
            ("aaa", &first),
            ("bbb", &second),
            ("ddd", &second),
        ] {
            record(&tx, &Blob::new(sha256, event_id, BlobRole::Original)).unwrap();
        }

        let ids = [first.clone(), second.clone(), bare.clone()];
        let batched = query::list_for_events(&tx, &ids).unwrap();

        for id in &ids {
            let one = query::list_for_event(&tx, id).unwrap();

            assert_eq!(
                batched.get(id).cloned().unwrap_or_default(),
                one,
                "batched blobs for {id} differ from the per-event read"
            );
        }

        // Sorted within the event, and an event with nothing is simply absent.
        assert_eq!(
            batched[&first]
                .iter()
                .map(|blob| blob.sha256.as_str())
                .collect::<Vec<_>>(),
            ["aaa", "ccc"]
        );
        assert!(!batched.contains_key(&bare));

        assert!(query::list_for_events(&tx, &[]).unwrap().is_empty());
    }

    #[test]
    fn the_whole_imeta_tag_survives_the_round_trip() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let event_id = store_event(&tx, "with media");
        let tag = Tag::new("imeta", ["x hash", "m image/jpeg", "service nostr.build"]);
        let blob = Blob::from_imeta(&tag, &event_id, BlobRole::Original).unwrap();

        record(&tx, &blob).unwrap();

        let stored = query::get(&tx, "hash").unwrap().unwrap();

        assert_eq!(stored, blob);
        // A key with no column of its own is still readable, which is the point
        // of keeping the tag rather than only what was parsed out of it.
        assert_eq!(stored.imeta_value("service"), Some("nostr.build"));
    }

    #[test]
    fn a_partial_transfer_is_resumable() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let event_id = store_event(&tx, "with media");
        record(&tx, &Blob::new("hash", &event_id, BlobRole::Original)).unwrap();

        assert!(record_progress(&tx, "hash", 4_096, Some(&[0b0000_0111])).unwrap());

        let blob = query::get(&tx, "hash").unwrap().unwrap();

        assert_eq!(blob.stored_bytes, 4_096);
        assert_eq!(blob.chunks.as_deref(), Some(&[0b0000_0111][..]));
        assert!(!blob.complete);
        assert!(!query::is_complete(&tx, "hash").unwrap());
    }

    #[test]
    fn completing_takes_a_blob_off_the_want_list() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let event_id = store_event(&tx, "with media");
        record(&tx, &Blob::new("hash", &event_id, BlobRole::Original)).unwrap();

        assert_eq!(query::wanted(&tx, 10).unwrap().len(), 1);
        assert!(mark_complete(&tx, "hash", 8_192, 100).unwrap());
        assert!(query::wanted(&tx, 10).unwrap().is_empty());

        // Already whole, so there is nothing to announce a second time.
        assert!(!mark_complete(&tx, "hash", 8_192, 200).unwrap());

        let blob = query::get(&tx, "hash").unwrap().unwrap();
        assert_eq!(blob.size, Some(8_192));
        assert_eq!(query::stored_bytes(&tx, BlobRole::Original).unwrap(), 8_192);
    }

    #[test]
    fn previews_are_wanted_before_originals() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let event_id = store_event(&tx, "with media");
        record(
            &tx,
            &Blob::new("original-hash", &event_id, BlobRole::Original),
        )
        .unwrap();
        record(
            &tx,
            &Blob::new("preview-hash", &event_id, BlobRole::Preview),
        )
        .unwrap();

        let wanted = query::wanted(&tx, 10).unwrap();

        assert_eq!(wanted[0].sha256, "preview-hash");
        assert_eq!(wanted[1].sha256, "original-hash");
    }

    #[test]
    fn eviction_order_is_least_recently_read() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let event_id = store_event(&tx, "with media");

        for hash in ["a", "b"] {
            record(&tx, &Blob::new(hash, &event_id, BlobRole::Original)).unwrap();
            mark_complete(&tx, hash, 1_024, 100).unwrap();
        }

        touch(&tx, "a", 300).unwrap();

        let candidates = query::least_recently_used(&tx, BlobRole::Original, 10).unwrap();

        assert_eq!(
            candidates
                .iter()
                .map(|blob| blob.sha256.as_str())
                .collect::<Vec<_>>(),
            ["b", "a"]
        );
    }

    #[test]
    fn a_blob_record_dies_with_the_event_that_anchors_it() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let first = store_event(&tx, "first");
        let second = store_event(&tx, "second");
        record(&tx, &Blob::new("hash", &first, BlobRole::Original)).unwrap();

        // Unless something else still references it, which is what reanchoring
        // is for — the bytes are on disk either way.
        assert!(reanchor(&tx, "hash", &second).unwrap());
        event_command::delete(&tx, &first).unwrap();
        assert!(query::get(&tx, "hash").unwrap().is_some());

        event_command::delete(&tx, &second).unwrap();
        assert!(query::get(&tx, "hash").unwrap().is_none());
    }
}
