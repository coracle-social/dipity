//! The core as the shell sees it: bytes and radio events in, actions out.
//!
//! One object owning every live session, the store, and the identity key.
//! Sans-io: it performs no I/O and reads no clock, so the whole stack above the
//! radio runs in `cargo test` with two nodes handing each other bytes.
//!
//! # The call shape
//!
//! Every entry point returns [`Action`]s. Nothing calls the shell back, so no
//! lock is ever held across a foreign call and the shell can answer an action
//! synchronously without deadlocking the core against itself.
//!
//! Nothing here takes the time. Whatever needs it calls
//! [`clock::now`](crate::clock::now), and a test drives the heartbeat timeout
//! and the drain cap through [`clock::at`](crate::clock::at) rather than by
//! sleeping.
//!
//! # Where this sits
//!
//! The view does not go through here; it holds the same `Arc<Db>` and reads it
//! directly. Only gossip comes through a `Node`.
//!
//! ```text
//!   shell ──radio events──► Node ──Action──► shell
//!                            │
//!   view ─────────────────► Db ◄┘
//! ```

mod scheduler;

pub use scheduler::{MAX_LINKS, RSSI_FLOOR};

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, bail};
use coracle_lib::events::HashedEvent;
use coracle_lib::keys::SecretKey;
use tokio::sync::broadcast::{self, error::TryRecvError};

use crate::blobs::{BlobStore, FileBlobStore, verified};
use crate::clock;
use crate::db::blob::channel as blob_channel;
use crate::db::blob::channel::BlobChange;
use crate::db::command as db_command;
use crate::db::event::channel::{self, EventChange};
use crate::db::{Db, query};
use crate::link::{LinkId, PeripheralId, Role};
use crate::model::{Blob, BlobHash, Policy};
use crate::session::gate::Presence;
use crate::session::{Ending, Session, State};
use crate::transport::Channel;
use scheduler::Scheduler;

/// Minimum gap between retention sweeps, against a window measured in days.
const SWEEP_INTERVAL_SECONDS: i64 = 60 * 60;

/// Something the shell does on the core's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Start or stop scanning for our service UUID.
    Scan(bool),
    /// Start or stop advertising it.
    Advertise(bool),
    /// Dial a peripheral the scheduler admitted.
    Connect(PeripheralId),
    /// Tear a link down.
    Disconnect(LinkId),
    /// Write one fragment, already sized to the link's MTU.
    ///
    /// The shell reports back with [`Node::write_complete`] once the ATT write
    /// is acknowledged, which releases the next.
    Send(LinkId, Vec<u8>),
    /// Ask the user whether an unadmitted stranger may connect. Answer with
    /// [`Node::approve`]; the link is held up to the drain cap meanwhile.
    RequestApproval(LinkId),
    /// Open an L2CAP channel, once outstanding blob bytes justify the setup
    /// round trip. Bulk moves across; control frames stay on GATT.
    OpenL2cap(LinkId),
    /// Call [`Node::tick`] at or after this unix second.
    ///
    /// Advisory: iOS runs no timer for a suspended app, so the heartbeat and
    /// the connection scheduler both recover on the next radio callback.
    WakeAt(i64),
}

/// Every live session, the store behind them, and the key they authenticate
/// with.
pub struct Node {
    /// The store
    db: Arc<Db>,
    /// The nostr identity
    identity: SecretKey,
    /// The user's policy
    policy: Arc<Policy>,
    /// Where blob bytes go, provided by the shell.
    blobs: Arc<dyn BlobStore>,
    /// The battery level, percent, as the shell last reported it.
    battery: Option<u8>,
    /// Live sessions, keyed on link
    sessions: BTreeMap<LinkId, Session>,
    /// The peripheral each live dialed link came from, for grading its
    /// outcome. One entry per dialed session, dropped with it, since its
    /// length is the central-link cap.
    link_peripheral: BTreeMap<LinkId, PeripheralId>,
    /// Decides which advertised peers to dial, and when.
    scheduler: Scheduler,
    /// The shared rolling spending ledger, metered across sessions.
    spending: Arc<crate::sync::spending::SpendingLedger>,
    /// Where the app is, for the cool-off admission window. `None` until the
    /// shell first reports it.
    presence: Option<Presence>,
    /// This store's event channel, so a stored own event is offered to every
    /// connected peer without the writer knowing.
    events: broadcast::Receiver<EventChange>,
    /// This store's blob channel, so bytes are reclaimed when their record
    /// goes without whoever removed it holding the blob store.
    blob_changes: broadcast::Receiver<BlobChange>,
    /// When the retention sweep last ran, so ticking often sweeps rarely.
    events_swept_at: Option<i64>,
}

