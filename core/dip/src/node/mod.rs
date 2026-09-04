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

use crate::blobs::{BlobStore, FileBlobStore};
use crate::clock;
use crate::db::command as db_command;
use crate::db::event::channel::{self, EventChange};
use crate::db::{Db, query};
use crate::link::{LinkId, PeripheralId, Role};
use crate::model::Policy;
use crate::session::gate::Presence;
use crate::session::{Ending, Session, State};
use crate::transport::Channel;
use scheduler::Scheduler;

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
            spending: Arc::new(crate::sync::spending::SpendingLedger::default()),
            presence: None,
        })
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

    // ========================================================================
    // Radio events, all from the shell
    // ========================================================================

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

        // Close rather than only telling the shell to disconnect. The session
        // is finished either way, and one left in the table would keep its
        // link-cap slot and go on being swept for heartbeats until the shell
        // reported the disconnect back.
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
                    // A gate hold and a drain both close outright on their cap:
                    // there is nothing in flight to finish, only to drop. A
                    // lapsed hold is this device's decision, a spent drain the
                    // peer already gone.
                    State::GatePending { .. } => session.close(Ending::Refused),
                    State::Draining { .. } => session.close(Ending::WalkedAway),
                    _ => session.drain(),
                }
            }
        }

        // A queued candidate may now be past its rate limit, its backoff, or
        // the link cap.
        let mut actions = Vec::new();
        if let Some(peripheral) = self.scheduler.next_dial(self.central_links()) {
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
            loop {
                match session.next_write() {
                    Ok(Some(fragment)) => actions.push(Action::Send(session.link, fragment)),
                    Ok(None) => break,
                    // Sealing a fragment fails only on a wire that can no
                    // longer carry the session, so nothing more goes out on it.
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

        // Every teardown passes through here, so grading is one decision. In
        // `link_down` a link this device dropped read as a peer walking away.
        // Taking the peripheral is what frees the link-cap slot: the shell owes
        // no disconnect report for a teardown the core decided.
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
    use super::scheduler::{
        DECLINED_BACKOFF_SECONDS, NEVER_ANSWERED_BACKOFF_SECONDS, REFUSED_BACKOFF_SECONDS,
        WALKED_AWAY_BACKOFF_SECONDS,
    };
    use super::*;
    use coracle_lib::tags::Tags;

    use crate::fixtures::{TempDir, author, note, secret};
    use crate::model::{Policy, Query};
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

        // The store the node hands its sessions is the file-backed one: what
        // it writes shows up as a file under `<directory>/blobs`, next to the
        // database file the shell already gives the core.
        let bytes = b"the quick brown fox";
        let hash = crate::model::BlobHash::parse(&hex::encode(sha256(bytes))).unwrap();

        node.blobs.append(&hash, bytes).unwrap();
        assert!(node.blobs.has(&hash).unwrap());
        assert_eq!(node.blobs.read(&hash, 0, 100).unwrap(), bytes);
        assert!(dir.0.join("blobs").join(hash.as_str()).is_file());
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
        // The shell is told to disconnect and may never report back, so the
        // slot has to come back when the session goes, not when the radio
        // says so.
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
        // The doc caps *central* links. Counting peers that dialed us against
        // it means six inbound connections stop this device dialing anyone —
        // self-silencing in exactly the crowd it exists for.
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
            Arc::new(crate::blobs::MemoryBlobStore::default()),
        )
        .unwrap();
        let event = note(author(1), 100, "hello", Tags::new());

        // A session that goes through the real REQ path, and the peer at the
        // other end of its cipher: everything it writes is sealed, so reading
        // one back takes the device it was sealed for.
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
        let first = node.publish(&event).unwrap();

        // The REQ serve's own EOSE was in flight; the shell's write_complete
        // acknowledgment releases what the offer queued behind it.
        assert!(
            first
                .iter()
                .any(|action| matches!(action, Action::Send(LinkId(2), _)))
        );
        let released = node.write_complete(LinkId(2));

        // Both batches go into the peer in the order the shell would have
        // written them, which is the order the cipher sealed them in.
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
    fn a_peer_this_device_dropped_is_not_redialed_at_walk_away_speed() {
        let mut node = clock::at(1_000, node);
        let peer = peripheral(1);

        clock::at(1_000, || {
            assert_eq!(
                node.peripheral_seen(&peer, -80),
                vec![Action::Connect(peer.clone())]
            );
        });

        // The link comes up and the peer sends something the wire cannot
        // carry, so this device drops it and the shell reports back.
        clock::at(1_030, || {
            node.link_up(LinkId(9), Some(peer.clone()), Role::Dialer, 100)
                .unwrap();
            node.bytes_received(LinkId(9), b"not a fragment");
            node.link_down(LinkId(9));
        });

        // Graded as a walk-away this would have been dialable at 1045, and as
        // never-answered at 1060.
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
