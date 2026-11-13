//! Blob transfers: the binary query and fetch pair on the blob channel, as
//! [`BlobMessage`] encodes them.
//!
//! [`BlobExchange`] is one session's blob half, both directions at once. The
//! relay direction serves a peer's requests from the [`BlobStore`] — the
//! core's file-backed store by default — and the client direction pulls blobs
//! a stored event references and this device does not hold, one group of
//! bytes at a time. Each group is written as it arrives; the record is
//! completed once the whole file is held and hashes to its address.
//! `docs/sync.md#blob-sync`.
//!
//! Both directions are metered against the session's [`Quota`]: a transfer is
//! an unbounded write to the user's disk in one direction and an unbounded
//! read of their battery in the other.
//! `docs/sync.md#quotas-1`.
//!
//! A fetch is two stages: a query for the length and how much of it the peer
//! holds, and then a fetch per group. Where the blob's `imeta` names a BLAKE3
//! root, each group comes back as a Bao slice, and is checked against that
//! root before any of it reaches the disk. So a bad group costs one group and
//! names the peer, and what is on disk is proved, which is what lets the next
//! session resume from it rather than fetch it again. An answer cut short
//! keeps the chunks it proved, and the next fetch starts where they end. A
//! blob with no root has nothing to check a group against: those are fetched
//! whole from a peer holding the whole file, checked once against the `x`
//! SHA-256 at the end, and started over when a transfer drops.
//! [`blobs::verified`](crate::blobs::verified).

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{Result, bail};
use coracle_lib::filters::Filter;

use crate::blobs::{BlobStore, verified};
use crate::db::Db;
use crate::db::command;
use crate::db::query as db_query;
use crate::model::{Blob, BlobDigest, BlobHash, Identity};
use crate::session::Peer;
use crate::sync::Quota;
use crate::sync::blob_message::{BlobMessage, BlobStatus};
use crate::sync::relay;

/// The bytes one fetch asks for, and the bytes one slice proves.
///
/// Sixteen BLAKE3 chunks. A group is a whole number of them, and a slice over
/// one carries no chunk the fetch does not need. [`CHUNK_BYTES`] is
/// Bao's and is not ours to raise; this is ours, and is what one round trip
/// over BLE is worth.
///
/// [`CHUNK_BYTES`]: verified::CHUNK_BYTES
pub const GROUP_BYTES: u64 = 16 * verified::CHUNK_BYTES;

/// The battery floor, percent, below which blob transfers do not start.
/// A transfer starves the link for minutes; a low battery makes that worse.
pub const BLOB_MIN_BATTERY: u8 = 20;

/// Outstanding blob bytes at which a bulk channel earns its setup round trip.
///
/// Four groups: below that the transfer is over in fewer round trips than
/// opening the channel costs, and the shell is asked for nothing.
/// `docs/transport.md#the-l2cap-bandwidth-upgrade`.
pub const L2CAP_THRESHOLD_BYTES: u64 = 4 * GROUP_BYTES;

/// The most content one answer carries, however long a range it was asked.
///
/// NIP-BE caps a frame at 1 MiB, and a Bao slice carries its proof beside the
/// bytes it proves. Half the cap leaves room for any proof, and is a whole
/// number of chunks, so that a proved range cut here still ends on one.
pub const ANSWER_BYTES: u64 = 512 * verified::CHUNK_BYTES;

/// How much of a blob this device can answer for, and how much there is.
struct Held {
    /// Bytes that may be served, which is what is on disk bounded by `total`.
    available: u64,
    /// The whole blob's length, once anything says what it is.
    total: Option<u64>,
    /// Whether a range can be proved, which takes the tree over a whole copy.
    provable: bool,
}

/// One in-flight fetch of one blob.
#[derive(Debug)]
struct BlobFetch {
    /// What is being fetched, metadata and all.
    blob: Blob,
    /// The blob's length once the query has answered, if it has.
    total: Option<u64>,
    /// How much of it this peer holds, which a partial holding caps.
    available: u64,
    /// Verified bytes held so far, which is where the next fetch resumes.
    stored: u64,
    /// How many bytes the outstanding fetch asked for, which bounds its answer.
    requested: u64,
    /// Correlates the outstanding request with its answer.
    request_id: u16,
    /// Whether the outstanding request is still the query.
    probing: bool,
    /// The BLAKE3 root the anchoring event committed to, when it named one.
    /// Its presence is what makes every group of this fetch a proved one.
    root: Option<verified::Root>,
}

impl BlobFetch {
    /// A fetch picking `blob` up from the `stored` bytes already proved, asked
    /// for first under `request_id`.
    fn begin(blob: Blob, stored: u64, request_id: u16) -> Self {
        let root = declared_root(&blob);

        Self {
            blob,
            total: None,
            available: 0,
            stored,
            requested: 0,
            request_id,
            probing: true,
            root,
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
    /// Whether it has been asked for at all. There is one request per link,
    /// because the channel outlives a group and re-arming per group would ask
    /// again every few seconds of a large transfer.
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
    /// The id the next request goes out under. Every request has its own, so
    /// that an answer to one this device gave up on is not taken for another.
    next_id: u16,
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
            next_id: 0,
        }
    }

