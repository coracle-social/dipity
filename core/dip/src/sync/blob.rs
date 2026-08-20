//! Blob transfers: the Blossom request/response pair over the sync channel.
//!
//! The relay half serves a peer's `BLOSSOM-REQ` from the [`BlobStore`], the
//! client half fetches blobs a stored event references and this device does
//! not hold, one group of bytes at a time. Each chunk is written as it
//! arrives; the record is completed once the whole file's length is held.
//! `docs/sync.md#blob-sync`.
//!
//! Verification is whole-file for now: the completed transfer is checked
//! against the `x` SHA-256 in the blob's `imeta`. Per-chunk BLAKE3 streaming
//! (`docs/nips/imeta-blake3.md`) slots into the same `record_progress` /
//! `complete_blob` seams once Bao lands.

use anyhow::{Context, Result};
use coracle_lib::filters::Filter;
use sha2::{Digest, Sha256};

use crate::blobstore::BlobStore;
use crate::db::Db;
use crate::db::query as db_query;
use crate::db::sql::hex_key;
use crate::model::{Blob, Query, Registers};
use crate::session::Peer;
use crate::sync::message::{BlossomRequest, BlossomResponse};

/// One GET group's worth of bytes.
pub const BLOB_GROUP_BYTES: u64 = 16 * 1024;

/// The battery floor, percent, below which blob transfers do not start.
/// A transfer starves the link for minutes; a low battery makes that worse.
pub const BLOB_MIN_BATTERY: u8 = 20;

/// The client half's in-flight fetch of one blob.
#[derive(Debug, Clone)]
pub struct BlobFetch {
    /// What is being fetched, metadata and all.
    pub blob: Blob,
    /// The blob's length once a `HEAD` has answered, if it has.
    pub total: Option<u64>,
    /// Bytes held and verified so far, which is where the next GET resumes.
    pub stored: u64,
    /// Correlates the outstanding request with its answer.
    pub request_id: String,
    /// Whether the outstanding request was the `HEAD` or a `GET`.
    pub probing: bool,
}

impl BlobFetch {
    /// A fresh fetch, resuming from nothing: without per-chunk verification,
    /// any partial bytes from a dead transfer cannot be trusted, so they start
    /// over rather than corrupt the re-fetch.
    pub fn begin(next: Blob, request_id: String) -> Self {
        Self {
            blob: next,
            total: None,
            stored: 0,
            request_id,
            probing: true,
        }
    }
}

/// Answer one `BLOSSOM-REQ` against the store.
///
/// A hash the peer could not be offered the anchoring event for is a 404 like
/// any other miss: it does not exist to them, and neither response says which.
pub fn handle_request(
    db: &Db,
    blobs: &dyn BlobStore,
    peer: &Peer,
    request: &BlossomRequest,
) -> Result<BlossomResponse> {
    let Some(blob) = offerable_blob(db, peer, &request.path)? else {
        return Ok(missing(&request.id));
    };

    match request.method.as_str() {
        "HEAD" => match blobs.len(&blob.sha256)? {
            Some(len) => Ok(BlossomResponse {
                id: request.id.clone(),
                status: 200,
                headers: vec![("content-length".into(), len.to_string())],
                body: Vec::new(),
            }),
            None => Ok(missing(&request.id)),
        },
        "GET" => {
            let (start, end) = range(request)?;
            // The range is inclusive on both ends, per HTTP.
            let len = end.saturating_sub(start).saturating_add(1);
            let bytes = blobs.read(&blob.sha256, start, len)?;

            if bytes.is_empty() {
                return Ok(missing(&request.id));
            }

            let last = start + bytes.len() as u64 - 1;

            Ok(BlossomResponse {
                id: request.id.clone(),
                status: 206,
                headers: vec![(
                    "content-range".into(),
                    format!("bytes {start}-{last}/{end}"),
                )],
                body: bytes,
            })
        }
        _ => Ok(missing(&request.id)),
    }
}

/// The request's byte range, or from zero to the end when none is given.
fn range(request: &BlossomRequest) -> Result<(u64, u64)> {
    let Some((_, value)) = request
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("range"))
    else {
        return Ok((0, u64::MAX));
    };

    let Some(spec) = value.strip_prefix("bytes=") else {
        anyhow::bail!("a range that is not bytes");
    };
    let Some((start, end)) = spec.split_once('-') else {
        anyhow::bail!("an open-ended range is not supported");
    };

    Ok((
        start.trim().parse().context("a malformed range start")?,
        end.trim().parse().context("a malformed range end")?,
    ))
}

