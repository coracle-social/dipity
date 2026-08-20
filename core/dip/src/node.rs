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

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, bail};
use coracle_lib::events::HashedEvent;
use coracle_lib::keys::SecretKey;
use tokio::sync::broadcast::{self, error::TryRecvError};

use crate::blobstore::{BlobStore, FileBlobStore};
use crate::clock;
use crate::db::command as db_command;
use crate::db::event::channel::{self, EventChange};
use crate::db::{Db, query};
use crate::link::{LinkId, PeripheralId, Role};
use crate::model::Policy;
use crate::session::{Session, State};
use crate::transport::Channel;

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

/// Signal floor below which a candidate is queued rather than dialed.
pub const RSSI_FLOOR: i16 = -90;

/// How many central links may be open at once.
pub const MAX_LINKS: usize = 6;

/// Minimum gap between connect attempts. The doc calls for roughly one per
/// 0.5s; the clock counts in whole seconds, so one per second.
const CONNECT_INTERVAL_SECONDS: i64 = 1;

/// How long a peripheral is left alone after a dial, so a peer that ignores
/// connections is not redialed every advertisement.
/// How long a peripheral that never answered a connect is left alone before
/// its advertisement may be dialed again.
const NEVER_ANSWERED_BACKOFF_SECONDS: i64 = 60;

/// How long after a connected peer walks away before redialing. They usually
/// come back, so this is short. `docs/discovery.md#connection-scheduling`.
const WALKED_AWAY_BACKOFF_SECONDS: i64 = 15;

/// How long after a consent-gate refusal before trying again. Hard, since the
/// user just said no.
const DECLINED_BACKOFF_SECONDS: i64 = 5 * 60;

/// Everything scheduling dials from an advertisement.
#[derive(Debug, Default)]
struct Scheduler {
    /// Candidates queued by the floor, the link cap, or the rate limit, most
    /// recently seen first.
    candidates: Vec<(PeripheralId, i16)>,
    /// When each peripheral may next be dialed, and what set it. The tier
    /// decides how a later outcome may move it.
    backoff: BTreeMap<PeripheralId, Backoff>,
    /// When the last connect attempt went out, for the global rate limit.
    last_attempt: Option<i64>,
}

/// One peripheral's backoff: a deadline and the outcome that set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Backoff {
    /// The earliest this peripheral may be dialed again.
    until: i64,
    /// Why it is backed off.
    tier: Tier,
}

/// What set a backoff, and how far out it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    /// A dial that never answered.
    NeverAnswered,
    /// A peer that was connected and walked away.
    WalkedAway,
    /// A peer the user declined at the consent gate.
    Declined,
}

impl Scheduler {
    /// A peripheral was seen; try to dial it, or queue it and say nothing.
    fn seen(&mut self, peripheral: PeripheralId, rssi: i16) -> Option<PeripheralId> {
        self.queue(peripheral, rssi);

        self.poll()
    }

    /// Make room for a candidate, keeping the best RSSI.
    fn queue(&mut self, peripheral: PeripheralId, rssi: i16) {
        match self
            .candidates
            .iter()
            .position(|(candidate, _)| *candidate == peripheral)
        {
            Some(index) => {
                // A stronger reading replaces the queued one.
                if self.candidates[index].1 < rssi {
                    self.candidates[index] = (peripheral, rssi);
                }
            }
            None => {
                self.candidates.push((peripheral, rssi));
                self.candidates
                    .sort_by_key(|(_, rssi)| std::cmp::Reverse(*rssi));
            }
        }
    }

    /// Dial the strongest admissible candidate, if any.
    fn poll(&mut self) -> Option<PeripheralId> {
        let now = clock::now();

        if self
            .last_attempt
            .is_some_and(|at| now - at < CONNECT_INTERVAL_SECONDS)
        {
            return None;
        }

        // Walk the queue most-recently-seen-but-strongest first; the first
        // candidate not under its backoff is dialed, and candidates under it
        // stay queued for a later tick.
        for index in 0..self.candidates.len() {
            let (peripheral, _) = self.candidates[index].clone();

            let admissible = match self.backoff.get(&peripheral) {
                Some(backoff) => now >= backoff.until,
                None => true,
            };

            if admissible {
                self.candidates.remove(index);
                self.last_attempt = Some(now);
                // A dial that gets no answer shows up again as an
                // advertisement before this lapses; a dial that connects
                // supersedes it with the walked-away or declined tier.
                self.backoff.insert(
                    peripheral.clone(),
                    Backoff {
                        until: now + NEVER_ANSWERED_BACKOFF_SECONDS,
                        tier: Tier::NeverAnswered,
                    },
                );

                return Some(peripheral);
            }
        }

        None
    }

