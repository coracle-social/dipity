//! Blob transfers: the Blossom request/response pair over the sync channel.
//!
//! [`BlobExchange`] is one session's blob half, both directions at once. The
//! relay direction serves a peer's `BLOSSOM-REQ` from the [`BlobStore`] — the
//! core's file-backed store by default — and the client direction pulls blobs
//! a stored event references and this device does not hold, one group of
//! bytes at a time. Each group is written as it arrives; the record is
//! completed once the whole file is held and hashes to its address.
//! `docs/sync.md#blob-sync`.
//!
//! Both directions are metered against the session's [`Quota`]: a transfer is
//! an unbounded write to the user's disk in one direction and an unbounded
//! read of their battery in the other, so neither runs on trust alone.
//! `docs/sync.md#quotas-1`.
//!
//! Verification is whole-file for now: the completed transfer is checked
//! against the `x` SHA-256 in the blob's `imeta`. Per-chunk BLAKE3 streaming
//! (`docs/nips/imeta-blake3.md`) slots into the same `record_progress` /
//! `complete_blob` seams once Bao lands, and until it does a transfer that
//! drops restarts rather than resuming.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use coracle_lib::filters::Filter;

use crate::blobs::BlobStore;
use crate::clock;
use crate::db::Db;
use crate::db::command;
use crate::db::query as db_query;
use crate::model::{Blob, BlobHash, Identity};
use crate::session::Peer;
use crate::sync::Quota;
use crate::sync::message::{BlossomRequest, BlossomResponse};
use crate::sync::relay;

/// One GET group's worth of bytes.
pub const BLOB_GROUP_BYTES: u64 = 16 * 1024;

/// The battery floor, percent, below which blob transfers do not start.
/// A transfer starves the link for minutes; a low battery makes that worse.
pub const BLOB_MIN_BATTERY: u8 = 20;

/// Outstanding blob bytes at which a bulk channel earns its setup round trip.
///
/// Four groups: below that the transfer is over in fewer round trips than
/// opening the channel costs, and the shell is asked for nothing.
/// `docs/transport.md#the-l2cap-bandwidth-upgrade`.
pub const L2CAP_THRESHOLD_BYTES: u64 = 4 * BLOB_GROUP_BYTES;

/// One in-flight fetch of one blob.
#[derive(Debug)]
struct BlobFetch {
    /// What is being fetched, metadata and all.
    blob: Blob,
    /// The blob's length once a `HEAD` has answered, if it has.
    total: Option<u64>,
    /// Bytes held so far, which is where the next GET resumes.
    stored: u64,
    /// How many bytes the outstanding GET asked for, which bounds its answer.
    requested: u64,
    /// Correlates the outstanding request with its answer.
    request_id: String,
    /// Whether the outstanding request was the `HEAD` or a `GET`.
    probing: bool,
}

impl BlobFetch {
    /// A fresh fetch, resuming from nothing: without per-chunk verification,
    /// any partial bytes from a dead transfer cannot be trusted, so they start
    /// over rather than corrupt the re-fetch.
    fn begin(blob: Blob) -> Self {
        let request_id = format!("blob-{}", blob.sha256.short());

        Self {
            blob,
            total: None,
            stored: 0,
            requested: 0,
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
    /// Whether it has been asked for at all. One link, one request: the
    /// channel outlives a group, so re-arming per group would ask again every
    /// few seconds of a large transfer.
    asked_l2cap: bool,
    /// Hashes this peer answered badly — a miss, a refusal, or bytes that
    /// failed their hash — skipped for the rest of the session.
    ///
    /// Without it the freed slot picks the same top-of-want-list blob and asks
    /// again immediately, which is a livelock between two radios rather than a
    /// retry.
    unavailable: HashSet<BlobHash>,
    /// Whether the peer has said it has no blob budget left for this device.
    refused: bool,
    /// Blob bytes taken from the peer this session.
    fetched_bytes: u64,
    /// Blob bytes served to the peer this session.
    served_bytes: u64,
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
            asked_l2cap: false,
            unavailable: HashSet::new(),
            refused: false,
            fetched_bytes: 0,
            served_bytes: 0,
        }
    }

    /// The battery level in percent, which gates new transfers.
    pub fn set_battery(&mut self, level: Option<u8>) {
        self.battery = level;
    }

    /// Whether a transfer is in flight, waiting on the peer's next answer.
    #[must_use]
    pub fn is_fetching(&self) -> bool {
        self.active.is_some()
    }

    /// Whether the shell should be told to open L2CAP for this link, once.
    pub fn take_l2cap_request(&mut self) -> bool {
        std::mem::take(&mut self.wants_l2cap)
    }