    /// The id for the next request, wrapping where it runs out.
    fn take_id(&mut self) -> u16 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        id
    }

    /// Whether a transfer is in flight, waiting on the peer's next answer.
    #[must_use]
    pub fn is_fetching(&self) -> bool {
        self.active.is_some()
    }

    /// Blob bytes taken from the peer this session, starting from what the
    /// rolling window had already charged it.
    #[must_use]
    pub fn fetched_bytes(&self) -> u64 {
        self.fetched_bytes
    }

    /// Start this session's count from what the window already charged the
    /// peer, which keeps reconnecting from refilling anything. `docs/sync.md#quotas`.
    pub fn carry_fetched(&mut self, bytes: u64) {
        self.fetched_bytes = self.fetched_bytes.max(bytes);
    }

    /// Blob bytes served to the peer this session, starting from what the
    /// rolling window had already served it.
    #[must_use]
    pub fn served_bytes(&self) -> u64 {
        self.served_bytes
    }

    /// Start this session's serving count from the window, for the same reason.
    pub fn carry_served(&mut self, bytes: u64) {
        self.served_bytes = self.served_bytes.max(bytes);
    }

    /// Ask for `hash` again although a peer could not serve it earlier, since a
    /// new reference to it may be one the peer can.
    pub fn reconsider(&mut self, hash: &BlobHash) {
        self.unavailable.remove(hash);
    }

    /// Give up the fetch in flight, whose request or answer was lost with the
    /// pipe it rode. The next [`poll`](Self::poll) resumes from the store.
    pub fn abandon_in_flight(&mut self) {
        self.active = None;
    }

    /// Ask again although the peer refused earlier, because standing can
    /// change mid-session: a peer that refused this device as a stranger may
    /// have named it since.
    pub fn forgive_refusal(&mut self) {
        self.refused = false;
    }

    /// The battery level in percent, which gates new transfers.
    pub fn set_battery(&mut self, level: Option<u8>) {
        self.battery = level;
    }

    /// Whether the shell should be told to open L2CAP for this link, once.
    pub fn take_l2cap_request(&mut self) -> bool {
        std::mem::take(&mut self.wants_l2cap)
    }

    /// Answer one of the peer's requests against the store.
    ///
    /// A hash the peer could not be offered the anchoring event for is not
    /// served like any other miss: it does not exist to them, and the answer
    /// does not say which. A peer that has spent its blob budget is told so,
    /// which does say which — it is the answer that stops it asking.
    pub fn serve(
        &mut self,
        db: &Db,
        peer: &Peer,
        local: &Identity,
        request: &BlobMessage,
        quota: Quota,
    ) -> Result<BlobMessage> {
        let (id, hash) = match request {
            BlobMessage::Query { id, hash } | BlobMessage::Fetch { id, hash, .. } => (*id, hash),
            other => bail!("{other:?} is an answer, not a request"),
        };

        let budget = quota.blob_bytes.saturating_sub(self.served_bytes);

        // This also tells a stranger, whose blob budget is zero, that there are no bytes here.
        if budget == 0 {
            return Ok(refusal(request, BlobStatus::BudgetSpent));
        }

        let Some(blob) = offerable_blob(db, peer, local, hash)? else {
            return Ok(refusal(request, BlobStatus::NotServed));
        };

        let Some(on_disk) = self.store.len(&blob.sha256)? else {
            log::debug!("blob {hash} is recorded and offerable but no bytes are held");
            return Ok(refusal(request, BlobStatus::NotServed));
        };

        // The event id commits to the whole file's length; what is served is what is on disk.
        let total = blob
            .declared_size()
            .or_else(|| blob.complete.then_some(on_disk));
        let held = Held {
            available: total.map_or(on_disk, |total| on_disk.min(total)),
            total,
            // A prefix serves raw, because a range is proved out of the outboard over the whole file.
            provable: self.store.outboard_len(&blob.sha256)?.is_some(),
        };

        if held.available == 0 {
            log::debug!("blob {hash} has {on_disk} bytes held and none available to serve");
            return Ok(refusal(request, BlobStatus::NotServed));
        }

        match *request {
            BlobMessage::Fetch {
                offset,
                length,
                proof,
                ..
            } => self.fetch(id, &blob, &held, (offset, length, proof), budget),
            _ => Ok(BlobMessage::Holding {
                id,
                status: BlobStatus::Ok,
                total: held.total,
                held: held.available,
                provable: held.provable,
            }),
        }
    }

    /// Answer a fetch of `offset`, `length` and whether to prove it, up to
    /// what the peer's budget allows.
    fn fetch(
        &mut self,
        id: u16,
        blob: &Blob,
        held: &Held,
        (offset, length, proof): (u64, u32, bool),
        budget: u64,
    ) -> Result<BlobMessage> {
        let answer = |status, body| BlobMessage::Bytes { id, status, body };

        // An empty range asks for nothing, and one past the end for nothing here.
        if length == 0 || offset >= held.available {
            return Ok(answer(BlobStatus::OutOfRange, Vec::new()));
        }

        let last = offset
            .saturating_add(u64::from(length) - 1)
            .min(held.available - 1);
        // A range one frame cannot hold is cut to what one can, the way the budget cuts it.
        let len = (last - offset + 1).min(budget).min(ANSWER_BYTES);

        if proof {
            if !held.provable {
                return Ok(answer(BlobStatus::CannotProve, Vec::new()));
            }

            // The answer is the subtree hashes on the path to the range, then its bytes.
            let body = verified::slice(self.store.as_ref(), &blob.sha256, offset, len)?;

            // The budget covers the proof too, because it rides the same radio as the bytes.
            self.served(body.len() as u64);

            return Ok(answer(BlobStatus::Ok, body));
        }

        let bytes = self.store.read(&blob.sha256, offset, len)?;

        if bytes.is_empty() {
            return Ok(answer(BlobStatus::NotServed, Vec::new()));
        }

        self.served(bytes.len() as u64);

        Ok(answer(BlobStatus::Ok, bytes))
    }

    /// Note bytes going out to the peer, as a spend against the peer's budget.
    ///
    /// A proof is charged for like the bytes it proves, because both answers
    /// pay for what went on the radio rather than for the content they carry.
    fn served(&mut self, bytes: u64) {
        self.served_bytes = self.served_bytes.saturating_add(bytes);
    }

    /// Start the next fetch if none is in flight, returning the query that
    /// probes it before any bytes move.
    pub fn poll(&mut self, db: &Db, quota: Quota) -> Result<Option<BlobMessage>> {
        if self.active.is_some() || self.refused {
            return Ok(None);
        }

        // Low battery skips the transfer; it is the one thing a fetch waits for on a phone.
        if self.battery.is_some_and(|level| level < BLOB_MIN_BATTERY) {
            log::debug!("not fetching blobs on a battery at {:?}", self.battery);
            return Ok(None);
        }

        if self.fetched_bytes >= quota.blob_bytes {
            log::debug!(
                "not fetching blobs past a budget of {} bytes",
                quota.blob_bytes
            );
            return Ok(None);
        }

        let Some(wanted) = self.next_wanted(db)? else {
            return Ok(None);
        };

        let stored = self.resume_from(db, &wanted)?;
        let id = self.take_id();
        let query = BlobMessage::Query {
            id,
            hash: wanted.sha256.clone(),
        };

        self.active = Some(BlobFetch::begin(wanted, stored, id));

        Ok(Some(query))
    }

    /// Where a fetch of `blob` picks up: the bytes already on disk, verified.
    ///
    /// Bytes fetched with no root to check them against were never verified
    /// group by group, and a store that disagrees with the record cannot be
    /// resumed into either. Both start over rather than corrupt a re-fetch.
    fn resume_from(&self, db: &Db, blob: &Blob) -> Result<u64> {
        let recorded = blob.stored();
        let held = self.store.len(&blob.sha256)?.unwrap_or(0);
        let whole_chunks =
            recorded % verified::CHUNK_BYTES == 0 || blob.declared_size() == Some(recorded);

        if declared_root(blob).is_some() && recorded == held && whole_chunks {
            return Ok(recorded);
        }

        if held > 0 {
            self.store.delete(&blob.sha256)?;
            command::record_blob_progress(db, &blob.sha256, 0)?;
        }

        Ok(0)
    }

    /// The next wanted blob this peer has not already failed on.
    fn next_wanted(&self, db: &Db) -> Result<Option<Blob>> {
        // One row further down the want list per hash already skipped.
        let limit = self.unavailable.len().saturating_add(1);

        Ok(db_query::wanted_blobs(db, limit)?
            .into_iter()
            .find(|blob| !self.unavailable.contains(&blob.sha256)))
    }

    /// Advance the in-flight fetch on one answer, returning the next request
    /// if the fetch continues. An answer that ends the fetch — done, missing,
    /// or refused — frees the slot for [`poll`](Self::poll).
    pub fn on_response(
        &mut self,
        db: &Db,
        peer: &Peer,
        response: &BlobMessage,
        quota: Quota,
    ) -> Result<Option<BlobMessage>> {
        let Some(mut fetch) = self.active.take() else {
            return Ok(None);
        };

        if fetch.request_id != response.id() {
            self.active = Some(fetch);
            return Ok(None);
        }

        // Nothing else asked this session is answered once the peer has no blob budget left.
        if response.status() == Some(BlobStatus::BudgetSpent) {
            self.refused = true;
            return Ok(None);
        }

        let advanced = match fetch.probing {
            true => self.on_probe(peer, &mut fetch, response, quota)?,
            false => self.on_group(db, peer, &mut fetch, response, quota)?,
        };

        if !advanced {
            return Ok(None);
        }

        if fetch.total.is_some_and(|total| fetch.stored >= total) {
            return self.finish(db, peer, &fetch).map(|()| None);
        }

        // What this peer or session has no more of waits for the next; the bytes stay on disk.
        let Some(next) = self.next_request(&mut fetch, quota) else {
            self.unavailable.insert(fetch.blob.sha256);
            return Ok(None);
        };

        self.active = Some(fetch);

        Ok(Some(next))
    }

    /// Take the answer to the query: what the blob's length is, how much of it
    /// this peer holds, and whether it can prove what it sends.
    fn on_probe(
        &mut self,
        peer: &Peer,
        fetch: &mut BlobFetch,
        response: &BlobMessage,
        quota: Quota,
    ) -> Result<bool> {
        let &BlobMessage::Holding {
            status,
            total,
            held,
            provable,
            ..
        } = response
        else {
            self.give_up(
                peer,
                &fetch.blob.sha256,
                "answered a query with something else",
            );
            return Ok(false);
        };

        let Some((total, available)) = probed(fetch, status, total, held) else {
            self.give_up(peer, &fetch.blob.sha256, "cannot serve the blob");
            return Ok(false);
        };

        let outstanding = total.saturating_sub(fetch.stored);
        let resumable = fetch.root.is_some();

        // Only a device holding the whole file can prove any of it, which rules out a prefix.
        if resumable && !provable {
            self.give_up(peer, &fetch.blob.sha256, "cannot prove what it holds");
            return Ok(false);
        }

        // A blob past the budget is only started when the groups it does fetch survive.
        if !resumable && outstanding > quota.blob_bytes.saturating_sub(self.fetched_bytes) {
            self.give_up(peer, &fetch.blob.sha256, "is larger than the budget left");
            return Ok(false);
        }

        if available <= fetch.stored {
            self.give_up(
                peer,
                &fetch.blob.sha256,
                "holds no more of it than this device",
            );
            return Ok(false);
        }

        self.consider_l2cap(outstanding);

        fetch.total = Some(total);
        fetch.available = available;
        fetch.probing = false;

        Ok(true)
    }

    /// Take one group of bytes, proved against the root before it is written.
    fn on_group(
        &mut self,
        db: &Db,
        peer: &Peer,
        fetch: &mut BlobFetch,
        response: &BlobMessage,
        quota: Quota,
    ) -> Result<bool> {
        let BlobMessage::Bytes { status, body, .. } = response else {
            self.give_up(
                peer,
                &fetch.blob.sha256,
                "answered a fetch with something else",
            );
            return Ok(false);
        };

        if *status != BlobStatus::Ok {
            // The peer is not asked for this hash again this session, freed slot or not.
            self.give_up(peer, &fetch.blob.sha256, "does not hold it");
            return Ok(false);
        }

        let allowed = fetch
            .requested
            .min(fetch.total.unwrap_or(0).saturating_sub(fetch.stored))
            .min(quota.blob_bytes.saturating_sub(self.fetched_bytes));

        // A group that does not prove out costs that group and the peer; the rest stands.
        let Some(content) = content_of(fetch, body) else {
            self.give_up(
                peer,
                &fetch.blob.sha256,
                "sent a group that did not prove out",
            );
            return Ok(false);
        };

        // More than asked for, declared or allowed: writing it is an unbounded write to disk.
        if content.is_empty() || content.len() as u64 > allowed {
            self.give_up(
                peer,
                &fetch.blob.sha256,
                "answered a range it was not asked",
            );
            return Ok(false);
        }

        self.store.append(&fetch.blob.sha256, &content)?;
        fetch.stored += content.len() as u64;

        // The budget covers the proof too, because it rides the same radio as the bytes.
        self.fetched_bytes = self.fetched_bytes.saturating_add(body.len() as u64);

        command::record_blob_progress(db, &fetch.blob.sha256, fetch.stored)?;

        Ok(true)
    }

    /// The next request of the fetch, or `None` when there is nothing this
    /// peer can still be asked for inside what is left of the budget.
    fn next_request(&mut self, fetch: &mut BlobFetch, quota: Quota) -> Option<BlobMessage> {
        let total = fetch.total?;
        let wanted = total.saturating_sub(fetch.stored).min(GROUP_BYTES);

        // A group at a time, of what this peer holds and the budget still covers.
        if wanted == 0
            || fetch.stored + wanted > fetch.available
            || wanted > quota.blob_bytes.saturating_sub(self.fetched_bytes)
        {
            return None;
        }

        fetch.requested = wanted;
        fetch.request_id = self.take_id();

        // Every start lands on a chunk boundary, because groups are a whole number of chunks.
        Some(BlobMessage::Fetch {
            id: fetch.request_id,
            hash: fetch.blob.sha256.clone(),
            offset: fetch.stored,
            length: u32::try_from(wanted).ok()?,
            proof: fetch.root.is_some(),
        })
    }

    /// Mark a fetch whole after verifying the assembled bytes.
    fn finish(&mut self, db: &Db, peer: &Peer, fetch: &BlobFetch) -> Result<()> {
        if self.assembled(fetch)? {
            log::debug!(
                "blob {} complete at {} bytes",
                fetch.blob.sha256,
                fetch.stored
            );
            command::complete_blob(db, &fetch.blob.sha256, fetch.stored)?;

            return Ok(());
        }

        // Bytes that do not hash to their address are deleted, and the peer is not asked again.
        self.store.delete(&fetch.blob.sha256)?;
        command::record_blob_progress(db, &fetch.blob.sha256, 0)?;
        self.give_up(
            peer,
            &fetch.blob.sha256,
            "sent bytes that failed their hash",
        );

        Ok(())
    }

    /// Whether the assembled bytes are the file the anchoring event named.
    ///
    /// The address is weighed here however the bytes were proved on the way
    /// in, because it is a SHA-256 over the whole file. A blob that
    /// named a root has to fold to that too, and building the tree that proves
    /// it is what leaves this device able to serve the blob on verified.
    fn assembled(&self, fetch: &BlobFetch) -> Result<bool> {
        let whole = verifies(&fetch.blob, self.store.as_ref())?;

        match (whole, fetch.root) {
            (false, _) => Ok(false),
            (true, None) => Ok(true),
            (true, Some(root)) => {
                Ok(verified::build(self.store.as_ref(), &fetch.blob.sha256)? == root)
            }
        }
    }

    /// Skip a hash for the rest of the session, naming the peer that earned it.
    fn give_up(&mut self, peer: &Peer, hash: &BlobHash, reason: &str) {
        // A bad answer names the peer that gave it. `docs/sync.md#blob-sync`.
        log::info!(
            "blob {hash} skipped for this session: peer {} {reason}",
            peer.pubkeys
                .iter()
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

/// The BLAKE3 root a blob's `imeta` committed to, when it named one that
/// parses.
///
/// A root that does not parse names nothing. A blob carrying one is fetched
/// as though the event had claimed none — unproved, and starting over rather
/// than resuming, which is the direction that trusts less.
fn declared_root(blob: &Blob) -> Option<verified::Root> {
    blob.blake3
        .as_deref()
        .and_then(|value| verified::root(value).ok())
}

/// The content bytes an answer carries: its body for an unproved transfer,
/// and what the slice yields for a proved one, which is fewer than asked for
/// when the answer was cut short.
///
/// `None` is a peer that answered a proved fetch with something other than a
/// slice over the range it was asked, which is the one answer a verified
/// transfer treats as a bad peer rather than as bytes.
fn content_of(fetch: &BlobFetch, body: &[u8]) -> Option<Vec<u8>> {
    let Some(root) = &fetch.root else {
        return Some(body.to_vec());
    };

    verified::verify(body, root, fetch.stored, fetch.requested).ok()
}

/// What a holding says the blob's whole length is and how much of it the
/// peer holds, or `None` when there is nothing here worth fetching from it.
///
/// A holding short of the total is a prefix. A prefix is only worth having if
/// each group of it can be checked as it arrives — and only a device holding
/// the whole file has the outboard tree that proves any of it. A rooted blob
/// is therefore fetched from a whole holding, and the prefix is refused at the
/// proof check. Otherwise the hash waits for a peer that holds the file.
fn probed(
    fetch: &BlobFetch,
    status: BlobStatus,
    total: Option<u64>,
    held: u64,
) -> Option<(u64, u64)> {
    if status != BlobStatus::Ok {
        return None;
    }

    let total = total?;

    if held < total && fetch.root.is_none() {
        return None;
    }

    // The event committed to the size; another length is another file.
    if total == 0 || held > total || fetch.blob.declared_size().is_some_and(|size| size != total) {
        return None;
    }

    Some((total, held))
}

/// The blob a request names, if the peer may be served every event that references it. `docs/sync.md#blob-sync`.
fn offerable_blob(db: &Db, peer: &Peer, local: &Identity, hash: &BlobHash) -> Result<Option<Blob>> {
    let Some(blob) = db_query::get_blob(db, hash)? else {
        log::debug!("blob {hash} has no record here");
        return Ok(None);
    };

    let referencing = db_query::events_referencing_blob(db, hash)?;
    let query = relay::query_for(peer, local, Filter::new().add_ids(referencing.clone()));
    let served = db_query::list_events(db, &query)?;
    let audience = db_query::list_events(db, &query.including_trash())?;
    let offerable = !served.is_empty() && audience.len() == referencing.len();

    if !offerable {
        log::debug!(
            "blob {hash} is referenced by {} events, the peer may be served {} and may see {}",
            referencing.len(),
            served.len(),
            audience.len()
        );
    }

    Ok(offerable.then_some(blob))
}

/// The answer to `request` that carries nothing but `status`.
fn refusal(request: &BlobMessage, status: BlobStatus) -> BlobMessage {
    match request {
        BlobMessage::Fetch { id, .. } => BlobMessage::Bytes {
            id: *id,
            status,
            body: Vec::new(),
        },
        other => BlobMessage::Holding {
            id: other.id(),
            status,
            total: None,
            held: 0,
            provable: false,
        },
    }
}

/// Whether a whole blob verifies against the `x` hash its record names.
///
/// Read a group at a time, the way the transfer wrote it: a blob is as large as
/// whatever the author attached, and holding a second copy of it in memory to
/// hash it is the sort of allocation a phone refuses at the worst moment.
fn verifies(blob: &Blob, store: &dyn BlobStore) -> Result<bool> {
    let Some(len) = store.len(&blob.sha256)? else {
        return Ok(false);
    };

    let mut digest = BlobDigest::new();
    let mut read = 0;

    while read < len {
        let group = store.read(&blob.sha256, read, GROUP_BYTES)?;

        // An empty read means the store is shorter than it just said it was.
        if group.is_empty() {
            return Ok(false);
        }

        read += group.len() as u64;
        digest.update(&group);
    }

    Ok(digest.finish() == blob.sha256)
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

    /// A quota with room for `bytes` of blob, and the contact budget for
    /// everything else.
    fn quota(bytes: u64) -> Quota {
        Quota {
            blob_bytes: bytes,
            ..Quota::CONTACT
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

    /// The peer asking how much of `hash` this device holds.
    fn query(hash: &BlobHash) -> BlobMessage {
        BlobMessage::Query {
            id: 1,
            hash: hash.clone(),
        }
    }

    /// The peer asking for `length` bytes of `hash` from `offset`.
    fn fetch(hash: &BlobHash, offset: u64, length: u32, proof: bool) -> BlobMessage {
        BlobMessage::Fetch {
            id: 1,
            hash: hash.clone(),
            offset,
            length,
            proof,
        }
    }

    /// A peer holding `held` bytes of a blob `total` long, answering `request`.
    fn holding(request: &BlobMessage, total: u64, held: u64) -> BlobMessage {
        BlobMessage::Holding {
            id: request.id(),
            status: BlobStatus::Ok,
            total: Some(total),
            held,
            provable: false,
        }
    }

    /// A peer answering `request` with `body`.
    fn bytes_for(request: &BlobMessage, body: &[u8]) -> BlobMessage {
        BlobMessage::Bytes {
            id: request.id(),
            status: BlobStatus::Ok,
            body: body.to_vec(),
        }
    }

    /// What a holding reports: how it went, the whole length, the prefix held,
    /// and whether a range can be proved.
    fn reported(answer: &BlobMessage) -> (BlobStatus, Option<u64>, u64, bool) {
        match *answer {
            BlobMessage::Holding {
                status,
                total,
                held,
                provable,
                ..
            } => (status, total, held, provable),
            ref other => panic!("expected a holding, got {other:?}"),
        }
    }

    /// What an answer to a fetch carries: how it went, and the bytes.
    fn carried(answer: &BlobMessage) -> (BlobStatus, &[u8]) {
        match answer {
            BlobMessage::Bytes { status, body, .. } => (*status, body),
            other => panic!("expected bytes, got {other:?}"),
        }
    }

    /// The range a fetch asks for, and whether it asks for a proof.
    fn range(request: &BlobMessage) -> (u64, u32, bool) {
        match *request {
            BlobMessage::Fetch {
                offset,
                length,
                proof,
                ..
            } => (offset, length, proof),
            ref other => panic!("expected a fetch, got {other:?}"),
        }
    }

    /// The blob a query asks about.
    fn queried(request: &BlobMessage) -> BlobHash {
        match request {
            BlobMessage::Query { hash, .. } => hash.clone(),
            other => panic!("expected a query, got {other:?}"),
        }
    }

    /// Run one whole fetch — a query, then fetches until the body is in —
    /// against `blobs`, returning the blob's hash.
    fn drive_fetch(blobs: &mut BlobExchange, db: &Db, bytes: &[u8]) -> BlobHash {
        let hash = given_wanted(db, bytes);
        let quota = quota(Quota::CONTACT.blob_bytes);
        let len = bytes.len() as u64;

        // The fetch begins with a query.
        let probe = blobs.poll(db, quota).unwrap().expect("a query to probe");
        assert_eq!(queried(&probe), hash);

        // The holding carries the length, and a fetch follows.
        let mut next = blobs
            .on_response(db, &peer(), &holding(&probe, len, len), quota)
            .unwrap();

        // Each fetch is answered with exactly the group it asked for.
        while let Some(request) = next {
            let (offset, length, _) = range(&request);
            let end = (offset + u64::from(length)).min(len);
            let group = &bytes[offset as usize..end as usize];

            next = blobs
                .on_response(db, &peer(), &bytes_for(&request, group), quota)
                .unwrap();
        }

        hash
    }

    #[test]
    fn a_peer_can_query_and_fetch_a_blob_it_is_offered() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let blob = given_blob(&db, &store, b"the quick brown fox");

        let answer = blobs
            .serve(&db, &peer(), &local(), &query(&blob.sha256), quota(1024))
            .unwrap();

        // A whole holding reports the length twice: what there is, and what is here.
        assert_eq!(reported(&answer), (BlobStatus::Ok, Some(19), 19, false));

        let answer = blobs
            .serve(
                &db,
                &peer(),
                &local(),
                &fetch(&blob.sha256, 4, 11, false),
                quota(1024),
            )
            .unwrap();

        assert_eq!(carried(&answer), (BlobStatus::Ok, &b"quick brown"[..]));
    }

    #[test]
    fn a_fetch_reaching_past_the_end_is_cut_to_it() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let blob = given_blob(&db, &store, b"the quick brown fox");
        let mut serve = |request| {
            blobs
                .serve(&db, &peer(), &local(), &request, quota(1024))
                .unwrap()
        };

        let rest = serve(fetch(&blob.sha256, 4, u32::MAX, false));
        assert_eq!(carried(&rest), (BlobStatus::Ok, &b"quick brown fox"[..]));

        // A range that starts past the end, or asks for nothing, is out of range rather than a miss.
        let past = serve(fetch(&blob.sha256, 99, 101, false));
        assert_eq!(carried(&past).0, BlobStatus::OutOfRange);

        let empty = serve(fetch(&blob.sha256, 0, 0, false));
        assert_eq!(carried(&empty).0, BlobStatus::OutOfRange);
    }

    #[test]
    fn an_answer_is_cut_to_what_one_frame_holds() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let bytes = filler(2 * ANSWER_BYTES as usize);
        let hash = given_wanted_verifiable(&db, &bytes);

        store.append(&hash, &bytes).unwrap();
        verified::build(store.as_ref(), &hash).unwrap();
        db_command::complete_blob(&db, &hash, bytes.len() as u64).unwrap();

        let mut serve = |request| {
            blobs
                .serve(&db, &peer(), &local(), &request, quota(1 << 24))
                .unwrap()
        };

        let plain = serve(fetch(&hash, 0, u32::MAX, false));
        assert_eq!(carried(&plain).1.len() as u64, ANSWER_BYTES);

        // The proof rides in the same frame, and the fetcher proves what came against what it asked.
        let proved = serve(fetch(&hash, 0, u32::MAX, true));
        let (status, body) = carried(&proved);

        assert_eq!(status, BlobStatus::Ok);
        assert!(body.len() + 4 <= 1024 * 1024);
        assert_eq!(
            verified::verify(
                body,
                &verified::root(&root_of(&bytes)).unwrap(),
                0,
                bytes.len() as u64
            )
            .unwrap(),
            bytes[..ANSWER_BYTES as usize]
        );
    }

    /// The holder's budget runs out partway through a group: the fetcher keeps
    /// the chunks that came, and asks for the rest from where they end.
    #[test]
    fn a_proved_answer_cut_short_is_kept_and_the_rest_asked_for() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let bytes = filler(2 * GROUP_BYTES as usize);
        let hash = given_wanted_verifiable(&db, &bytes);
        let mut holder = given_holder(&bytes, bytes.len());
        let tight = quota(5 * 1024 + 100);
        let ample = quota(1 << 20);

        let probe = blobs.poll(&db, ample).unwrap().unwrap();
        let answer = as_peer(&mut holder, &db, &probe);
        let first = blobs
            .on_response(&db, &peer(), &answer, ample)
            .unwrap()
            .unwrap();

        // The cut lands inside the sixth chunk, which the slice carries whole.
        let cut = holder.serve(&db, &peer(), &local(), &first, tight).unwrap();
        let mut next = blobs.on_response(&db, &peer(), &cut, ample).unwrap();

        assert_eq!(store.len(&hash).unwrap(), Some(6 * 1024));
        assert_eq!(
            range(next.as_ref().unwrap()),
            (6 * 1024, GROUP_BYTES as u32, true)
        );

        while let Some(request) = next {
            let answer = as_peer(&mut holder, &db, &request);
            next = blobs.on_response(&db, &peer(), &answer, ample).unwrap();
        }

        assert_eq!(store.read(&hash, 0, u64::MAX).unwrap(), bytes);
        assert!(db_query::get_blob(&db, &hash).unwrap().unwrap().complete);
    }

    #[test]
    fn a_partial_holding_reports_the_prefix_and_the_whole_length() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();

        // A 2 MiB blob held 4 KiB of: reporting 4 KiB as the length would stop the fetcher early.
        let hash = BlobHash::digest(b"a big file");
        given_wanted_sized(&db, &hash, 2 * 1024 * 1024);
        store.append(&hash, &vec![7u8; 4096]).unwrap();

        let answer = blobs
            .serve(&db, &peer(), &local(), &query(&hash), quota(1024 * 1024))
            .unwrap();

        assert_eq!(
            reported(&answer),
            (BlobStatus::Ok, Some(2 * 1024 * 1024), 4096, false)
        );
    }

    #[test]
    fn a_peer_that_holds_only_part_is_not_fetched_from() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(1024 * 1024);

        given_wanted_sized(&db, &BlobHash::digest(b"a big file"), 2 * 1024 * 1024);

        let probe = blobs.poll(&db, quota).unwrap().unwrap();
        let next = blobs
            .on_response(&db, &peer(), &holding(&probe, 2 * 1024 * 1024, 4096), quota)
            .unwrap();

        // A prefix that cannot be verified or resumed is not started.
        assert!(next.is_none());
        assert!(blobs.poll(&db, quota).unwrap().is_none());
    }

    /// A prefix is only worth taking from a peer that can prove it, and a root
    /// that does not parse proves nothing — so the blob is fetched as though
    /// the event had named none, which passes a partial holding by.
    #[test]
    fn a_root_that_does_not_parse_leaves_the_blob_unrooted() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(4 * 1024 * 1024);
        let hash = BlobHash::digest(b"a big file");
        let event = note(
            author(1),
            1,
            "with a blob",
            Tags::new().add(
                "imeta",
                [
                    format!("x {hash}"),
                    "size 2097152".to_string(),
                    "blake3 not a root".to_string(),
                ],
            ),
        );

        db_command::publish_event(&db, &event, &author(1), 1).unwrap();

        let probe = blobs.poll(&db, quota).unwrap().unwrap();
        let next = blobs
            .on_response(&db, &peer(), &holding(&probe, 2_097_152, 32_768), quota)
            .unwrap();

        assert!(next.is_none());
        assert!(blobs.poll(&db, quota).unwrap().is_none());
    }

    #[test]
    fn an_unknown_hash_is_not_served() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let unknown = BlobHash::parse(&"ab".repeat(32)).unwrap();

        let answer = blobs
            .serve(&db, &peer(), &local(), &query(&unknown), quota(1024))
            .unwrap();

        assert_eq!(reported(&answer).0, BlobStatus::NotServed);
    }

    #[test]
    fn a_stranger_is_served_no_blob_bytes_and_fetches_none() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let blob = given_blob(&db, &store, b"the quick brown fox");

        // `Quota::STRANGER` carries no blob budget, and this refusal is what means it.
        let answer = blobs
            .serve(
                &db,
                &peer(),
                &local(),
                &query(&blob.sha256),
                Quota::STRANGER,
            )
            .unwrap();

        assert_eq!(reported(&answer).0, BlobStatus::BudgetSpent);

        given_wanted(&db, b"something wanted");
        assert!(blobs.poll(&db, Quota::STRANGER).unwrap().is_none());
    }

    #[test]
    fn the_serving_budget_bounds_what_one_peer_can_take() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let blob = given_blob(&db, &store, b"the quick brown fox");
        let quota = quota(10);
        let whole = fetch(&blob.sha256, 0, 19, false);

        // Ten bytes of budget: the answer is clipped, and what follows is refused.
        let first = blobs.serve(&db, &peer(), &local(), &whole, quota).unwrap();

        assert_eq!(carried(&first), (BlobStatus::Ok, &b"the quick "[..]));

        let second = blobs.serve(&db, &peer(), &local(), &whole, quota).unwrap();

        assert_eq!(carried(&second), (BlobStatus::BudgetSpent, &[][..]));
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
    fn a_wanted_blob_is_driven_through_a_query_and_fetches() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();

        let bytes = b"the quick brown fox";
        let hash = drive_fetch(&mut blobs, &db, bytes);

        assert_eq!(store.len(&hash).unwrap(), Some(19));
        assert!(db_query::get_blob(&db, &hash).unwrap().unwrap().complete);
        assert!(blobs.active.is_none());
    }

    /// Every request goes out under its own id, so a late answer to one the
    /// fetcher gave up on is not taken for the request that replaced it.
    #[test]
    fn an_answer_to_a_request_given_up_on_is_not_taken_for_the_next() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(Quota::CONTACT.blob_bytes);
        let bytes = b"the quick brown fox";

        given_wanted(&db, bytes);

        let abandoned = blobs.poll(&db, quota).unwrap().unwrap();
        blobs.abandon_in_flight();
        let current = blobs.poll(&db, quota).unwrap().unwrap();

        assert_ne!(abandoned.id(), current.id());

        let late = blobs
            .on_response(&db, &peer(), &holding(&abandoned, 19, 19), quota)
            .unwrap();

        assert!(late.is_none());
        assert!(blobs.active.is_some());

        let next = blobs
            .on_response(&db, &peer(), &holding(&current, 19, 19), quota)
            .unwrap();

        assert_eq!(range(&next.unwrap()), (0, 19, false));
    }

    /// A blob lives as long as an event references it, however much else is held.
    #[test]
    fn a_completed_fetch_keeps_every_other_held_blob() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();

        let held = BlobHash::digest(b"a large picture held since an earlier session");
        given_wanted_sized(&db, &held, 256 * 1024 * 1024 + 1);
        db_command::complete_blob(&db, &held, 256 * 1024 * 1024 + 1).unwrap();

        let fetched = drive_fetch(&mut blobs, &db, b"the quick brown fox");

        assert!(db_query::get_blob(&db, &held).unwrap().unwrap().complete);
        assert!(db_query::get_blob(&db, &fetched).unwrap().unwrap().complete);
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

        // One file named by the hash and a complete record: the store a real session uses.
        assert_eq!(store.len(&hash).unwrap(), Some(19));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        assert!(dir.join(hash.as_str()).is_file());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_battery_floor_gates_fetches() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(Quota::CONTACT.blob_bytes);

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
        let quota = quota(Quota::CONTACT.blob_bytes);

        given_wanted(&db, b"nobody holds this");

        let probe = blobs.poll(&db, quota).unwrap().unwrap();
        let next = blobs
            .on_response(&db, &peer(), &refusal(&probe, BlobStatus::NotServed), quota)
            .unwrap();

        // The blob is still wanted, but asking this peer the moment a slot frees is a loop.
        assert!(next.is_none());
        assert!(blobs.poll(&db, quota).unwrap().is_none());
        assert_eq!(db_query::wanted_blobs(&db, 10).unwrap().len(), 1);
    }

    #[test]
    fn the_want_list_moves_past_a_hash_the_peer_missed() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(Quota::CONTACT.blob_bytes);

        let first = given_wanted(&db, b"the first one");
        let second = given_wanted(&db, b"the second one");

        let probe = blobs.poll(&db, quota).unwrap().unwrap();
        blobs
            .on_response(&db, &peer(), &refusal(&probe, BlobStatus::NotServed), quota)
            .unwrap();

        // The next poll asks for the other blob rather than the same one.
        let next = blobs.poll(&db, quota).unwrap().unwrap();

        assert_ne!(queried(&next), queried(&probe));
        assert!([first, second].contains(&queried(&next)));
    }

    #[test]
    fn bytes_that_fail_their_hash_are_dropped_and_the_peer_skipped() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let quota = quota(Quota::CONTACT.blob_bytes);
        let bytes = b"the quick brown fox";
        let hash = given_wanted(&db, bytes);

        let probe = blobs.poll(&db, quota).unwrap().unwrap();
        let request = blobs
            .on_response(&db, &peer(), &holding(&probe, 19, 19), quota)
            .unwrap()
            .unwrap();

        // The right number of bytes, the wrong bytes.
        let next = blobs
            .on_response(
                &db,
                &peer(),
                &bytes_for(&request, b"the quick brown FOX"),
                quota,
            )
            .unwrap();

        assert!(next.is_none());
        assert!(!db_query::get_blob(&db, &hash).unwrap().unwrap().complete);
        // The bytes go, the record agrees, and this peer would only send the same bytes again.
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
        let quota = quota(Quota::CONTACT.blob_bytes);
        let hash = given_wanted_sized(&db, &BlobHash::digest(b"small"), 8);

        let probe = blobs.poll(&db, quota).unwrap().unwrap();
        let request = blobs
            .on_response(&db, &peer(), &holding(&probe, 8, 8), quota)
            .unwrap()
            .unwrap();

        // Eight bytes were asked for and declared; a megabyte arrives.
        let next = blobs
            .on_response(
                &db,
                &peer(),
                &bytes_for(&request, &vec![0u8; 1024 * 1024]),
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
        let quota = quota(Quota::CONTACT.blob_bytes);

        given_wanted_sized(&db, &BlobHash::digest(b"small"), 8);

        let probe = blobs.poll(&db, quota).unwrap().unwrap();
        let next = blobs
            .on_response(&db, &peer(), &holding(&probe, 1_048_576, 1_048_576), quota)
            .unwrap();

        // Another length is another file, because the event id commits to the size in `imeta`.
        assert!(next.is_none());
        assert!(blobs.poll(&db, quota).unwrap().is_none());
    }

    #[test]
    fn a_blob_larger_than_the_budget_is_not_started() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(1024);

        given_wanted_sized(&db, &BlobHash::digest(b"a big file"), 64 * 1024);

        let probe = blobs.poll(&db, quota).unwrap().unwrap();
        let next = blobs
            .on_response(&db, &peer(), &holding(&probe, 65_536, 65_536), quota)
            .unwrap();

        assert!(next.is_none());
    }

    #[test]
    fn the_fetch_budget_stops_a_transfer_it_cannot_finish() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(GROUP_BYTES);
        let bytes = vec![3u8; 2 * GROUP_BYTES as usize];
        let len = bytes.len() as u64;

        given_wanted(&db, &bytes);

        let probe = blobs.poll(&db, quota).unwrap().unwrap();
        let next = blobs
            .on_response(&db, &peer(), &holding(&probe, len, len), quota)
            .unwrap();

        // Two groups wanted, one group of budget: it is not begun at all.
        assert!(next.is_none());
        assert!(blobs.poll(&db, quota).unwrap().is_none());
    }

    #[test]
    fn the_bulk_channel_is_asked_for_once_and_only_when_it_pays() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, _) = exchange();
        let quota = quota(Quota::CONTACT.blob_bytes);

        // A small blob moves in a couple of round trips; a channel costs more than it saves.
        drive_fetch(&mut blobs, &db, b"the quick brown fox");
        assert!(!blobs.take_l2cap_request());

        // One over the threshold does pay, and asks exactly once.
        given_wanted_sized(&db, &BlobHash::digest(b"a big file"), L2CAP_THRESHOLD_BYTES);

        let probe = blobs.poll(&db, quota).unwrap().unwrap();
        blobs
            .on_response(
                &db,
                &peer(),
                &holding(&probe, L2CAP_THRESHOLD_BYTES, L2CAP_THRESHOLD_BYTES),
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
        let quota = quota(Quota::CONTACT.blob_bytes);

        given_wanted(&db, b"the first one");
        given_wanted(&db, b"the second one");

        let probe = blobs.poll(&db, quota).unwrap().unwrap();
        blobs
            .on_response(
                &db,
                &peer(),
                &refusal(&probe, BlobStatus::BudgetSpent),
                quota,
            )
            .unwrap();

        // No other hash will fare better once the peer's budget is spent.
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
        let quota = quota(Quota::CONTACT.blob_bytes);

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

    /// An uppercase hash is stored lowercase, which makes the row a peer's
    /// event writes the row this device can read back and complete.
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
    fn a_blob_a_peer_may_not_be_served_the_event_for_is_not_served() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let blob = given_blob(&db, &store, b"the quick brown fox");

        // Refused is a veto over the whole set, and it is not the pubkey that sorts first.
        let mut policy = Policy::new(author(1));
        policy.refuse(author(3));

        let refused = Peer::bind(LinkId(1), [author(2), author(3)], &policy);
        assert!(refused.policy.is_refused());
        let answer = blobs
            .serve(&db, &refused, &local(), &query(&blob.sha256), quota(1024))
            .unwrap();

        assert_eq!(reported(&answer).0, BlobStatus::NotServed);
    }

    /// The narrowest referencing event wins, or a peer could publish an event
    /// naming somebody else's hash and be served their media.
    #[test]
    fn a_second_event_the_peer_may_not_be_served_takes_the_blob_with_it() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let blob = given_blob(&db, &store, b"the quick brown fox");

        let offered = blobs
            .serve(&db, &peer(), &local(), &query(&blob.sha256), quota(1024))
            .unwrap();

        assert_eq!(reported(&offered).0, BlobStatus::Ok);

        let held = note(
            author(3),
            2,
            "the same media",
            Tags::new().add("imeta", [format!("x {}", blob.sha256)]),
        );

        db_command::receive_event(&db, &held, &[author(3)], 2).unwrap();

        let narrowed = blobs
            .serve(&db, &peer(), &local(), &query(&blob.sha256), quota(1024))
            .unwrap();

        assert_eq!(reported(&narrowed).0, BlobStatus::NotServed);
    }

    /// A copy the user threw away is judged by who could see it, so it does not take the live one's media.
    #[test]
    fn a_trashed_copy_of_a_post_does_not_withhold_the_live_ones_media() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let blob = given_blob(&db, &store, b"the quick brown fox");
        let copy = note(
            author(1),
            2,
            "the same picture again",
            Tags::new().add("imeta", [format!("x {}", blob.sha256)]),
        );

        db_command::publish_event(&db, &copy, &author(1), 2).unwrap();
        db_command::set_trashed(&db, &copy.id, true, 3).unwrap();

        let answer = blobs
            .serve(&db, &peer(), &local(), &query(&blob.sha256), quota(1024))
            .unwrap();

        assert_eq!(reported(&answer).0, BlobStatus::Ok);
    }

    /// Media only the trash still references is not handed on, since nothing it belongs to is.
    #[test]
    fn media_only_a_trashed_post_references_is_not_served() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let blob = given_blob(&db, &store, b"the quick brown fox");
        let referencing = db_query::events_referencing_blob(&db, &blob.sha256).unwrap();

        db_command::set_trashed(&db, &referencing[0], true, 3).unwrap();

        let answer = blobs
            .serve(&db, &peer(), &local(), &query(&blob.sha256), quota(1024))
            .unwrap();

        assert_eq!(reported(&answer).0, BlobStatus::NotServed);
    }

    /// The BLAKE3 root of `bytes`, the way an `imeta` tag carries it.
    fn root_of(bytes: &[u8]) -> String {
        verified::hash(bytes).to_hex().to_string()
    }

    /// `len` bytes of something that is not all one byte.
    fn filler(len: usize) -> Vec<u8> {
        (0..len).map(|index| (index % 251) as u8).collect()
    }

    /// Publish an event wanting a blob over `bytes`, carrying the root that
    /// lets each group be proved as it arrives.
    fn given_wanted_verifiable(db: &Db, bytes: &[u8]) -> BlobHash {
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
                    format!("blake3 {}", root_of(bytes)),
                ],
            ),
        );

        db_command::publish_event(db, &event, &author(1), 1).unwrap();

        hash
    }

    /// The device on the other end: an exchange holding the first `available`
    /// bytes of `bytes`, with the tree that proves them where it holds them
    /// all. A prefix has no tree, because a tree is over the whole file.
    fn given_holder(bytes: &[u8], available: usize) -> BlobExchange {
        let (blobs, store) = exchange();
        let hash = BlobHash::digest(bytes);

        store.append(&hash, &bytes[..available]).unwrap();

        if available == bytes.len() {
            verified::build(store.as_ref(), &hash).unwrap();
        }

        blobs
    }

    /// What the peer answers, out of its own serving half rather than out of a
    /// hand-written answer: the wire is then the same one the code speaks.
    fn as_peer(holder: &mut BlobExchange, db: &Db, request: &BlobMessage) -> BlobMessage {
        holder
            .serve(db, &peer(), &local(), request, quota(1 << 24))
            .unwrap()
    }

    /// Drive a fetch to a stop against a peer holding `available` bytes of
    /// `bytes`, returning every request the fetcher made.
    fn drive_against(
        blobs: &mut BlobExchange,
        db: &Db,
        bytes: &[u8],
        available: usize,
        quota: Quota,
    ) -> Vec<BlobMessage> {
        let mut holder = given_holder(bytes, available);
        let mut made = Vec::new();
        let mut next = blobs.poll(db, quota).unwrap();

        while let Some(request) = next {
            let answer = as_peer(&mut holder, db, &request);

            made.push(request);
            next = blobs.on_response(db, &peer(), &answer, quota).unwrap();
        }

        made
    }

    /// Every group crosses as a slice and is checked against the root before
    /// it is written, which is what makes the bytes on disk worth keeping.
    #[test]
    fn a_blob_with_a_root_is_fetched_group_by_group() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let bytes = filler(3 * GROUP_BYTES as usize + 7);
        let hash = given_wanted_verifiable(&db, &bytes);

        let made = drive_against(&mut blobs, &db, &bytes, bytes.len(), quota(1 << 20));

        // A query, then one fetch per group and a fourth for the remainder, each asking for a proof.
        assert_eq!(queried(&made[0]), hash);
        assert_eq!(made.len(), 1 + 4);
        assert!(made[1..].iter().all(|request| range(request).2));

        assert_eq!(store.read(&hash, 0, u64::MAX).unwrap(), bytes);
        assert!(db_query::get_blob(&db, &hash).unwrap().unwrap().complete);

        // The tree is beside the bytes, ready to answer the next peer in kind.
        assert!(store.outboard_len(&hash).unwrap().is_some());
    }

    /// The groups a dropped link stopped short of are picked up by the next
    /// session rather than fetched again.
    #[test]
    fn a_transfer_resumes_from_the_groups_it_proved() {
        let db = Db::open_in_memory().unwrap();
        let store = Arc::new(MemoryBlobStore::default());
        let bytes = filler(4 * GROUP_BYTES as usize);
        let hash = given_wanted_verifiable(&db, &bytes);
        let mut holder = given_holder(&bytes, bytes.len());
        let quota = quota(1 << 20);

        // Two groups in, the link drops and the session goes with it.
        let mut first = BlobExchange::new(store.clone());
        let mut next = first.poll(&db, quota).unwrap();

        while store.len(&hash).unwrap() < Some(2 * GROUP_BYTES) {
            let request = next.expect("the fetch to still be going");
            let answer = as_peer(&mut holder, &db, &request);

            next = first.on_response(&db, &peer(), &answer, quota).unwrap();
        }

        assert_eq!(
            db_query::get_blob(&db, &hash)
                .unwrap()
                .unwrap()
                .stored_bytes,
            2 * GROUP_BYTES as i64
        );

        // The next session picks the blob up and asks from the group after those held.
        let mut second = BlobExchange::new(store.clone());
        let made = drive_against(&mut second, &db, &bytes, bytes.len(), quota);

        assert_eq!(range(&made[1]), (2 * GROUP_BYTES, GROUP_BYTES as u32, true));

        assert_eq!(store.read(&hash, 0, u64::MAX).unwrap(), bytes);
        assert!(db_query::get_blob(&db, &hash).unwrap().unwrap().complete);
    }

    /// A prefix cannot be proved — the tree is over the whole file — so a peer
    /// holding one offers no proofs and a rooted blob passes it by.
    #[test]
    fn a_peer_holding_a_prefix_cannot_prove_it_and_is_passed_by() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let bytes = filler(4 * GROUP_BYTES as usize);
        let hash = given_wanted_verifiable(&db, &bytes);

        let made = drive_against(
            &mut blobs,
            &db,
            &bytes,
            2 * GROUP_BYTES as usize,
            quota(1 << 20),
        );

        // The query is the whole conversation: no bytes are asked for.
        assert_eq!(made.len(), 1);
        assert_eq!(queried(&made[0]), hash);
        assert!(store.len(&hash).unwrap().is_none());
    }

    /// A bad group costs that group and the peer, not the whole transfer.
    #[test]
    fn a_group_that_does_not_prove_out_leaves_the_prefix_alone() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let bytes = filler(3 * GROUP_BYTES as usize);
        let hash = given_wanted_verifiable(&db, &bytes);
        let mut holder = given_holder(&bytes, bytes.len());
        let quota = quota(1 << 20);

        let mut next = blobs.poll(&db, quota).unwrap();
        let mut groups = 0;

        while let Some(request) = next {
            let mut answer = as_peer(&mut holder, &db, &request);

            if let BlobMessage::Bytes { body, .. } = &mut answer {
                groups += 1;

                // The second group arrives altered by one bit.
                if groups == 2 {
                    body[3] ^= 1;
                }
            }

            next = blobs.on_response(&db, &peer(), &answer, quota).unwrap();
        }

        assert_eq!(groups, 2);
        assert_eq!(store.len(&hash).unwrap(), Some(GROUP_BYTES));

        let blob = db_query::get_blob(&db, &hash).unwrap().unwrap();

        assert_eq!(blob.stored_bytes, GROUP_BYTES as i64);
        assert!(!blob.complete);
        // Nothing else is asked of this peer, which keeps the slot from livelocking.
        assert!(blobs.poll(&db, quota).unwrap().is_none());
    }

    /// The serving half: a device holding the whole file answers a range with
    /// the proof over it, and the answer checks out against the root alone.
    #[test]
    fn a_held_blob_answers_a_range_with_a_proof() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let bytes = filler(5 * GROUP_BYTES as usize + 3);
        let hash = given_wanted_verifiable(&db, &bytes);

        store.append(&hash, &bytes).unwrap();
        verified::build(store.as_ref(), &hash).unwrap();
        db_command::complete_blob(&db, &hash, bytes.len() as u64).unwrap();

        let probe = blobs
            .serve(&db, &peer(), &local(), &query(&hash), quota(1 << 20))
            .unwrap();

        assert!(reported(&probe).3, "a whole copy did not offer proofs");

        let answer = blobs
            .serve(
                &db,
                &peer(),
                &local(),
                &fetch(&hash, GROUP_BYTES, GROUP_BYTES as u32, true),
                quota(1 << 20),
            )
            .unwrap();
        let (status, body) = carried(&answer);

        assert_eq!(status, BlobStatus::Ok);
        // The answer is longer than the range it carries, because the proof rides with the bytes.
        assert!(body.len() > GROUP_BYTES as usize);

        let proved = verified::verify(
            body,
            &verified::root(&root_of(&bytes)).unwrap(),
            GROUP_BYTES,
            GROUP_BYTES,
        )
        .unwrap();

        assert_eq!(
            proved,
            bytes[GROUP_BYTES as usize..2 * GROUP_BYTES as usize]
        );
    }

    /// A peer holding only a prefix is asked for a proof it cannot give, and
    /// says so rather than sending bytes nothing can check.
    #[test]
    fn a_proof_asked_of_a_prefix_is_refused() {
        let db = Db::open_in_memory().unwrap();
        let bytes = filler(4 * GROUP_BYTES as usize);
        let hash = given_wanted_verifiable(&db, &bytes);
        let mut holder = given_holder(&bytes, 2 * GROUP_BYTES as usize);

        let answer = as_peer(&mut holder, &db, &fetch(&hash, 0, GROUP_BYTES as u32, true));

        assert_eq!(carried(&answer), (BlobStatus::CannotProve, &[][..]));
    }

    /// A blob of one group is proved the same way anything larger is: Bao has
    /// no floor below which a range carries no proof.
    #[test]
    fn a_blob_of_one_group_is_fetched_in_one_group() {
        let db = Db::open_in_memory().unwrap();
        let (mut blobs, store) = exchange();
        let bytes = filler(GROUP_BYTES as usize);
        let hash = given_wanted_verifiable(&db, &bytes);

        let made = drive_against(&mut blobs, &db, &bytes, bytes.len(), quota(1 << 20));

        assert_eq!(made.len(), 2);
        assert_eq!(store.read(&hash, 0, u64::MAX).unwrap(), bytes);
        assert!(db_query::get_blob(&db, &hash).unwrap().unwrap().complete);
    }
}