impl Node {
    /// Build a node over a store the shell has opened and a key it has read out
    /// of the Keychain or Keystore, with the blob store of the shell's choosing.
    pub fn new(db: Arc<Db>, identity: SecretKey, blobs: Arc<dyn BlobStore>) -> Result<Self> {
        let policy = Arc::new(query::policy(&db, &identity.public_key())?);

        let mut node = Self {
            events: channel::subscribe(&db),
            blob_changes: blob_channel::subscribe(&db),
            db,
            identity,
            policy,
            blobs,
            battery: None,
            sessions: BTreeMap::new(),
            link_peripheral: BTreeMap::new(),
            scheduler: Scheduler::default(),
            spending: Arc::new(crate::sync::spending::SpendingLedger::default()),
            presence: None,
            events_swept_at: None,
        };

        // Whatever was removed while no node was listening is still on disk.
        if let Err(error) = node.sweep_blobs() {
            log::error!("sweeping the blob store at open failed: {error:#}");
        }

        // A device that meets nobody never ticks, so opening is the sure sweep.
        node.sweep_events();

        Ok(node)
    }

    /// Build a node with the file-backed blob store, under the same directory
    /// the shell already tells the core to open the database in. Blobs live in
    /// `directory/blobs`, one file per hash.
    pub fn open(db: Arc<Db>, identity: SecretKey, directory: impl AsRef<Path>) -> Result<Self> {
        let blobs = Arc::new(FileBlobStore::open(directory.as_ref().join("blobs"))?);

        Self::new(db, identity, blobs)
    }

    /// This device's pubkey. A method, not a field, because the field is the
    /// secret key and it never leaves the node.
    #[must_use]
    pub fn identity(&self) -> coracle_lib::keys::PublicKey {
        self.identity.public_key()
    }

    // --------------------------------------- Radio events, all from the shell

    /// A peripheral advertising our service UUID was seen.
    ///
    /// Whether to dial is the core's call; the shell keeps the radio.
    /// `docs/discovery.md#connection-scheduling`.
    pub fn peripheral_seen(&mut self, peripheral: &PeripheralId, rssi: i16) -> Vec<Action> {
        self.scheduler.seen(peripheral.clone(), rssi);

        match self.scheduler.next_dial(self.central_links()) {
            Some(peripheral) => vec![Action::Connect(peripheral)],
            None => Vec::new(),
        }
    }

    /// A GATT connection came up, with the MTU the link negotiated.
    ///
    /// `peripheral` is the one this device dialed, so the scheduler can grade
    /// the link's outcome; a link the peer dialed has none.
    pub fn link_up(
        &mut self,
        link: LinkId,
        peripheral: Option<PeripheralId>,
        role: Role,
        mtu: usize,
    ) -> Result<Vec<Action>> {
        if self.sessions.contains_key(&link) {
            bail!("link {link:?} is already up");
        }

        let mut session = Session::open(
            link,
            role,
            mtu,
            Arc::clone(&self.policy),
            self.identity.clone(),
            Arc::clone(&self.blobs),
            Arc::clone(&self.spending),
        )?;
        session.gate.presence = self.presence;

        if role == Role::Dialer {
            session.initiate()?;
        }

        if let Some(peripheral) = peripheral {
            self.link_peripheral.insert(link, peripheral);
        }

        self.sessions.insert(link, session);

        Ok(self.collect())
    }

    /// A link went away. The shell only reports a disconnect that fired, which is
    /// reliable: the link is gone and nothing on it will ever finish, so the
    /// session closes rather than waiting out a drain that cannot complete.
    ///
    /// A peer that walked away is redialed soon: they usually come back. A
    /// teardown this device decided has already been graded by then, in
    /// [`collect`](Self::collect), and keeps its own backoff.
    pub fn link_down(&mut self, link: LinkId) -> Vec<Action> {
        if let Some(session) = self.sessions.get_mut(&link) {
            session.close(Ending::WalkedAway);
        }

        self.collect()
    }

    /// One write arrived off the characteristic.
    ///
    /// Reassembles, decrypts, and hands the frame to the session or to
    /// [`crate::sync`] by channel. A malformed or unauthenticated frame ends
    /// the link rather than being dropped.
    pub fn bytes_received(&mut self, link: LinkId, write: &[u8]) -> Vec<Action> {
        let Err(error) = self.receive_frame(link, write) else {
            return self.collect();
        };

        log::debug!("dropping link {link:?}: {error:#}");

        // Close rather than only telling the shell: a session left in the table keeps its slot.
        if let Some(session) = self.sessions.get_mut(&link) {
            session.close(Ending::Refused);
        }

        self.collect()
    }

    /// Reassemble, decrypt, and dispatch one frame off the characteristic.
    fn receive_frame(&mut self, link: LinkId, write: &[u8]) -> Result<()> {
        let Some(session) = self.sessions.get_mut(&link) else {
            return Ok(());
        };

        let Some(frame) = session.receive(write)? else {
            return Ok(());
        };

        match frame.channel {
            Channel::Control => session.advance(&self.db, &frame)?,
            Channel::Sync => session.handle_sync(&self.db, &frame)?,
            Channel::Blob => {} // blob transfers are not yet wired
        }

        Ok(())
    }

    /// The write the shell was handed has been acknowledged, which releases the
    /// next fragment.
    pub fn write_complete(&mut self, link: LinkId) -> Vec<Action> {
        if let Some(session) = self.sessions.get_mut(&link) {
            session.acknowledge_write();
        }

        self.collect()
    }