    /// Answer one of the peer's `BLOSSOM-REQ`s against the store.
    ///
    /// A hash the peer could not be offered the anchoring event for is a 404
    /// like any other miss: it does not exist to them, and neither response
    /// says which. A peer that has spent its blob budget gets a 429, which
    /// does say which — it is the answer that stops it asking.
    pub fn serve(
        &mut self,
        db: &Db,
        peer: &Peer,
        local: &Identity,
        request: &BlossomRequest,
        quota: Quota,
    ) -> Result<BlossomResponse> {
        let budget = quota.blob_bytes.saturating_sub(self.served_bytes);

        // A stranger's blob budget is zero, so this is also how a peer outside
        // the trust graph is told there are no bytes here for it.
        if budget == 0 {
            return Ok(status(&request.id, 429));
        }

        // A path that is not a hash names nothing, and is answered like
        // anything else this device does not hold.
        let Ok(hash) = BlobHash::parse(request.path.trim_start_matches('/')) else {
            return Ok(missing(&request.id));
        };

        let Some(blob) = offerable_blob(db, peer, local, &hash)? else {
            return Ok(missing(&request.id));
        };

        let Some(held) = self.store.len(&blob.sha256)? else {
            return Ok(missing(&request.id));
        };

        // The whole file's length is what the event declared — the event id
        // commits to it — and what is on disk only once the record says the
        // transfer finished. What may be served is what is on disk either way.
        let total = blob
            .size
            .and_then(|size| u64::try_from(size).ok())
            .or_else(|| blob.complete.then_some(held));
        let available = total.map_or(held, |total| held.min(total));
        let whole = total.is_some_and(|total| available >= total);

        if available == 0 {
            return Ok(missing(&request.id));
        }

        match request.method.as_str() {
            "HEAD" => Ok(self.head(request, &blob, available, total, whole)),
            "GET" => self.get(request, &blob, available, total, whole, budget),
            _ => Ok(status(&request.id, 405)),
        }
    }

    /// Answer a `HEAD`: the content type, and how much of the blob is here.
    ///
    /// A partial holding answers 206 with the range that is available, so the
    /// fetcher learns the whole length without mistaking a prefix for it.
    /// `docs/sync.md#blob-sync`.
    fn head(
        &self,
        request: &BlossomRequest,
        blob: &Blob,
        available: u64,
        total: Option<u64>,
        whole: bool,
    ) -> BlossomResponse {
        let mut headers = vec![
            ("accept-ranges".to_string(), "bytes".to_string()),
            ("content-length".to_string(), available.to_string()),
        ];

        if let Some(mime) = &blob.mime_type {
            headers.push(("content-type".to_string(), mime.clone()));
        }

        if !whole {
            headers.push((
                "content-range".to_string(),
                content_range(0, available - 1, total),
            ));
        }

        BlossomResponse {
            id: request.id.clone(),
            status: if whole { 200 } else { 206 },
            headers,
            body: Vec::new(),
        }
    }

    /// Answer a `GET`, whole or by range, up to what the peer's budget allows.
    fn get(
        &mut self,
        request: &BlossomRequest,
        blob: &Blob,
        available: u64,
        total: Option<u64>,
        whole: bool,
        budget: u64,
    ) -> Result<BlossomResponse> {
        let Some(wanted) = requested_range(request) else {
            return Ok(status(&request.id, 400));
        };

        let (start, end) = match wanted {
            Requested::Whole => (0, None),
            Requested::Range { start, end } => (start, end),
        };

        // Inclusive on both ends, per HTTP, and never past what is here.
        let last = end.unwrap_or(available - 1).min(available - 1);

        if start > last {
            return Ok(status(&request.id, 416));
        }

        let len = (last - start + 1).min(budget);
        let bytes = self.store.read(&blob.sha256, start, len)?;

        if bytes.is_empty() {
            return Ok(missing(&request.id));
        }

        self.served_bytes = self.served_bytes.saturating_add(bytes.len() as u64);

        let last = start + bytes.len() as u64 - 1;
        let mut headers = vec![("content-length".to_string(), bytes.len().to_string())];

        if let Some(mime) = &blob.mime_type {
            headers.push(("content-type".to_string(), mime.clone()));
        }

        // Only an answer that is the entire blob is a 200; anything less is a
        // range, and says which one it is.
        let entire = whole && matches!(wanted, Requested::Whole) && bytes.len() as u64 == available;

        if entire {
            headers.push(("accept-ranges".to_string(), "bytes".to_string()));
        } else {
            headers.push((
                "content-range".to_string(),
                content_range(start, last, total),
            ));
        }

        Ok(BlossomResponse {
            id: request.id.clone(),
            status: if entire { 200 } else { 206 },
            headers,
            body: bytes,
        })
    }

    /// Start the next fetch if none is in flight, returning the `HEAD` that
    /// probes it before any bytes move.
    pub fn poll(&mut self, db: &Db, quota: Quota) -> Result<Option<BlossomRequest>> {
        if self.active.is_some() || self.refused {
            return Ok(None);
        }

        // Low battery skips the transfer; it is the one thing a fetch cannot
        // be interrupted for on a phone.
        if self.battery.is_some_and(|level| level < BLOB_MIN_BATTERY) {
            return Ok(None);
        }

        if self.fetched_bytes >= quota.blob_bytes {
            return Ok(None);
        }

        let Some(wanted) = self.next_wanted(db)? else {
            return Ok(None);
        };

        // Partial bytes from a dead transfer cannot be trusted without
        // per-chunk verification, so a re-fetch starts clean — record
        // included, or the row would claim bytes the store no longer holds.
        if self.store.has(&wanted.sha256)? {
            self.store.delete(&wanted.sha256)?;
            command::record_blob_progress(db, &wanted.sha256, 0, None)?;
        }

        let fetch = BlobFetch::begin(wanted);
        let head = request(&fetch, "HEAD", &[]);

        self.active = Some(fetch);

        Ok(Some(head))
    }