    /// A peer was connected and left: redial soon, since they usually come
    /// back. Pulls in a stale never-answered backoff — this one answered, just
    /// not for long — but never a decline.
    fn walked_away(&mut self, peripheral: &PeripheralId) {
        self.record(peripheral, Tier::WalkedAway, WALKED_AWAY_BACKOFF_SECONDS);
    }

    /// A peer whose gate was refused: leave them alone. Never shortened by any
    /// later report.
    fn declined(&mut self, peripheral: &PeripheralId) {
        self.record(peripheral, Tier::Declined, DECLINED_BACKOFF_SECONDS);
    }

    /// Merge one outcome into a peripheral's backoff.
    ///
    /// A shorter tier (never answered → walked away) replaces the longer
    /// deadline; a longer one (walked away → declined) never shrinks, so the
    /// user's "no" survives a disconnect report arriving afterwards.
    fn record(&mut self, peripheral: &PeripheralId, tier: Tier, seconds: i64) {
        let deadline = clock::now() + seconds;

        match self.backoff.get_mut(peripheral) {
            Some(backoff) => {
                let replace = match (backoff.tier, tier) {
                    (Tier::NeverAnswered, Tier::WalkedAway) => true,
                    (Tier::NeverAnswered, Tier::Declined) => true,
                    (Tier::WalkedAway, Tier::Declined) => true,
                    (Tier::Declined, _) => false,
                    _ => backoff.until < deadline,
                };

                if replace {
                    // Only the walked-away pull-in shortens what a dial left
                    // behind; every other merge extends.
                    backoff.until = if matches!(
                        (backoff.tier, tier),
                        (Tier::NeverAnswered, Tier::WalkedAway)
                    ) {
                        deadline
                    } else {
                        deadline.max(backoff.until)
                    };
                    backoff.tier = tier;
                }
            }
            None => {
                self.backoff.insert(
                    peripheral.clone(),
                    Backoff {
                        until: deadline,
                        tier,
                    },
                );
            }
        }
    }
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
    /// The peripheral each dialed link came from, for grading its outcome.
    link_peripheral: BTreeMap<LinkId, PeripheralId>,
    /// Decides which advertised peers to dial, and when.
    scheduler: Scheduler,
    /// When the app was last foregrounded, for the cool-off admission window.
    cool_off_since: Option<i64>,
    /// This store's event channel, so a stored own event is offered to every
    /// connected peer without the writer knowing.
    events: broadcast::Receiver<EventChange>,
}

impl Node {
    /// Build a node over a store the shell has opened and a key it has read out
    /// of the Keychain or Keystore, with the blob store of the shell's choosing.
    pub fn new(db: Arc<Db>, identity: SecretKey, blobs: Arc<dyn BlobStore>) -> Result<Self> {
        let policy = Arc::new(query::policy(&db, &identity.public_key())?);

        Ok(Self {
            events: channel::subscribe(&db),
            db,
            identity,
            policy,
            blobs,
            battery: None,
            sessions: BTreeMap::new(),
            link_peripheral: BTreeMap::new(),
            scheduler: Scheduler::default(),
            cool_off_since: None,
        })
    }

    /// Build a node with the file-backed blob store, under the same directory
    /// the shell already tells the core to open the database in. Blobs live in
    /// `directory/blobs`, one file per hash.
    pub fn open(db: Arc<Db>, identity: SecretKey, directory: impl AsRef<Path>) -> Result<Self> {
        let blobs = Arc::new(FileBlobStore::open(directory.as_ref().join("blobs"))?);

        Self::new(db, identity, blobs)
    }

    /// The store, for the view and for tests.
    #[must_use]
    pub fn db(&self) -> &Arc<Db> {
        &self.db
    }

    /// This device's pubkey.
    #[must_use]
    pub fn identity(&self) -> coracle_lib::keys::PublicKey {
        self.identity.public_key()
    }

    // ========================================================================
    // Radio events, all from the shell
    // ========================================================================

    /// A peripheral advertising our service UUID was seen.
    ///
    /// Whether to dial is the core's call; the shell keeps the radio.
    /// `docs/discovery.md#connection-scheduling`.
    pub fn peripheral_seen(&mut self, peripheral: &PeripheralId, rssi: i16) -> Vec<Action> {
        let mut actions = Vec::new();

        if rssi < RSSI_FLOOR || self.sessions.len() >= MAX_LINKS {
            // Queued, not dialed: still worth the scheduler remembering.
            self.scheduler.queue(peripheral.clone(), rssi);
        } else if let Some(peripheral) = self.scheduler.seen(peripheral.clone(), rssi) {
            actions.push(Action::Connect(peripheral));
        }

        actions
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
        )?;
        session.set_cool_off_since(self.cool_off_since);