    /// Time passed. Expires heartbeats, ends drains, and gives the scheduler a
    /// chance to dial.
    pub fn tick(&mut self) -> Vec<Action> {
        for session in self.sessions.values_mut() {
            if session.expired() {
                match session.state {
                    // A gate hold and a drain both close outright: nothing in flight to finish.
                    State::GatePending { .. } => session.close(Ending::Refused),
                    State::Draining { .. } => session.close(Ending::WalkedAway),
                    _ => session.drain(),
                }
            }
        }

        self.sweep_events();

        // A queued candidate may now be past its rate limit, its backoff, or the link cap.
        let mut actions = Vec::new();
        if let Some(peripheral) = self.scheduler.next_dial(self.central_links()) {
            actions.push(Action::Connect(peripheral));
        }

        actions.extend(self.collect());
        actions
    }

    // -------------------------------------------- The view, through the shell

    /// The battery level, percent, as the shell reports it. Blob transfers are
    /// gated on it.
    pub fn battery(&mut self, level: u8) -> Vec<Action> {
        self.battery = Some(level);

        for session in self.sessions.values_mut() {
            session.set_battery(Some(level));
        }

        self.collect()
    }

    /// The app came to the foreground, which accepts unknown peers outright.
    pub fn notify_foregrounded(&mut self) -> Vec<Action> {
        self.set_presence(Presence::Foreground)
    }

    /// The app went to the background, which starts the cool-off window.
    ///
    /// The window runs from here rather than from foregrounding, so a user who
    /// reads for a while and pockets the phone still gets the full cool-off.
    /// `docs/policy.md#discoverability`.
    pub fn notify_backgrounded(&mut self) -> Vec<Action> {
        self.set_presence(Presence::Background {
            since: clock::now(),
        })
    }

    /// Record where the app is and rebind it on every live session.
    fn set_presence(&mut self, presence: Presence) -> Vec<Action> {
        self.presence = Some(presence);

        for session in self.sessions.values_mut() {
            session.gate.presence = self.presence;
        }

        self.collect()
    }

    /// Answer a pending consent gate. `approved` admits the stranger and
    /// resumes the exchange; refusing closes the link and the scheduler leaves
    /// the peer alone.
    pub fn approve(&mut self, link: LinkId, approved: bool) -> Result<Vec<Action>> {
        if let Some(session) = self.sessions.get_mut(&link) {
            session.approve(&self.db, approved)?;
        }

        if !approved && let Some(peripheral) = self.link_peripheral.get(&link) {
            self.scheduler.declined(peripheral);
        }

        Ok(self.collect())
    }

    /// The user changed a preference, so re-read the policy and rebind it on
    /// every live session.
    ///
    /// Driven by [`db::pref::channel`](crate::db::pref::channel), because a
    /// session that cached a policy would keep serving a peer the user has just
    /// blocked. A session whose peer the new policy blocks closes here.
    pub fn policy_changed(&mut self) -> Result<Vec<Action>> {
        self.policy = Arc::new(query::policy(&self.db, &self.identity.public_key())?);

        for session in self.sessions.values_mut() {
            session.set_policy(Arc::clone(&self.policy));
        }

        Ok(self.collect())
    }

    /// Store an event the user wrote and the media it attaches, then offer both.
    pub fn publish(&mut self, event: &HashedEvent, media: &[&[u8]]) -> Result<Vec<Action>> {
        db_command::publish_event(&self.db, event, &self.identity.public_key(), clock::now())?;
        self.store_own_media(event, media)?;

        Ok(self.collect())
    }

    /// The `imeta` entries an event has to carry for a peer to fetch `bytes` and check what it gets.
    pub fn media_tags(bytes: &[u8]) -> Vec<String> {
        vec![
            format!("x {}", BlobHash::digest(bytes)),
            format!("size {}", bytes.len()),
            format!("blake3 {}", verified::hash(bytes).to_hex()),
        ]
    }

    /// Write the media a published event attaches, and mark whole every blob it names that we hold.
    fn store_own_media(&self, event: &HashedEvent, media: &[&[u8]]) -> Result<()> {
        let attached: Vec<Blob> = event
            .tags
            .find_all("imeta")
            .filter_map(Blob::from_imeta)
            .collect();

        for bytes in media {
            let sha256 = BlobHash::digest(bytes);

            if !attached.iter().any(|blob| blob.sha256 == sha256) {
                bail!("the event being published attaches no media hashing to {sha256}");
            }

            // The same file published twice is one blob; appending would make it two copies of itself.
            self.blobs.delete(&sha256)?;
            self.blobs.append(&sha256, bytes)?;
            verified::build(self.blobs.as_ref(), &sha256)?;
        }

        for blob in attached {
            // A prefix marked whole is how a device comes to serve half a file as the whole of it.
            let Some(size) = blob.declared_size() else {
                continue;
            };

            if self.blobs.len(&blob.sha256)? == Some(size) {
                db_command::complete_blob(&self.db, &blob.sha256, size, clock::now())?;
            }
        }

        Ok(())
    }