    /// The next wanted blob this peer has not already failed on.
    fn next_wanted(&self, db: &Db) -> Result<Option<Blob>> {
        // One row further down the want list per hash already skipped.
        let limit = self.unavailable.len().saturating_add(1);

        Ok(db_query::wanted_blobs(db, limit)?
            .into_iter()
            .find(|blob| !self.unavailable.contains(&blob.sha256)))
    }

    /// Advance the in-flight fetch on one `BLOSSOM-RES`, returning the next
    /// request if the fetch continues. A response that ends the fetch — done,
    /// missing, or refused — frees the slot for [`poll`](Self::poll).
    pub fn on_response(
        &mut self,
        db: &Db,
        peer: &Peer,
        response: &BlossomResponse,
        quota: Quota,
    ) -> Result<Option<BlossomRequest>> {
        let Some(mut fetch) = self.active.take() else {
            return Ok(None);
        };

        if fetch.request_id != response.id {
            self.active = Some(fetch);
            return Ok(None);
        }

        // The peer has no blob budget left for this device, so nothing else it
        // is asked this session will be answered either.
        if response.status == 429 {
            self.refused = true;
            return Ok(None);
        }

        if fetch.probing {
            let Some(total) = probed_total(&fetch, response) else {
                self.give_up(peer, &fetch.blob.sha256, "cannot serve the whole blob");
                return Ok(None);
            };

            // A blob that cannot fit in what is left of the budget is not
            // started: the bytes would be spent without ever completing it.
            if total > quota.blob_bytes.saturating_sub(self.fetched_bytes) {
                self.give_up(peer, &fetch.blob.sha256, "is larger than the budget left");
                return Ok(None);
            }

            self.consider_l2cap(total);

            fetch.total = Some(total);
            fetch.probing = false;
        } else if response.status == 200 || response.status == 206 {
            let allowed = fetch
                .requested
                .min(fetch.total.unwrap_or(0).saturating_sub(fetch.stored))
                .min(quota.blob_bytes.saturating_sub(self.fetched_bytes));

            // More than was asked for, more than the event declared, or more
            // than the budget allows: writing it is what makes a blob transfer
            // an unbounded write to the user's disk.
            if response.body.is_empty() || response.body.len() as u64 > allowed {
                self.give_up(
                    peer,
                    &fetch.blob.sha256,
                    "answered a range it was not asked",
                );
                return Ok(None);
            }

            self.store.append(&fetch.blob.sha256, &response.body)?;
            fetch.stored += response.body.len() as u64;
            self.fetched_bytes = self
                .fetched_bytes
                .saturating_add(response.body.len() as u64);

            command::record_blob_progress(
                db,
                &fetch.blob.sha256,
                i64::try_from(fetch.stored).unwrap_or(i64::MAX),
                None,
            )?;

            if fetch.total.is_some_and(|total| fetch.stored >= total) {
                return self.finish(db, peer, &fetch).map(|()| None);
            }

            // The budget ran out mid-file; the rest waits for another session.
            if self.fetched_bytes >= quota.blob_bytes {
                return Ok(None);
            }
        } else {
            // A miss or a refusal. The peer is not asked for this hash again
            // this session, which is what keeps the freed slot from putting
            // the same request back on the radio.
            self.give_up(peer, &fetch.blob.sha256, "does not hold it");
            return Ok(None);
        }

        let get = next_group(&mut fetch);

        self.active = Some(fetch);

        Ok(Some(get))
    }

    /// Mark a fetch whole after verifying the assembled bytes.
    fn finish(&mut self, db: &Db, peer: &Peer, fetch: &BlobFetch) -> Result<()> {
        if verifies(&fetch.blob, self.store.as_ref())? {
            command::complete_blob(
                db,
                &fetch.blob.sha256,
                i64::try_from(fetch.stored).unwrap_or(i64::MAX),
                clock::now(),
            )?;

            return Ok(());
        }

        // The bytes do not hash to the address they were fetched under, so
        // they go and the peer that sent them is not asked for this blob
        // again this session.
        self.store.delete(&fetch.blob.sha256)?;
        command::record_blob_progress(db, &fetch.blob.sha256, 0, None)?;
        self.give_up(
            peer,
            &fetch.blob.sha256,
            "sent bytes that failed their hash",
        );

        Ok(())
    }

    /// Skip a hash for the rest of the session, naming the peer that earned it.
    fn give_up(&mut self, peer: &Peer, hash: &BlobHash, reason: &str) {
        // A bad answer names the peer that gave it. `docs/sync.md#blob-sync`.
        log::info!(
            "blob {hash} skipped for this session: peer {} {reason}",
            peer.pubkeys()
                .next()
                .map_or_else(|| "unidentified".to_string(), ToString::to_string)
        );

        self.unavailable.insert(hash.clone());
    }

    /// Ask the shell for a bulk channel, if this transfer is big enough to pay
    /// for it and nothing has asked yet.
    fn consider_l2cap(&mut self, outstanding: u64) {
        if !self.asked_l2cap && outstanding >= L2CAP_THRESHOLD_BYTES {
            self.wants_l2cap = true;
            self.asked_l2cap = true;
        }
    }
}

/// What a `range` header asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Requested {
    /// No range header: the whole blob.
    Whole,
    /// `bytes=start-end`, or `bytes=start-` for everything from `start` on.
    Range {
        /// The first byte wanted.
        start: u64,
        /// The last byte wanted, inclusive, or `None` for the rest of it.
        end: Option<u64>,
    },
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