/// The blob a request path names, if this device knows it and the peer may be
/// served its anchor.
fn offerable_blob(db: &Db, peer: &Peer, path: &str) -> Result<Option<Blob>> {
    let hash = hex_key(path.trim_start_matches('/'))?;
    let Some(blob) = db_query::get_blob(db, &hash)? else {
        return Ok(None);
    };

    // The anchoring event's permissions are the blob's: a peer that could not
    // be served the event cannot fetch its media. A blocked peer is covered by
    // the policy binding, exactly as on the event path.
    let mut query = Query::new()
        .with_filter(Filter::new().add_ids([blob.event_id]))
        .with_registers(Registers::offerable(peer.identity));
    if let Some(policy) = peer.policies.first() {
        query = query.with_policy(policy.clone());
    }

    let anchor = db_query::list_events(db, &query)?;

    Ok(anchor
        .iter()
        .any(|event| event.id == blob.event_id)
        .then_some(blob))
}

/// The 404 every refusal looks like.
fn missing(id: &str) -> BlossomResponse {
    BlossomResponse {
        id: id.to_string(),
        status: 404,
        headers: Vec::new(),
        body: Vec::new(),
    }
}

/// Whether a whole blob verifies against the `x` hash its record names.
pub fn verifies(blob: &Blob, store: &dyn BlobStore) -> Result<bool> {
    let Some(len) = store.len(&blob.sha256)? else {
        return Ok(false);
    };

    let bytes = store.read(&blob.sha256, 0, len)?;
    let digest = Sha256::digest(bytes);

    Ok(hex::encode(digest) == blob.sha256)
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::events::EventId;
    use coracle_lib::tags::Tags;

    use crate::blobstore::MemoryBlobStore;
    use crate::db::{Db, command as db_command};
    use crate::fixtures::{author, note};
    use crate::link::LinkId;
    use crate::model::{BlobRole, Policy};

    fn peer() -> Peer {
        Peer::bind(LinkId(1), [author(2)], &Policy::new(author(1)))
    }

    /// An own event carrying an imeta tag records the blob row, and its bytes
    /// go into the store.
    fn given_blob(store: &MemoryBlobStore, bytes: &[u8]) -> Blob {
        let hash = hex::encode(Sha256::digest(bytes));
        store.append(&hash, bytes).unwrap();

        Blob {
            sha256: hash,
            event_id: EventId::new([0u8; 32]),
            role: BlobRole::Original,
            stored_bytes: bytes.len() as i64,
            ..Blob::new(String::new(), EventId::new([0u8; 32]), BlobRole::Original)
        }
    }

    #[test]
    fn a_peer_can_head_and_get_a_blob_it_is_offered() {
        let db = Db::open_in_memory().unwrap();
        let store = MemoryBlobStore::default();
        let bytes = b"the quick brown fox";
        let blob = given_blob(&store, bytes);

        // An own event recording the blob anchors it in the store's register.
        let event = note(
            author(1),
            1,
            "blob",
            Tags::new().add("imeta", [format!("x {}", blob.sha256), "size 17".into()]),
        );
        db_command::publish_event(&db, &event, &author(1), 1).unwrap();

        let head = handle_request(
            &db,
            &store,
            &peer(),
            &BlossomRequest {
                id: "head-1".into(),
                method: "HEAD".into(),
                path: format!("/{}", blob.sha256),
                headers: vec![],
                body: vec![],
            },
        )
        .unwrap();

        assert_eq!(head.status, 200);
        assert_eq!(
            head.headers
                .iter()
                .find(|(name, _)| name == "content-length")
                .map(|(_, value)| value.as_str()),
            Some("19")
        );

        let get = handle_request(
            &db,
            &store,
            &peer(),
            &BlossomRequest {
                id: "get-1".into(),
                method: "GET".into(),
                path: format!("/{}", blob.sha256),
                headers: vec![("range".into(), "bytes=4-14".into())],
                body: vec![],
            },
        )
        .unwrap();

        assert_eq!(get.status, 206);
        assert_eq!(get.body, b"quick brown" as &[u8]);
    }

    #[test]
    fn an_unknown_hash_is_a_404() {
        let db = Db::open_in_memory().unwrap();
        let store = MemoryBlobStore::default();

        let response = handle_request(
            &db,
            &store,
            &peer(),
            &BlossomRequest {
                id: "1".into(),
                method: "HEAD".into(),
                path: format!("/{}", "ab".repeat(32)),
                headers: vec![],
                body: vec![],
            },
        )
        .unwrap();

        assert_eq!(response.status, 404);
    }

    #[test]
    fn verifies_the_whole_file_against_its_hash() {
        let store = MemoryBlobStore::default();
        let bytes = b"verified bytes";
        let blob = given_blob(&store, bytes);

        assert!(verifies(&blob, &store).unwrap());

        store.append(&blob.sha256, b"!").unwrap();
        assert!(!verifies(&blob, &store).unwrap());
    }
}
