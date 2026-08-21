//! Blob transfers: the Blossom request/response pair over the sync channel.
//!
//! [`BlobExchange`] is one session's blob half, both directions at once. The
//! relay direction serves a peer's `BLOSSOM-REQ` from the [`BlobStore`] — the
//! core's file-backed store by default — and the client direction pulls blobs
//! a stored event references and this device does not hold, one group of
//! bytes at a time. Each chunk is written as it arrives; the record is
//! completed once the whole file's length is held. `docs/sync.md#blob-sync`.
//!
//! Verification is whole-file for now: the completed transfer is checked
//! against the `x` SHA-256 in the blob's `imeta`. Per-chunk BLAKE3 streaming
//! (`docs/nips/imeta-blake3.md`) slots into the same `record_progress` /
//! `complete_blob` seams once Bao lands.

use std::sync::Arc;

use anyhow::{Context, Result};
use coracle_lib::filters::Filter;
use sha2::{Digest, Sha256};

use crate::blobstore::BlobStore;
use crate::clock;
use crate::db::Db;
use crate::db::command;
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

/// One in-flight fetch of one blob.
#[derive(Debug)]
struct BlobFetch {
    /// What is being fetched, metadata and all.
    blob: Blob,
    /// The blob's length once a `HEAD` has answered, if it has.
    total: Option<u64>,
    /// Bytes held so far, which is where the next GET resumes.
    stored: u64,
    /// Correlates the outstanding request with its answer.
    request_id: String,
    /// Whether the outstanding request was the `HEAD` or a `GET`.
    probing: bool,
}

impl BlobFetch {
    /// A fresh fetch, resuming from nothing: without per-chunk verification,
    /// any partial bytes from a dead transfer cannot be trusted, so they start
    /// over rather than corrupt the re-fetch.
    fn begin(blob: Blob, request_id: String) -> Self {
        Self {
            blob,
            total: None,
            stored: 0,
            request_id,
            probing: true,
        }
    }
}

/// One session's blob half, over the store the shell provided.
///
/// Serves the peer's requests, drives this device's one fetch at a time, and
/// owns the battery gate and the request for a bulk channel — so the session
/// only forwards requests and responses.
pub struct BlobExchange {
    /// Where blob bytes live.
    store: Arc<dyn BlobStore>,
    /// The fetch in flight, if any.
    active: Option<BlobFetch>,
    /// The battery level in percent, as the shell last reported it.
    battery: Option<u8>,
    /// Whether a bulk channel has been asked for and not yet taken.
    wants_l2cap: bool,
}

impl BlobExchange {
    /// A fresh exchange over the shell's blob store.
    #[must_use]
    pub fn new(store: Arc<dyn BlobStore>) -> Self {
        Self {
            store,
            active: None,
            battery: None,
            wants_l2cap: false,
        }
    }

    /// The battery level in percent, which gates new transfers.
    pub fn set_battery(&mut self, level: Option<u8>) {
        self.battery = level;
    }

    /// Whether the shell should be told to open L2CAP for this link, once.
    /// Once bytes move, the setup round trip is worth it.
    pub fn take_l2cap_request(&mut self) -> bool {
        std::mem::take(&mut self.wants_l2cap)
    }