/// The `GET` for the next group of a probe-completed fetch, recording how much
/// it asked for so the answer can be held to it.
fn next_group(fetch: &mut BlobFetch) -> BlossomRequest {
    let start = fetch.stored;
    let remaining = fetch
        .total
        .map_or(BLOB_GROUP_BYTES, |total| total.saturating_sub(start));

    // Never zero: a range is inclusive, so a zero-length one has no spelling.
    fetch.requested = remaining.clamp(1, BLOB_GROUP_BYTES);

    let end = start + fetch.requested - 1;

    request(
        fetch,
        "GET",
        &[("range".to_string(), format!("bytes={start}-{end}"))],
    )
}

/// What a `HEAD` answer says the whole blob's length is, or `None` when this
/// peer cannot serve it whole.
///
/// A 206 means the peer holds a prefix. Without per-chunk verification a
/// prefix can be neither trusted nor resumed, so there is nothing to fetch
/// from it and the hash waits for a peer that holds the file.
fn probed_total(fetch: &BlobFetch, response: &BlossomResponse) -> Option<u64> {
    if response.status != 200 {
        return None;
    }

    let total: u64 = header(&response.headers, "content-length")?.parse().ok()?;
    let declared = fetch.blob.size.and_then(|size| u64::try_from(size).ok());

    // The event committed to the size; a peer claiming a different one is not
    // offering the file the event references.
    if total == 0 || declared.is_some_and(|size| size != total) {
        return None;
    }

    Some(total)
}

/// A header's value, matched the way HTTP matches header names.
fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
}

/// A `content-range` value: which bytes these are, over the length of the
/// whole blob. RFC 7233 wants the complete length after the slash — `*` where
/// this device does not know it — never the end of the range.
fn content_range(start: u64, last: u64, total: Option<u64>) -> String {
    match total {
        Some(total) => format!("bytes {start}-{last}/{total}"),
        None => format!("bytes {start}-{last}/*"),
    }
}

/// What the request's `range` header asks for, or `None` if it is malformed.
///
/// No header is a request for the whole blob rather than a range of it, and an
/// open-ended `bytes=N-` is the ordinary way to ask for the rest.
fn requested_range(request: &BlossomRequest) -> Option<Requested> {
    let Some(value) = header(&request.headers, "range") else {
        return Some(Requested::Whole);
    };

    let spec = value.strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    let end = end.trim();

    Some(Requested::Range {
        start: start.trim().parse().ok()?,
        end: if end.is_empty() {
            None
        } else {
            Some(end.parse().ok()?)
        },
    })
}

/// The blob a request path names, if this device knows it and the peer may be
/// served its anchor.
fn offerable_blob(db: &Db, peer: &Peer, local: &Identity, hash: &BlobHash) -> Result<Option<Blob>> {
    let Some(blob) = db_query::get_blob(db, hash)? else {
        return Ok(None);
    };

    // The anchoring event's permissions are the blob's: a peer that could not
    // be served the event cannot fetch its media. The same query the relay
    // half builds, so one place decides what a peer may see — a blocked
    // binding included.
    let query = relay::query_for(peer, local, Filter::new().add_ids([blob.event_id]));
    let anchor = db_query::list_events(db, &query)?;

    Ok(anchor
        .iter()
        .any(|event| event.id == blob.event_id)
        .then_some(blob))
}

/// A bodyless answer carrying only its status.
fn status(id: &str, status: u16) -> BlossomResponse {
    BlossomResponse {
        id: id.to_string(),
        status,
        headers: Vec::new(),
        body: Vec::new(),
    }
}

/// The 404 every miss looks like.
fn missing(id: &str) -> BlossomResponse {
    status(id, 404)
}