        if let Some(peripheral) = peripheral {
            self.link_peripheral.insert(link, peripheral);
        }

        if role == Role::Dialer {
            session.initiate()?;
        }

        self.sessions.insert(link, session);

        Ok(self.collect())
    }

    /// A link went away. The shell only reports a disconnect that fired, which is
    /// reliable: the link is gone and nothing on it will ever finish, so the
    /// session closes rather than waiting out a drain that cannot complete.
    ///
    /// A peer that walked away is redialed soon: they usually come back.
    pub fn link_down(&mut self, link: LinkId) -> Vec<Action> {
        if let Some(session) = self.sessions.get_mut(&link) {
            session.close();
        }

        if let Some(peripheral) = self.link_peripheral.remove(&link) {
            self.scheduler.walked_away(&peripheral);
        }

        self.collect()
    }

    /// One write arrived off the characteristic.
    ///
    /// Reassembles, decrypts, and hands the frame to the session or to
    /// [`crate::sync`] by channel. A malformed or unauthenticated frame ends
    /// the link rather than being dropped.
    pub fn bytes_received(&mut self, link: LinkId, write: &[u8]) -> Vec<Action> {
        let result = self.receive_frame(link, write);

        match result {
            Ok(()) => self.collect(),
            Err(_) => vec![Action::Disconnect(link)],
        }
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
                match session.state() {
                    // A gate hold and a drain both close outright on their cap:
                    // there is nothing in flight to finish, only to drop.
                    State::GatePending => session.close(),
                    State::Draining => session.close(),
                    _ => session.drain(),
                }
            }
        }

        // A queued candidate may now be past its rate limit or backoff.
        let mut actions = Vec::new();
        if let Some(peripheral) = self.scheduler.poll() {
            actions.push(Action::Connect(peripheral));
        }

        actions.extend(self.collect());
        actions
    }

    // ========================================================================
    // The view, through the shell
    // ========================================================================

    /// The battery level, percent, as the shell reports it. Blob transfers are
    /// gated on it.
    pub fn battery(&mut self, level: u8) -> Vec<Action> {
        self.battery = Some(level);

        for session in self.sessions.values_mut() {
            session.set_battery(Some(level));
        }

        self.collect()
    }

    /// The app came to the foreground, which starts the cool-off admission
    /// window for unknown peers.
    pub fn notify_foregrounded(&mut self) -> Vec<Action> {
        self.cool_off_since = Some(clock::now());

        for session in self.sessions.values_mut() {
            session.set_cool_off_since(self.cool_off_since);
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

    /// Store an event the user wrote. The event channel does the offering: what
    /// this write announces is drained in `collect`, so a note written in a
    /// crowd propagates while the crowd is still there.
    pub fn publish(&mut self, event: &HashedEvent) -> Result<Vec<Action>> {
        db_command::publish_event(&self.db, event, &self.identity.public_key(), clock::now())?;

        Ok(self.collect())
    }

    // ========================================================================
    // Draining what the sessions produced
    // ========================================================================

    /// Sweep every session for work: fragments to write, links to tear down,
    /// and the earliest deadline worth waking for.
    ///
    /// The one place actions are produced, so an entry point cannot forget to
    /// flush a session it advanced.
    fn collect(&mut self) -> Vec<Action> {
        self.offer_saved_events();

        // Heartbeats go out before writes are drained, so a quiet session
        // still proves it is alive within its jittered interval.
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
            while let Some(fragment) = session.next_write() {
                actions.push(Action::Send(session.link, fragment));
            }
        }

        let closed: Vec<LinkId> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.state() == State::Closed)
            .map(|(link, _)| *link)
            .collect();

        for link in closed {
            self.sessions.remove(&link);
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
                        if let Err(error) = session.offer_event(&event) {
                            log::error!(
                                "offering a saved event on link {:?} failed: {error:#}",
                                session.link
                            );
                        }
                    }
                }
                // Seen events are not newly stored, and Deleted are gone. Both
                // are for the view.
                Ok(_) => {}
                // A lag tells the subscriber it missed changes; the next
                // reconciliation covers them.
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                Err(TryRecvError::Lagged(_)) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::tags::Tags;

    use crate::fixtures::{author, note, secret};
    use crate::model::{Policy, Query};
    use crate::session::Session;
    use crate::sync::{Message, SubscriptionId};
    use crate::transport::Frame;
    use coracle_lib::filters::Filter;

    fn node() -> Node {
        Node::new(
            db(),
            SecretKey::generate(),
            Arc::new(crate::blobstore::MemoryBlobStore::default()),
        )
        .unwrap()
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

    /// A self-cleaning directory under the system temp dir.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "dip-node-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));

            std::fs::create_dir_all(&dir).unwrap();

            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_file_backed_node_puts_blobs_in_the_blob_directory() {
        let dir = TempDir::new();
        let node = Node::open(db(), SecretKey::generate(), &dir.0).unwrap();

        // The store the node hands its sessions is the file-backed one: what
        // it writes shows up as a file under `<directory>/blobs`, next to the
        // database file the shell already gives the core.
        let bytes = b"the quick brown fox";
        let hash = hex::encode(sha256(bytes));

        node.blobs.append(&hash, bytes).unwrap();
        assert!(node.blobs.has(&hash).unwrap());
        assert_eq!(node.blobs.read(&hash, 0, 100).unwrap(), bytes);
        assert!(dir.0.join("blobs").join(&hash).is_file());
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

        // The dial went unanswered; a fresh advertisement is held until the
        // backoff lapses.
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
        // Fill the session table to the cap.
        let mut node = node();
        let policy = Arc::new(Policy::new(author(1)));

        for index in 0..MAX_LINKS {
            node.sessions.insert(
                LinkId(index as u64),
                Session::open(
                    LinkId(index as u64),
                    Role::Receiver,
                    100,
                    Arc::clone(&policy),
                    SecretKey::generate(),
                    Arc::new(crate::blobstore::MemoryBlobStore::default()),
                )
                .unwrap(),
            );
        }

        let actions = node.peripheral_seen(&peripheral(1), -80);

        assert!(actions.is_empty(), "a dial slipped past the link cap");
    }

    #[test]
    fn publish_stores_and_offers_via_the_channel() {
        let db = db();
        let mut node = Node::new(
            Arc::clone(&db),
            secret(1),
            Arc::new(crate::blobstore::MemoryBlobStore::default()),
        )
        .unwrap();
        let event = note(author(1), 100, "hello", Tags::new());

        node.publish(&event).unwrap();

        let stored = query::list_events(&db, &Query::new()).unwrap();
        assert_eq!(stored, vec![event]);
    }

    #[test]
    fn a_saved_own_event_reaches_a_subscribed_peer() {
        let db = db();
        // Node 1 authors; a peer's session is attached to the same node (as in
        // a single device with a live link).
        let mut node = Node::new(
            Arc::clone(&db),
            secret(1),
            Arc::new(crate::blobstore::MemoryBlobStore::default()),
        )
        .unwrap();
        let event = note(author(1), 100, "hello", Tags::new());

        // A session that goes through the real REQ path.
        let policy = Arc::new(Policy::new(author(1)));
        let mut session = Session::open(
            LinkId(2),
            Role::Receiver,
            4096,
            policy,
            secret(2),
            Arc::new(crate::blobstore::MemoryBlobStore::default()),
        )
        .unwrap();
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
        let first = node.publish(&event).unwrap();

        // The REQ serve's own EOSE was in flight; the shell's write_complete
        // acknowledgment releases what the offer queued behind it.
        assert!(
            first
                .iter()
                .any(|action| matches!(action, Action::Send(LinkId(2), _)))
        );
        let actions = node.write_complete(LinkId(2));

        // The Event carrying the note itself reaches the peer's link.
        let pushed = actions
            .iter()
            .find_map(|action| match action {
                Action::Send(link, fragment) if *link == LinkId(2) => {
                    // No Noise handshake ran in this test, so the fragment is
                    // plaintext: two header bytes then the sync message.
                    match Message::decode(&fragment[2..]) {
                        Ok(Message::Event(subscription, carried)) => Some((subscription, *carried)),
                        _ => None,
                    }
                }
                _ => None,
            })
            .expect("the note reached the peer's link");

        assert_eq!(pushed.0, SubscriptionId("sub".into()));
        assert_eq!(pushed.1, event);
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
                Arc::new(crate::blobstore::MemoryBlobStore::default()),
            )
            .unwrap(),
        );

        let actions = node.link_down(LinkId(1));

        // The link is gone and the session with it; a redundant Disconnect is
        // the only action, and the session no longer occupies the table.
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

        // The walker is admissible at the short walked-away backoff… which was set
        // at 1001, so the deadline is 1001 + WALKED_AWAY_BACKOFF_SECONDS.
        clock::at(1_001 + WALKED_AWAY_BACKOFF_SECONDS, || {
            assert_eq!(
                node.peripheral_seen(&walker, -80),
                vec![Action::Connect(walker.clone())]
            );
        });
        // …while the silent one must wait out the never-answered backoff,
        // which was set at 1000.
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

        // A decline outlasts the walked-away tier even if the disconnect
        // report arrives after the refusal.
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