    // ------------------------------------ Draining what the sessions produced

    /// How many links this device dialed.
    ///
    /// `docs/discovery.md` caps *central* links, so a peer dialing us must not
    /// spend one of our slots — six inbound connections would otherwise stop
    /// this device dialing anyone, which in a crowd is the moment it most
    /// wants to.
    fn central_links(&self) -> usize {
        self.link_peripheral.len()
    }

    /// Sweep every session for work: fragments to write, links to tear down,
    /// and the earliest deadline worth waking for.
    ///
    /// The one place actions are produced, so an entry point cannot forget to
    /// flush a session it advanced.
    fn collect(&mut self) -> Vec<Action> {
        self.offer_saved_events();
        self.reclaim_removed_blobs();

        // Heartbeats go out before writes are drained, so a quiet session still proves alive.
        for session in self.sessions.values_mut() {
            if let Err(error) = session.maybe_heartbeat() {
                log::error!(
                    "sending a heartbeat on link {:?} failed: {error:#}",
                    session.link
                );
            }
        }

        let mut actions = Vec::new();

        // A held gate asks the shell once.
        for session in self.sessions.values_mut() {
            if session.request_approval() {
                actions.push(Action::RequestApproval(session.link));
            }
        }

        // A blob transfer is the opening that justifies a bulk channel.
        for session in self.sessions.values_mut() {
            if session.take_l2cap_request() {
                actions.push(Action::OpenL2cap(session.link));
            }
        }

        for session in self.sessions.values_mut() {
            loop {
                match session.next_write() {
                    Ok(Some(fragment)) => actions.push(Action::Send(session.link, fragment)),
                    Ok(None) => break,
                    // Sealing fails only on a wire that can no longer carry the session.
                    Err(error) => {
                        log::error!(
                            "sealing a fragment on link {:?} failed: {error:#}",
                            session.link
                        );
                        break;
                    }
                }
            }
        }

        let closed: Vec<(LinkId, Ending)> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.state == State::Closed)
            .map(|(link, session)| (*link, session.ending))
            .collect();

        // Every teardown passes through here, so grading is one decision.
        for (link, ending) in closed {
            self.sessions.remove(&link);

            if let Some(peripheral) = self.link_peripheral.remove(&link) {
                match ending {
                    Ending::WalkedAway => self.scheduler.walked_away(&peripheral),
                    Ending::Refused => self.scheduler.refused(&peripheral),
                }
            }

            actions.push(Action::Disconnect(link));
        }

        if let Some(deadline) = self.sessions.values().map(Session::deadline).min() {
            actions.push(Action::WakeAt(deadline));
        }