/// Whether a whole blob verifies against the `x` hash its record names.
fn verifies(blob: &Blob, store: &dyn BlobStore) -> Result<bool> {
    let Some(len) = store.len(&blob.sha256)? else {
        return Ok(false);
    };

    let bytes = store.read(&blob.sha256, 0, len)?;

    Ok(BlobHash::digest(&bytes) == blob.sha256)
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::tags::Tags;

    use crate::blobs::MemoryBlobStore;
    use crate::db::{Db, command as db_command};
    use crate::fixtures::{author, note};
    use crate::link::LinkId;
    use crate::model::Policy;

    fn local() -> Identity {
        Identity::from([author(1)])
    }

    fn peer() -> Peer {
        Peer::bind(LinkId(1), [author(2)], &Policy::new(author(1)))
    }

    /// A memory-backed exchange, returning the store too so tests can inspect
    /// what lands in it.
    fn exchange() -> (BlobExchange, Arc<MemoryBlobStore>) {
        let store = Arc::new(MemoryBlobStore::default());

        (BlobExchange::new(store.clone()), store)
    }

    /// A quota with room for `bytes` of blob, and the trusted budget for
    /// everything else.
    fn quota(bytes: u64) -> Quota {
        Quota {
            blob_bytes: bytes,
            ..Quota::TRUSTED
        }
    }

    /// An own event carrying an imeta tag records the blob row, and its bytes
    /// go into the store.
    fn given_blob(db: &Db, store: &MemoryBlobStore, bytes: &[u8]) -> Blob {
        let hash = BlobHash::digest(bytes);
        let event = note(
            author(1),
            1,
            "with a blob",
            Tags::new().add(
                "imeta",
                [
                    format!("x {hash}"),
                    format!("size {}", bytes.len()),
                    "m image/jpeg".to_string(),
                ],
            ),
        );

        db_command::publish_event(db, &event, &author(1), 1).unwrap();
        store.append(&hash, bytes).unwrap();

        db_query::get_blob(db, &hash).unwrap().unwrap()
    }

    /// Publish an event wanting a blob over `bytes`, returning its hash.
    fn given_wanted(db: &Db, bytes: &[u8]) -> BlobHash {
        given_wanted_sized(db, &BlobHash::digest(bytes), bytes.len() as u64)
    }

    /// Publish an event wanting `hash`, declaring `size` for it — which is
    /// what the fetcher holds the peer's answers to.
    fn given_wanted_sized(db: &Db, hash: &BlobHash, size: u64) -> BlobHash {
        let event = note(
            author(1),
            1,
            "with a blob",
            Tags::new().add("imeta", [format!("x {hash}"), format!("size {size}")]),
        );

        db_command::publish_event(db, &event, &author(1), 1).unwrap();

        hash.clone()
    }

    /// One request the peer makes of us.
    fn ask(method: &str, path: &str, headers: &[(&str, &str)]) -> BlossomRequest {
        BlossomRequest {
            id: format!("{method}-1"),
            method: method.to_string(),
            path: path.to_string(),
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
            body: Vec::new(),
        }
    }

    /// One answer to the request `id`.
    fn answer(id: &str, status: u16, headers: &[(&str, &str)], body: &[u8]) -> BlossomResponse {
        BlossomResponse {
            id: id.to_string(),
            status,
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
            body: body.to_vec(),
        }
    }

    /// Run one whole fetch — HEAD, then GETs until the body is in — against
    /// `blobs`, returning the blob's hash.
    fn drive_fetch(blobs: &mut BlobExchange, db: &Db, bytes: &[u8]) -> BlobHash {
        let hash = given_wanted(db, bytes);
        let quota = quota(Quota::TRUSTED.blob_bytes);

        // The fetch begins with a HEAD.
        let head = blobs.poll(db, quota).unwrap().expect("a HEAD to probe");
        assert_eq!(head.method, "HEAD");

        // The HEAD answers with the length, and a GET follows.
        let mut next = blobs
            .on_response(
                db,
                &peer(),
                &answer(
                    &head.id,
                    200,
                    &[("content-length", &bytes.len().to_string())],
                    &[],
                ),
                quota,
            )
            .unwrap();

        // Each GET is answered with exactly the group it asked for.
        while let Some(get) = next {
            assert_eq!(get.method, "GET");

            let (start, end) = match requested_range(&get).unwrap() {
                Requested::Range { start, end } => (start, end.unwrap()),
                Requested::Whole => panic!("the fetcher asked for no range"),
            };
            let group = &bytes[start as usize..=(end as usize).min(bytes.len() - 1)];

            next = blobs
                .on_response(db, &peer(), &answer(&get.id, 206, &[], group), quota)
                .unwrap();
        }

        hash
    }

    #[test]
    fn a_peer_can_head_and_get_a_blob_it_is_offered() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let bytes = b"the quick brown fox";
        let blob = given_blob(&db, &store, bytes);
        let path = format!("/{}", blob.sha256);

        let head = blobs
            .serve(
                &db,
                &peer(),
                &local(),
                &ask("HEAD", &path, &[]),
                quota(1024),
            )
            .unwrap();

        // Whole, so a 200 — and the content type the event declared, which is
        // what a HEAD is for as much as the length.
        assert_eq!(head.status, 200);
        assert_eq!(header(&head.headers, "content-length"), Some("19"));
        assert_eq!(header(&head.headers, "content-type"), Some("image/jpeg"));
        assert_eq!(header(&head.headers, "accept-ranges"), Some("bytes"));

        let get = blobs
            .serve(
                &db,
                &peer(),
                &local(),
                &ask("GET", &path, &[("range", "bytes=4-14")]),
                quota(1024),
            )
            .unwrap();

        assert_eq!(get.status, 206);
        assert_eq!(get.body, b"quick brown" as &[u8]);
        // The complete length after the slash, never the end of the range.
        assert_eq!(header(&get.headers, "content-range"), Some("bytes 4-14/19"));
        assert_eq!(header(&get.headers, "content-length"), Some("11"));
    }

    #[test]
    fn a_rangeless_get_is_a_200_over_the_whole_blob() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let bytes = b"the quick brown fox";
        let blob = given_blob(&db, &store, bytes);

        let get = blobs
            .serve(
                &db,
                &peer(),
                &local(),
                &ask("GET", &format!("/{}", blob.sha256), &[]),
                quota(1024),
            )
            .unwrap();

        // A rangeless GET is not a partial response, whatever the body is.
        assert_eq!(get.status, 200);
        assert_eq!(get.body, bytes);
        assert_eq!(header(&get.headers, "content-range"), None);
    }

    #[test]
    fn an_open_ended_range_is_the_rest_of_the_blob() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let blob = given_blob(&db, &store, b"the quick brown fox");

        let get = blobs
            .serve(
                &db,
                &peer(),
                &local(),
                &ask(
                    "GET",
                    &format!("/{}", blob.sha256),
                    &[("range", "bytes=4-")],
                ),
                quota(1024),
            )
            .unwrap();

        assert_eq!(get.status, 206);
        assert_eq!(get.body, b"quick brown fox" as &[u8]);
        assert_eq!(header(&get.headers, "content-range"), Some("bytes 4-18/19"));

        // A range that starts past the end is unsatisfiable, not a miss.
        let past = blobs
            .serve(
                &db,
                &peer(),
                &local(),
                &ask(
                    "GET",
                    &format!("/{}", blob.sha256),
                    &[("range", "bytes=99-200")],
                ),
                quota(1024),
            )
            .unwrap();

        assert_eq!(past.status, 416);

        // A range that is not bytes at all is the peer's error.
        let malformed = blobs
            .serve(
                &db,
                &peer(),
                &local(),
                &ask(
                    "GET",
                    &format!("/{}", blob.sha256),
                    &[("range", "items=1-2")],
                ),
                quota(1024),
            )
            .unwrap();

        assert_eq!(malformed.status, 400);
    }

    #[test]
    fn a_partial_holding_is_served_as_the_range_it_is() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();

        // A 2 MiB blob this device holds 4 KiB of: reporting 4 KiB as the
        // length would have the fetcher stop early and fail verification.
        let hash = BlobHash::digest(b"a big file");
        given_wanted_sized(&db, &hash, 2 * 1024 * 1024);
        store.append(&hash, &vec![7u8; 4096]).unwrap();

        let head = blobs
            .serve(
                &db,
                &peer(),
                &local(),
                &ask("HEAD", &format!("/{hash}"), &[]),
                quota(1024 * 1024),
            )
            .unwrap();

        assert_eq!(head.status, 206);
        assert_eq!(header(&head.headers, "content-length"), Some("4096"));
        assert_eq!(
            header(&head.headers, "content-range"),
            Some("bytes 0-4095/2097152")
        );
    }

    #[test]
    fn a_peer_that_holds_only_part_is_not_fetched_from() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(1024 * 1024);

        given_wanted_sized(&db, &BlobHash::digest(b"a big file"), 2 * 1024 * 1024);

        let head = blobs.poll(&db, quota).unwrap().unwrap();
        let next = blobs
            .on_response(
                &db,
                &peer(),
                &answer(
                    &head.id,
                    206,
                    &[
                        ("content-length", "4096"),
                        ("content-range", "bytes 0-4095/2097152"),
                    ],
                    &[],
                ),
                quota,
            )
            .unwrap();

        // A prefix cannot be verified or resumed, so it is not started — and
        // the peer is not asked for it again this session.
        assert!(next.is_none());
        assert!(blobs.poll(&db, quota).unwrap().is_none());
    }

    #[test]
    fn an_unknown_hash_is_a_404() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();

        let response = blobs
            .serve(
                &db,
                &peer(),
                &local(),
                &ask("HEAD", &format!("/{}", "ab".repeat(32)), &[]),
                quota(1024),
            )
            .unwrap();

        assert_eq!(response.status, 404);
    }

    #[test]
    fn a_path_that_is_not_a_hash_is_a_404_rather_than_an_error() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();

        // A peer choosing the path cannot end the session by choosing a bad
        // one, and cannot name a file outside the store either.
        for path in ["/ab", "/../../etc/passwd", "/", "/not-hex"] {
            let response = blobs
                .serve(&db, &peer(), &local(), &ask("HEAD", path, &[]), quota(1024))
                .unwrap();

            assert_eq!(response.status, 404, "{path} was not a miss");
        }
    }

    #[test]
    fn a_stranger_is_served_no_blob_bytes_and_fetches_none() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let blob = given_blob(&db, &store, b"the quick brown fox");

        // `Quota::STRANGER` carries no blob budget, and it is this refusal
        // that makes that mean something. `docs/sync.md#quotas-1`.
        let head = blobs
            .serve(
                &db,
                &peer(),
                &local(),
                &ask("HEAD", &format!("/{}", blob.sha256), &[]),
                Quota::STRANGER,
            )
            .unwrap();

        assert_eq!(head.status, 429);

        given_wanted(&db, b"something wanted");
        assert!(blobs.poll(&db, Quota::STRANGER).unwrap().is_none());
    }

    #[test]
    fn the_serving_budget_bounds_what_one_peer_can_take() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let blob = given_blob(&db, &store, b"the quick brown fox");
        let path = format!("/{}", blob.sha256);
        let quota = quota(10);

        // Ten bytes of budget: the answer is clipped to it rather than
        // refused, and what follows is refused rather than clipped.
        let first = blobs
            .serve(&db, &peer(), &local(), &ask("GET", &path, &[]), quota)
            .unwrap();

        assert_eq!(first.status, 206);
        assert_eq!(first.body, b"the quick " as &[u8]);
        assert_eq!(
            header(&first.headers, "content-range"),
            Some("bytes 0-9/19")
        );

        let second = blobs
            .serve(&db, &peer(), &local(), &ask("GET", &path, &[]), quota)
            .unwrap();

        assert_eq!(second.status, 429);
        assert!(second.body.is_empty());
    }

    #[test]
    fn verifies_the_whole_file_against_its_hash() {
        let db = Db::open_in_memory().unwrap();
        let store = MemoryBlobStore::default();
        let bytes = b"verified bytes";
        let blob = given_blob(&db, &store, bytes);

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
    }

    #[test]
    fn a_blob_fetch_writes_a_real_file() {
        use crate::blobs::FileBlobStore;
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
        assert!(dir.join(hash.as_str()).is_file());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_battery_floor_gates_fetches() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(Quota::TRUSTED.blob_bytes);

        given_wanted(&db, b"below the floor");

        blobs.set_battery(Some(BLOB_MIN_BATTERY - 1));
        assert!(
            blobs.poll(&db, quota).unwrap().is_none(),
            "a low battery fetches nothing"
        );

        blobs.set_battery(Some(BLOB_MIN_BATTERY));
        assert!(blobs.poll(&db, quota).unwrap().is_some());
    }

    #[test]
    fn a_missing_blob_is_not_asked_of_the_same_peer_again() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(Quota::TRUSTED.blob_bytes);

        given_wanted(&db, b"nobody holds this");

        let head = blobs.poll(&db, quota).unwrap().unwrap();
        let next = blobs
            .on_response(&db, &peer(), &answer(&head.id, 404, &[], &[]), quota)
            .unwrap();

        // Nothing further is asked of this peer. The blob is still wanted —
        // another encounter may hold it — but asking this one again the moment
        // the slot frees is a request loop, not a retry.
        assert!(next.is_none());
        assert!(blobs.poll(&db, quota).unwrap().is_none());
        assert_eq!(db_query::wanted_blobs(&db, 10).unwrap().len(), 1);
    }

    #[test]
    fn the_want_list_moves_past_a_hash_the_peer_missed() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(Quota::TRUSTED.blob_bytes);

        let first = given_wanted(&db, b"the first one");
        let second = given_wanted(&db, b"the second one");

        let head = blobs.poll(&db, quota).unwrap().unwrap();
        blobs
            .on_response(&db, &peer(), &answer(&head.id, 404, &[], &[]), quota)
            .unwrap();

        // The next poll asks for the other blob rather than the same one.
        let next = blobs.poll(&db, quota).unwrap().unwrap();
        let asked = [first, second]
            .into_iter()
            .find(|hash| next.path == format!("/{hash}"))
            .expect("a wanted blob");

        assert_ne!(next.path, head.path);
        assert!(next.path.ends_with(asked.as_str()));
    }

    #[test]
    fn bytes_that_fail_their_hash_are_dropped_and_the_peer_skipped() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let quota = quota(Quota::TRUSTED.blob_bytes);
        let bytes = b"the quick brown fox";
        let hash = given_wanted(&db, bytes);

        let head = blobs.poll(&db, quota).unwrap().unwrap();
        let get = blobs
            .on_response(
                &db,
                &peer(),
                &answer(
                    &head.id,
                    200,
                    &[("content-length", &bytes.len().to_string())],
                    &[],
                ),
                quota,
            )
            .unwrap()
            .unwrap();

        // The right number of bytes, the wrong bytes.
        let next = blobs
            .on_response(
                &db,
                &peer(),
                &answer(&get.id, 206, &[], b"the quick brown FOX"),
                quota,
            )
            .unwrap();

        assert!(next.is_none());
        assert!(!db_query::get_blob(&db, &hash).unwrap().unwrap().complete);
        // The bytes go, the record agrees they are gone, and this peer is not
        // asked for the blob again — it would send the same bytes.
        assert_eq!(store.len(&hash).unwrap(), None);
        assert_eq!(
            db_query::get_blob(&db, &hash)
                .unwrap()
                .unwrap()
                .stored_bytes,
            0
        );
        assert!(blobs.poll(&db, quota).unwrap().is_none());
    }

    #[test]
    fn a_body_longer_than_the_request_is_refused() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let quota = quota(Quota::TRUSTED.blob_bytes);
        let hash = given_wanted_sized(&db, &BlobHash::digest(b"small"), 8);

        let head = blobs.poll(&db, quota).unwrap().unwrap();
        let get = blobs
            .on_response(
                &db,
                &peer(),
                &answer(&head.id, 200, &[("content-length", "8")], &[]),
                quota,
            )
            .unwrap()
            .unwrap();

        // Eight bytes were asked for and declared; a megabyte arrives.
        let next = blobs
            .on_response(
                &db,
                &peer(),
                &answer(&get.id, 206, &[], &vec![0u8; 1024 * 1024]),
                quota,
            )
            .unwrap();

        assert!(next.is_none());
        assert_eq!(store.len(&hash).unwrap(), None);
    }

    #[test]
    fn a_peer_disagreeing_with_the_declared_size_is_not_fetched_from() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(Quota::TRUSTED.blob_bytes);

        given_wanted_sized(&db, &BlobHash::digest(b"small"), 8);

        let head = blobs.poll(&db, quota).unwrap().unwrap();
        let next = blobs
            .on_response(
                &db,
                &peer(),
                &answer(&head.id, 200, &[("content-length", "1048576")], &[]),
                quota,
            )
            .unwrap();

        // The event id commits to the size in `imeta`, so a peer offering
        // another length is offering another file.
        assert!(next.is_none());
        assert!(blobs.poll(&db, quota).unwrap().is_none());
    }

    #[test]
    fn a_blob_larger_than_the_budget_is_not_started() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(1024);

        given_wanted_sized(&db, &BlobHash::digest(b"a big file"), 64 * 1024);

        let head = blobs.poll(&db, quota).unwrap().unwrap();
        let next = blobs
            .on_response(
                &db,
                &peer(),
                &answer(&head.id, 200, &[("content-length", "65536")], &[]),
                quota,
            )
            .unwrap();

        assert!(next.is_none());
    }

    #[test]
    fn the_fetch_budget_stops_a_transfer_it_cannot_finish() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(BLOB_GROUP_BYTES);
        let bytes = vec![3u8; 2 * BLOB_GROUP_BYTES as usize];

        given_wanted(&db, &bytes);

        let head = blobs.poll(&db, quota).unwrap().unwrap();
        let next = blobs
            .on_response(
                &db,
                &peer(),
                &answer(
                    &head.id,
                    200,
                    &[("content-length", &bytes.len().to_string())],
                    &[],
                ),
                quota,
            )
            .unwrap();

        // Two groups wanted, one group of budget: it is not begun at all.
        assert!(next.is_none());
        assert!(blobs.poll(&db, quota).unwrap().is_none());
    }

    #[test]
    fn the_bulk_channel_is_asked_for_once_and_only_when_it_pays() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(Quota::TRUSTED.blob_bytes);

        // A small blob moves in a couple of round trips; opening a channel for
        // it costs more than it saves.
        drive_fetch(&mut blobs, &db, b"the quick brown fox");
        assert!(!blobs.take_l2cap_request());

        // One over the threshold does pay, and asks exactly once.
        given_wanted_sized(&db, &BlobHash::digest(b"a big file"), L2CAP_THRESHOLD_BYTES);

        let head = blobs.poll(&db, quota).unwrap().unwrap();
        blobs
            .on_response(
                &db,
                &peer(),
                &answer(
                    &head.id,
                    200,
                    &[("content-length", &L2CAP_THRESHOLD_BYTES.to_string())],
                    &[],
                ),
                quota,
            )
            .unwrap();

        assert!(blobs.take_l2cap_request());
        assert!(!blobs.take_l2cap_request());
    }

    #[test]
    fn a_refusal_ends_the_session_s_fetching() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(Quota::TRUSTED.blob_bytes);

        given_wanted(&db, b"the first one");
        given_wanted(&db, b"the second one");

        let head = blobs.poll(&db, quota).unwrap().unwrap();
        blobs
            .on_response(&db, &peer(), &answer(&head.id, 429, &[], &[]), quota)
            .unwrap();

        // The peer's budget is spent, so no other hash will fare better.
        assert!(blobs.poll(&db, quota).unwrap().is_none());
    }

    /// The reproducer: one gossiped event carrying `["imeta", "x ab"]`.
    ///
    /// Stored as it arrived, the row sat on the want list forever and every
    /// later poll panicked taking a prefix of it — which killed sync on the
    /// device, not just blobs.
    #[test]
    fn a_malformed_hash_from_a_peer_never_reaches_the_store() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(Quota::TRUSTED.blob_bytes);

        for bad in ["ab", "", "not-hex", &"ab".repeat(31), &"zz".repeat(32)] {
            let event = note(
                author(2),
                1,
                "gossiped",
                Tags::new().add("imeta", [format!("x {bad}"), "size 19".to_string()]),
            );

            db_command::receive_event(&db, &event, &[author(2)], 1).unwrap();
        }

        assert!(db_query::wanted_blobs(&db, 10).unwrap().is_empty());
        assert!(blobs.poll(&db, quota).unwrap().is_none());
    }

    /// An uppercase hash is stored lowercase, so the row a peer's event writes
    /// is the row this device can read back, complete and evict.
    #[test]
    fn an_uppercase_hash_from_a_peer_is_stored_the_way_it_is_read() {
        let db = Db::open_in_memory().unwrap();
        let hash = BlobHash::digest(b"the quick brown fox");
        let event = note(
            author(2),
            1,
            "gossiped",
            Tags::new().add(
                "imeta",
                [
                    format!("x {}", hash.to_string().to_uppercase()),
                    "size 19".to_string(),
                ],
            ),
        );

        db_command::receive_event(&db, &event, &[author(2)], 1).unwrap();

        assert_eq!(db_query::wanted_blobs(&db, 10).unwrap()[0].sha256, hash);
        assert!(db_query::get_blob(&db, &hash).unwrap().is_some());
    }

    #[test]
    fn a_blob_a_peer_may_not_be_served_the_event_for_is_a_404() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let blob = given_blob(&db, &store, b"the quick brown fox");

        // Blocked is a veto over the whole set, exactly as on the event path,
        // because the query is the relay half's own. The blocked pubkey is not
        // the one that sorts first, so anything picking a binding by position
        // would have served the blob.
        let mut policy = Policy::new(author(1));
        policy.graph.blocked.insert(author(3));

        let blocked = Peer::bind(LinkId(1), [author(2), author(3)], &policy);
        assert!(blocked.is_blocked());
        let response = blobs
            .serve(
                &db,
                &blocked,
                &local(),
                &ask("HEAD", &format!("/{}", blob.sha256), &[]),
                quota(1024),
            )
            .unwrap();

        assert_eq!(response.status, 404);
    }
}