    /// Answer one of the peer's `BLOSSOM-REQ`s against the store.
    ///
    /// A hash the peer could not be offered the anchoring event for is a 404
    /// like any other miss: it does not exist to them, and neither response
    /// says which.
    pub fn serve(&self, db: &Db, peer: &Peer, request: &BlossomRequest) -> Result<BlossomResponse> {
        let Some(blob) = offerable_blob(db, peer, &request.path)? else {
            return Ok(missing(&request.id));
        };

        match request.method.as_str() {
            "HEAD" => match self.store.len(&blob.sha256)? {
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
                let bytes = self.store.read(&blob.sha256, start, len)?;

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

    /// Start the next fetch if none is in flight, returning the `HEAD` that
    /// probes it before any bytes move.
    pub fn poll(&mut self, db: &Db) -> Result<Option<BlossomRequest>> {
        if self.active.is_some() {
            return Ok(None);
        }

        // Low battery skips the transfer; it is the one thing a fetch cannot
        // be interrupted for on a phone.
        if self.battery.is_some_and(|level| level < BLOB_MIN_BATTERY) {
            return Ok(None);
        }

        let Some(wanted) = db_query::wanted_blobs(db, 1)?.into_iter().next() else {
            return Ok(None);
        };

        // Partial bytes from a dead transfer cannot be trusted without
        // per-chunk verification, so a re-fetch starts clean.
        if self.store.has(&wanted.sha256)? {
            self.store.delete(&wanted.sha256)?;
        }

        let request_id = format!("blob-{}", &wanted.sha256[..16]);
        let fetch = BlobFetch::begin(wanted, request_id);
        let head = request(&fetch, "HEAD", &[]);

        self.active = Some(fetch);

        Ok(Some(head))
    }

    /// Advance the in-flight fetch on one `BLOSSOM-RES`, returning the next
    /// request if the fetch continues. A response that ends the fetch — done,
    /// missing, or refused — frees the slot for [`poll`](Self::poll).
    pub fn on_response(
        &mut self,
        db: &Db,
        response: &BlossomResponse,
    ) -> Result<Option<BlossomRequest>> {
        let Some(mut fetch) = self.active.take() else {
            return Ok(None);
        };

        if fetch.request_id != response.id {
            self.active = Some(fetch);
            return Ok(None);
        }

        if fetch.probing {
            // The HEAD answers: either the length, or that the peer lacks it.
            if response.status == 404 {
                return Ok(None);
            }

            let total = response
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.parse::<u64>().ok());

            let Some(total) = total else {
                return Ok(None);
            };

            fetch.total = Some(total);
            fetch.probing = false;

            if fetch.stored >= total {
                return self.finish(db, &fetch).map(|()| None);
            }
        } else if response.status == 206 {
            // One group of bytes arrived.
            self.store.append(&fetch.blob.sha256, &response.body)?;
            fetch.stored += response.body.len() as u64;

            command::record_blob_progress(db, &fetch.blob.sha256, fetch.stored as i64, None)?;

            if fetch.total.is_some_and(|total| fetch.stored >= total) {
                return self.finish(db, &fetch).map(|()| None);
            }
        } else {
            // Anything else ends this fetch.
            return Ok(None);
        }

        let get = next_group(&fetch);

        self.wants_l2cap = true;
        self.active = Some(fetch);

        Ok(Some(get))
    }

    /// Mark a fetch whole after verifying the assembled bytes.
    fn finish(&self, db: &Db, fetch: &BlobFetch) -> Result<()> {
        if verifies(&fetch.blob, self.store.as_ref())? {
            command::complete_blob(db, &fetch.blob.sha256, fetch.stored as i64, clock::now())?;
        }

        Ok(())
    }
}

/// One Blossom request over the fetch's blob.
fn request(fetch: &BlobFetch, method: &str, headers: &[(String, String)]) -> BlossomRequest {
    BlossomRequest {
        id: fetch.request_id.clone(),
        method: method.to_string(),
        path: format!("/{}", fetch.blob.sha256),
        headers: headers.to_vec(),
        body: Vec::new(),
    }
}

/// The `GET` for the next group of a probe-completed fetch.
fn next_group(fetch: &BlobFetch) -> BlossomRequest {
    let start = fetch.stored;
    let end = start + BLOB_GROUP_BYTES - 1;

    request(
        fetch,
        "GET",
        &[("range".to_string(), format!("bytes={start}-{end}"))],
    )
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
fn verifies(blob: &Blob, store: &dyn BlobStore) -> Result<bool> {
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

    /// A memory-backed exchange, returning the store too so tests can inspect
    /// what lands in it.
    fn exchange() -> (BlobExchange, Arc<MemoryBlobStore>) {
        let store = Arc::new(MemoryBlobStore::default());

        (BlobExchange::new(store.clone()), store)
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

    /// Publish an event wanting a blob over `bytes`, returning its hash.
    fn given_wanted(db: &Db, bytes: &[u8]) -> String {
        let hash = hex::encode(Sha256::digest(bytes));
        let event = note(
            author(1),
            1,
            "with a blob",
            Tags::new().add(
                "imeta",
                [format!("x {hash}"), format!("size {}", bytes.len())],
            ),
        );

        db_command::publish_event(db, &event, &author(1), 1).unwrap();

        hash
    }

    /// Run one whole fetch — HEAD, then GET with the full body — against
    /// `blobs`, returning the blob's hash.
    fn drive_fetch(blobs: &mut BlobExchange, db: &Db, bytes: &[u8]) -> String {
        let hash = given_wanted(db, bytes);

        // The fetch begins with a HEAD.
        let head = blobs.poll(db).unwrap().expect("a HEAD to probe");
        assert_eq!(head.method, "HEAD");

        // The HEAD answers with the length, and a GET follows.
        let get = blobs
            .on_response(
                db,
                &BlossomResponse {
                    id: head.id.clone(),
                    status: 200,
                    headers: vec![("content-length".into(), bytes.len().to_string())],
                    body: vec![],
                },
            )
            .unwrap()
            .expect("a GET after the probe");
        assert_eq!(get.method, "GET");

        // The GET answers with the bytes, the whole blob now held and complete.
        let done = blobs
            .on_response(
                db,
                &BlossomResponse {
                    id: get.id.clone(),
                    status: 206,
                    headers: vec![],
                    body: bytes.to_vec(),
                },
            )
            .unwrap();
        assert!(done.is_none(), "the finished fetch asked for more");

        hash
    }

    #[test]
    fn a_peer_can_head_and_get_a_blob_it_is_offered() {
        let db = Db::open_in_memory().unwrap();
        let (blobs, store) = exchange();
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

        let head = blobs
            .serve(
                &db,
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

        let get = blobs
            .serve(
                &db,
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
        let (blobs, _) = exchange();

        let response = blobs
            .serve(
                &db,
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

    #[test]
    fn a_wanted_blob_is_driven_through_head_and_get() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();

        let bytes = b"the quick brown fox";
        let hash = drive_fetch(&mut blobs, &db, bytes);

        assert_eq!(store.len(&hash).unwrap(), Some(19));
        assert!(db_query::get_blob(&db, &hash).unwrap().unwrap().complete);
        assert!(blobs.active.is_none());

        // Moving bytes is what justifies the bulk channel, once.
        assert!(blobs.take_l2cap_request());
        assert!(!blobs.take_l2cap_request());
    }

    #[test]
    fn a_blob_fetch_writes_a_real_file() {
        use crate::blobstore::FileBlobStore;
        use std::fs;

        // A fresh store of the same kind the shells use, over a real directory.
        let dir = std::env::temp_dir().join(format!(
            "dip-blob-exchange-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = Arc::new(FileBlobStore::open(&dir).unwrap());
        let mut blobs = BlobExchange::new(store.clone());

        let db = Db::open_in_memory().unwrap();
        let bytes = b"the quick brown fox";
        let hash = drive_fetch(&mut blobs, &db, bytes);

        // The bytes are on disk, one file named by the hash, and the record is
        // complete — the file-backed store is what a real session uses.
        assert_eq!(store.len(&hash).unwrap(), Some(19));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        assert!(dir.join(&hash).is_file());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_battery_floor_gates_fetches() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();

        given_wanted(&db, b"below the floor");

        blobs.set_battery(Some(BLOB_MIN_BATTERY - 1));
        assert!(
            blobs.poll(&db).unwrap().is_none(),
            "a low battery fetches nothing"
        );

        blobs.set_battery(Some(BLOB_MIN_BATTERY));
        assert!(blobs.poll(&db).unwrap().is_some());
    }

    #[test]
    fn a_missing_blob_ends_the_fetch_and_frees_the_slot() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();

        given_wanted(&db, b"nobody holds this");

        let head = blobs.poll(&db).unwrap().unwrap();
        let next = blobs
            .on_response(
                &db,
                &BlossomResponse {
                    id: head.id,
                    status: 404,
                    headers: vec![],
                    body: vec![],
                },
            )
            .unwrap();

        // Nothing further is asked of this peer, and the still-wanted blob is
        // free to be probed again.
        assert!(next.is_none());
        assert!(blobs.poll(&db).unwrap().is_some());
    }
}