        actions
    }

    /// Offer events the store just saved to every connected peer.
    ///
    /// Drains the store's event channel, which anything that stores an event
    /// announces on — a publish, or an own event coming home through an ingest
    /// — so the writer never has to know a peer is attached. Only events
    /// authored by this device travel this way; everything else waits for the
    /// next reconciliation, where the registers are checked.
    fn offer_saved_events(&mut self) {
        let identity = self.identity.public_key();

        loop {
            match self.events.try_recv() {
                Ok(EventChange::Stored(event)) if event.pubkey == identity => {
                    for session in self.sessions.values_mut() {
                        if let Err(error) = session.offer_event(&self.db, &event) {
                            log::error!(
                                "offering a saved event on link {:?} failed: {error:#}",
                                session.link
                            );
                        }
                    }
                }
                // Seen events are not newly stored, and Deleted are gone. Both are the view's.
                Ok(_) => {}
                // A lag says the subscriber missed changes; the next reconciliation covers them.
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                Err(TryRecvError::Lagged(_)) => {}
            }
        }
    }

    /// Delete the bytes of blobs whose record has gone.
    ///
    /// Drains the store's blob channel, which eviction and event deletion both
    /// announce on. The record is what references the file, so a store holds
    /// bytes for exactly as long as the `blob` table says to — and whoever
    /// removed the row does not have to hold the blob store to say so.
    fn reclaim_removed_blobs(&mut self) {
        loop {
            match self.blob_changes.try_recv() {
                Ok(BlobChange::Removed(sha256)) => {
                    if let Err(error) = self.blobs.delete(&sha256) {
                        log::error!("deleting the bytes of blob {sha256} failed: {error:#}");
                    }
                }
                // Recording, progress and completion are the transfer layer's and the view's.
                Ok(_) => {}
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                // A dropped removal would orphan its bytes for good, so the sweep answers for it.
                Err(TryRecvError::Lagged(_)) => {
                    if let Err(error) = self.sweep_blobs() {
                        log::error!("sweeping the blob store after a lag failed: {error:#}");
                    }
                }
            }
        }
    }

    /// Delete every byte held for a hash the `blob` table has no record of.
    ///
    /// The channel is the prompt path and it is lossy: a subscriber that falls
    /// behind misses removals, and nothing is listening at all while the app is
    /// closed. The table is the record either way, so it is what settles which
    /// files may stay.
    fn sweep_blobs(&self) -> Result<()> {
        let recorded = query::recorded_blob_hashes(&self.db)?;

        for sha256 in self.blobs.hashes()? {
            if !recorded.contains(&sha256) {
                self.blobs.delete(&sha256)?;
            }
        }

        Ok(())
    }

    /// Forget what stopped circulating, at most once an interval.
    fn sweep_events(&mut self) {
        let now = clock::now();
        let due = self
            .events_swept_at
            .is_none_or(|last| now - last >= SWEEP_INTERVAL_SECONDS);

        if due {
            self.events_swept_at = Some(now);

            let cutoff = now - i64::from(self.policy.retention_days) * 86_400;

            match db_command::forget_events_unseen_since(&self.db, &self.identity(), cutoff) {
                Ok(forgotten) => log::debug!("the retention sweep forgot {forgotten} events"),
                Err(error) => log::error!("the retention sweep failed: {error:#}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::scheduler::{
        DECLINED_BACKOFF_SECONDS, NEVER_ANSWERED_BACKOFF_SECONDS, REFUSED_BACKOFF_SECONDS,
        WALKED_AWAY_BACKOFF_SECONDS,
    };
    use super::*;
    use coracle_lib::tags::Tags;

    use crate::fixtures::{TempDir, author, note, secret};
    use crate::model::{BlobHash, Policy, Query};
    use crate::session::Session;
    use crate::sync::{Message, SubscriptionId};
    use crate::transport::Frame;
    use coracle_lib::filters::Filter;

    fn node() -> Node {
        Node::new(
            db(),
            SecretKey::generate(),
            Arc::new(crate::blobs::MemoryBlobStore::default()),
        )
        .unwrap()
    }

    fn spending() -> Arc<crate::sync::spending::SpendingLedger> {
        Arc::new(crate::sync::spending::SpendingLedger::default())
    }

    fn db() -> Arc<Db> {
        Arc::new(Db::open_in_memory().unwrap())
    }

    fn peripheral(id: u8) -> PeripheralId {
        PeripheralId(id.to_string())
    }

    fn sha256(bytes: &[u8]) -> [u8; 32] {
        use sha2::{Digest as _, Sha256};

        Sha256::digest(bytes).into()
    }

    #[test]
    fn a_file_backed_node_puts_blobs_in_the_blob_directory() {
        let dir = TempDir::new("node");
        let node = Node::open(db(), SecretKey::generate(), &dir.0).unwrap();

        // The store the node hands its sessions is file-backed, under `<directory>/blobs`.
        let bytes = b"the quick brown fox";
        let hash = crate::model::BlobHash::parse(&hex::encode(sha256(bytes))).unwrap();

        node.blobs.append(&hash, bytes).unwrap();
        assert!(node.blobs.has(&hash).unwrap());
        assert_eq!(node.blobs.read(&hash, 0, 100).unwrap(), bytes);
        assert!(dir.0.join("blobs").join(hash.as_str()).is_file());
    }

    /// Publish an event anchoring `bytes`, which is what puts them in the store.
    fn given_held(node: &mut Node, bytes: &[u8]) -> BlobHash {
        let hash = BlobHash::digest(bytes);
        let event = note(
            author(1),
            100,
            "with media",
            Tags::new().add("imeta", [format!("x {hash}")]),
        );

        clock::at(1_000, || node.publish(&event, &[bytes])).unwrap();

        hash
    }

    /// The author's end of a verified transfer: the signed tag carries the root and publishing writes the tree.
    #[test]
    fn media_the_user_publishes_carries_a_root_and_is_held_whole() {
        let mut node = node();
        let bytes = b"the quick brown fox";
        let imeta = Node::media_tags(bytes);
        let hash = BlobHash::digest(bytes);

        assert_eq!(imeta[0], format!("x {hash}"));
        assert_eq!(imeta[1], "size 19");
        assert!(imeta[2].starts_with("blake3 "));

        // Describing the bytes stores nothing: a composition abandoned here leaves the store as it was.
        assert!(!node.blobs.has(&hash).unwrap());

        node.publish(
            &note(
                author(1),
                100,
                "with media",
                Tags::new().add("imeta", imeta),
            ),
            &[bytes],
        )
        .unwrap();

        assert!(node.blobs.outboard_len(&hash).unwrap().is_some());
        assert!(query::get_blob(&node.db, &hash).unwrap().unwrap().complete);
        assert!(query::wanted_blobs(&node.db, 10).unwrap().is_empty());
    }

    /// The sweep at open takes every file the table does not know, so the device's own media has to be in it.
    #[test]
    fn own_media_survives_the_sweep_at_open() {
        let db = db();
        let blobs: Arc<dyn BlobStore> = Arc::new(crate::blobs::MemoryBlobStore::default());
        let bytes = b"the quick brown fox";
        let imeta = Node::media_tags(bytes);
        let mut node = Node::new(Arc::clone(&db), secret(1), Arc::clone(&blobs)).unwrap();

        node.publish(
            &note(
                author(1),
                100,
                "with media",
                Tags::new().add("imeta", imeta),
            ),
            &[bytes],
        )
        .unwrap();

        Node::new(db, secret(1), Arc::clone(&blobs)).unwrap();

        assert!(blobs.has(&BlobHash::digest(bytes)).unwrap());
    }

    /// Bytes with no tag naming them would be a file no row keeps and no peer asks for.
    #[test]
    fn publishing_media_the_event_does_not_attach_is_refused() {
        let mut node = node();
        let event = note(author(1), 100, "no media at all", Tags::new());

        assert!(node.publish(&event, &[b"unannounced"]).is_err());
        assert!(!node.blobs.has(&BlobHash::digest(b"unannounced")).unwrap());
    }

    #[test]
    fn deleting_an_event_deletes_the_bytes_of_the_blob_it_anchored() {
        let mut node = node();
        let hash = given_held(&mut node, b"media");

        db_command::forget_events_unseen_since(&node.db, &node.identity(), 2_000).unwrap();

        // Nothing is reclaimed until the node drains the channel the removal was announced on.
        assert!(node.blobs.has(&hash).unwrap());

        node.tick();

        assert!(!node.blobs.has(&hash).unwrap());
    }

    /// The retention window has a caller: time passing is the whole prompt.
    #[test]
    fn ticking_forgets_what_stopped_circulating() {
        let db = db();
        let mut node = clock::at(1_000, || {
            Node::new(
                Arc::clone(&db),
                SecretKey::generate(),
                Arc::new(crate::blobs::MemoryBlobStore::default()),
            )
            .unwrap()
        });

        let carried = note(author(1), 100, "nobody passed it on", Tags::new());

        db_command::receive_event(&db, &carried, &[author(200)], 1_000).unwrap();

        let past_the_window = 1_000 + i64::from(node.policy.retention_days) * 86_400 + 1;

        clock::at(past_the_window, || node.tick());

        assert!(query::get_event(&db, &carried.id).unwrap().is_none());
    }

    /// A removal announced while no node was listening is still delivered, by
    /// the store being reconciled against the table rather than by the channel.
    #[test]
    fn a_file_no_record_references_is_swept_at_open() {
        let db = db();
        let blobs = Arc::new(crate::blobs::MemoryBlobStore::default());

        let mut node = Node::new(Arc::clone(&db), SecretKey::generate(), blobs.clone()).unwrap();
        let partial = given_held(&mut node, b"half a transfer");
        let orphan = BlobHash::digest(b"nothing references this");

        blobs.append(&orphan, b"nothing references this").unwrap();

        drop(node);
        Node::new(db, SecretKey::generate(), blobs.clone()).unwrap();

        assert!(!blobs.has(&orphan).unwrap());
        // A transfer in flight has a record from the moment its event was stored.
        assert!(blobs.has(&partial).unwrap());
    }

    #[test]
    fn a_candidate_above_the_floor_is_dialed() {
        let mut node = node();

        let actions = node.peripheral_seen(&peripheral(1), -80);

        assert_eq!(actions, vec![Action::Connect(peripheral(1))]);
    }

    #[test]
    fn a_candidate_below_the_floor_is_queued_until_the_reading_recovers() {
        let mut node = node();

        assert!(node.peripheral_seen(&peripheral(1), -100).is_empty());
        assert!(node.peripheral_seen(&peripheral(1), -95).is_empty());

        // A reading above the floor lets the queued candidate out.
        let actions = node.peripheral_seen(&peripheral(1), -85);
        assert_eq!(actions, vec![Action::Connect(peripheral(1))]);
    }

    #[test]
    fn attempts_are_rate_limited() {
        let mut node = clock::at(1_000, node);

        clock::at(1_000, || {
            assert_eq!(
                node.peripheral_seen(&peripheral(1), -80),
                vec![Action::Connect(peripheral(1))]
            );
            assert!(node.peripheral_seen(&peripheral(2), -80).is_empty());
        });

        // A second later the rate limit has cleared.
        clock::at(1_001, || {
            assert_eq!(
                node.peripheral_seen(&peripheral(2), -80),
                vec![Action::Connect(peripheral(2))]
            );
        });
    }

    #[test]
    fn a_peer_ignoring_connects_is_backed_off() {
        let mut node = clock::at(1_000, node);

        clock::at(1_000, || {
            assert_eq!(
                node.peripheral_seen(&peripheral(1), -80),
                vec![Action::Connect(peripheral(1))]
            );
        });

        // The dial went unanswered; a fresh advertisement is held until the backoff lapses.
        clock::at(1_005, || {
            assert!(node.peripheral_seen(&peripheral(1), -80).is_empty());
        });

        clock::at(1_000 + NEVER_ANSWERED_BACKOFF_SECONDS, || {
            assert_eq!(
                node.peripheral_seen(&peripheral(1), -80),
                vec![Action::Connect(peripheral(1))]
            );
        });
    }

    #[test]
    fn the_link_cap_queues_candidates_beyond_max_links() {
        // Fill the cap with links this device dialed.
        let mut node = node();

        for index in 0..MAX_LINKS {
            let link = LinkId(index as u64);
            node.link_up(link, Some(peripheral(index as u8)), Role::Dialer, 100)
                .unwrap();
        }

        let actions = node.peripheral_seen(&peripheral(200), -80);

        assert!(actions.is_empty(), "a dial slipped past the link cap");
    }

    #[test]
    fn a_link_the_core_closed_frees_its_slot_without_a_disconnect_report() {
        // The shell may never report back, so the slot returns when the session goes.
        let mut node = node();

        for index in 0..MAX_LINKS {
            let link = LinkId(index as u64);
            node.link_up(link, Some(peripheral(index as u8)), Role::Dialer, 100)
                .unwrap();

            // Garbage off the characteristic: the core drops the link.
            let actions = node.bytes_received(link, &[0xff; 16]);
            assert!(actions.contains(&Action::Disconnect(link)));
        }

        let actions = node.peripheral_seen(&peripheral(200), -80);

        assert_eq!(actions, vec![Action::Connect(peripheral(200))]);
    }

    #[test]
    fn inbound_links_do_not_spend_the_central_cap() {
        // The doc caps *central* links; counting inbound ones would self-silence in a crowd.
        let mut node = node();

        for index in 0..MAX_LINKS {
            node.link_up(LinkId(index as u64), None, Role::Receiver, 100)
                .unwrap();
        }

        let actions = node.peripheral_seen(&peripheral(1), -80);

        assert_eq!(actions, vec![Action::Connect(peripheral(1))]);
    }

    #[test]
    fn publish_stores_and_offers_via_the_channel() {
        let db = db();
        let mut node = Node::new(
            Arc::clone(&db),
            secret(1),
            Arc::new(crate::blobs::MemoryBlobStore::default()),
        )
        .unwrap();
        let event = note(author(1), 100, "hello", Tags::new());

        node.publish(&event, &[]).unwrap();

        let stored = query::list_events(&db, &Query::new()).unwrap();
        assert_eq!(stored, vec![event]);
    }

    #[test]
    fn a_saved_own_event_reaches_a_subscribed_peer() {
        let db = db();
        // Node 1 authors, and a peer's session is attached to the same node.
        let mut node = Node::new(
            Arc::clone(&db),
            secret(1),
            Arc::new(crate::blobs::MemoryBlobStore::default()),
        )
        .unwrap();
        let event = note(author(1), 100, "hello", Tags::new());

        // Everything a session writes is sealed, so reading one back takes its peer.
        let policy = Arc::new(Policy::new(author(1)));
        let mut session = Session::open(
            LinkId(2),
            Role::Receiver,
            4096,
            policy,
            secret(2),
            Arc::new(crate::blobs::MemoryBlobStore::default()),
            spending(),
        )
        .unwrap();
        let mut peer = Session::open(
            LinkId(3),
            Role::Dialer,
            4096,
            Arc::new(Policy::new(author(2))),
            secret(3),
            Arc::new(crate::blobs::MemoryBlobStore::default()),
            spending(),
        )
        .unwrap();

        handshake(&mut peer, &mut session, &db);
        session.identify([author(2)]);

        let req = Message::Req(
            SubscriptionId("sub".into()),
            vec![Filter::new().add_kinds([1])],
        );
        let frame = Frame {
            channel: Channel::Sync,
            payload: req.encode(),
        };
        session.handle_sync(&db, &frame).unwrap();
        node.sessions.insert(LinkId(2), session);

        // Publishing the own event flows through the channel into the offer.
        let first = node.publish(&event, &[]).unwrap();

        // The REQ serve's own EOSE was in flight; write_complete releases what queued behind.
        assert!(
            first
                .iter()
                .any(|action| matches!(action, Action::Send(LinkId(2), _)))
        );
        let released = node.write_complete(LinkId(2));

        // Both batches arrive in the order the shell wrote them, which is the seal order.
        let mut arrived = Vec::new();

        for action in first.into_iter().chain(released) {
            if let Action::Send(LinkId(2), fragment) = action
                && let Some(frame) = peer.receive(&fragment).unwrap()
            {
                arrived.push(frame);
            }
        }

        // The Event carrying the note itself reaches the peer's link.
        let pushed = arrived
            .iter()
            .find_map(|frame| match Message::decode(&frame.payload) {
                Ok(Message::Event(subscription, carried)) => Some((subscription, *carried)),
                _ => None,
            })
            .expect("the note reached the peer's link");

        assert_eq!(pushed.0, SubscriptionId("sub".into()));
        assert_eq!(pushed.1, event);
    }

    /// Run the Noise handshake between two sessions, leaving both wires
    /// secured and the peer holding the other end of the cipher.
    fn handshake(dialer: &mut Session, receiver: &mut Session, db: &Db) {
        dialer.initiate().unwrap();

        // One fragment per handshake message at this MTU.
        let opening = dialer.next_write().unwrap().unwrap();
        dialer.acknowledge_write();
        let frame = receiver.receive(&opening).unwrap().unwrap();
        receiver.advance(db, &frame).unwrap();

        let reply = receiver.next_write().unwrap().unwrap();
        receiver.acknowledge_write();
        let frame = dialer.receive(&reply).unwrap().unwrap();
        dialer.advance(db, &frame).unwrap();

        let closing = dialer.next_write().unwrap().unwrap();
        dialer.acknowledge_write();
        let frame = receiver.receive(&closing).unwrap().unwrap();
        receiver.advance(db, &frame).unwrap();
    }

    #[test]
    fn a_reported_disconnect_closes_the_session() {
        let mut node = node();
        let policy = Arc::new(Policy::new(author(1)));
        node.sessions.insert(
            LinkId(1),
            Session::open(
                LinkId(1),
                Role::Receiver,
                100,
                policy,
                SecretKey::generate(),
                Arc::new(crate::blobs::MemoryBlobStore::default()),
                spending(),
            )
            .unwrap(),
        );

        let actions = node.link_down(LinkId(1));

        // The link is gone and the session with it; a redundant Disconnect is the only action.
        assert!(matches!(
            actions.as_slice(),
            [Action::Disconnect(LinkId(1))]
        ));
        assert!(node.sessions.is_empty());
    }

    #[test]
    fn a_second_link_up_for_a_live_link_is_an_error() {
        let mut node = node();

        node.link_up(LinkId(1), None, Role::Receiver, 100).unwrap();
        assert!(node.link_up(LinkId(1), None, Role::Dialer, 100).is_err());
    }

    #[test]
    fn a_walked_away_peer_is_redialed_sooner_than_a_silent_one() {
        let mut node = clock::at(1_000, node);
        let silent = peripheral(1);
        let walker = peripheral(2);

        // Both are dialed, a second apart.
        clock::at(1_000, || {
            assert_eq!(
                node.peripheral_seen(&silent, -80),
                vec![Action::Connect(silent.clone())]
            );
        });
        clock::at(1_001, || {
            assert_eq!(
                node.peripheral_seen(&walker, -80),
                vec![Action::Connect(walker.clone())]
            );
        });

        // The walker connects and leaves at once; the silent one never answers.
        clock::at(1_001, || {
            node.link_up(LinkId(9), Some(walker.clone()), Role::Dialer, 100)
                .unwrap();
            node.link_down(LinkId(9));
        });

        // The walker is admissible at 1001 + WALKED_AWAY_BACKOFF_SECONDS, set at 1001.
        clock::at(1_001 + WALKED_AWAY_BACKOFF_SECONDS, || {
            assert_eq!(
                node.peripheral_seen(&walker, -80),
                vec![Action::Connect(walker.clone())]
            );
        });
        // …while the silent one waits out the never-answered backoff, set at 1000.
        clock::at(1_001 + WALKED_AWAY_BACKOFF_SECONDS + 1, || {
            assert!(node.peripheral_seen(&silent, -80).is_empty());
        });
        clock::at(1_000 + NEVER_ANSWERED_BACKOFF_SECONDS, || {
            assert_eq!(
                node.peripheral_seen(&silent, -80),
                vec![Action::Connect(silent)]
            );
        });
    }

    #[test]
    fn a_peer_this_device_dropped_is_not_redialed_at_walk_away_speed() {
        let mut node = clock::at(1_000, node);
        let peer = peripheral(1);

        clock::at(1_000, || {
            assert_eq!(
                node.peripheral_seen(&peer, -80),
                vec![Action::Connect(peer.clone())]
            );
        });

        // The peer sends something the wire cannot carry, so this device drops the link.
        clock::at(1_030, || {
            node.link_up(LinkId(9), Some(peer.clone()), Role::Dialer, 100)
                .unwrap();
            node.bytes_received(LinkId(9), b"not a fragment");
            node.link_down(LinkId(9));
        });

        // Graded as a walk-away this would be dialable at 1045, and as never-answered at 1060.
        clock::at(1_060, || {
            assert!(node.peripheral_seen(&peer, -80).is_empty());
        });
        clock::at(1_030 + REFUSED_BACKOFF_SECONDS, || {
            assert_eq!(
                node.peripheral_seen(&peer, -80),
                vec![Action::Connect(peer)]
            );
        });
    }

    #[test]
    fn a_declined_peer_is_backed_off_longer_than_a_walked_away_one() {
        let mut node = clock::at(1_000, node);
        let peer = peripheral(1);

        clock::at(1_000, || {
            node.peripheral_seen(&peer, -80);
        });
        clock::at(1_000, || {
            node.link_up(LinkId(9), Some(peer.clone()), Role::Dialer, 100)
                .unwrap();
            node.approve(LinkId(9), false).unwrap();
        });

        // A decline outlasts the walked-away tier even if the report arrives after it.
        clock::at(1_000, || {
            node.link_down(LinkId(9));
        });

        clock::at(1_000 + WALKED_AWAY_BACKOFF_SECONDS + 1, || {
            assert!(node.peripheral_seen(&peer, -80).is_empty());
        });
        clock::at(1_000 + DECLINED_BACKOFF_SECONDS, || {
            assert_eq!(
                node.peripheral_seen(&peer, -80),
                vec![Action::Connect(peer)]
            );
        });
    }
}
