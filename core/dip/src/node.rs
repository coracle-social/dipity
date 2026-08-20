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

use anyhow::Result;
use coracle_lib::events::HashedEvent;
use coracle_lib::keys::SecretKey;
use std::sync::Arc;
use tokio::sync::broadcast::{self, error::TryRecvError};

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
const BACKOFF_SECONDS: i64 = 15;

/// Everything scheduling dials from an advertisement.
#[derive(Debug, Default)]
struct Scheduler {
    /// Candidates queued by the floor, the link cap, or the rate limit, most
    /// recently seen first.
    candidates: Vec<(PeripheralId, i16)>,
    /// When each peripheral may next be dialed, ratcheted back on each attempt.
    backoff: BTreeMap<PeripheralId, i64>,
    /// When the last connect attempt went out, for the global rate limit.
    last_attempt: Option<i64>,
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
                Some(&until) => now >= until,
                None => true,
            };

            if admissible {
                self.candidates.remove(index);
                self.last_attempt = Some(now);
                self.backoff
                    .insert(peripheral.clone(), now + BACKOFF_SECONDS);

                return Some(peripheral);
            }
        }

        None
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
    /// Live sessions, keyed on link
    sessions: BTreeMap<LinkId, Session>,
    /// Decides which advertised peers to dial, and when.
    scheduler: Scheduler,
    /// This store's event channel, so a stored own event is offered to every
    /// connected peer without the writer knowing.
    events: broadcast::Receiver<EventChange>,
}

impl Node {
    /// Build a node over a store the shell has opened and a key it has read out
    /// of the Keychain or Keystore.
    pub fn new(db: Arc<Db>, identity: SecretKey) -> Result<Self> {
        let policy = Arc::new(query::policy(&db, &identity.public_key())?);

        Ok(Self {
            events: channel::subscribe(&db),
            db,
            identity,
            policy,
            sessions: BTreeMap::new(),
            scheduler: Scheduler::default(),
        })
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
    /// The dialer opens the Noise handshake here; the receiver waits for it.
    pub fn link_up(&mut self, link: LinkId, role: Role, mtu: usize) -> Result<Vec<Action>> {
        let mut session = Session::open(
            link,
            role,
            mtu,
            Arc::clone(&self.policy),
            self.identity.clone(),
        )?;

        if role == Role::Dialer {
            session.initiate()?;
        }

        self.sessions.insert(link, session);

        Ok(self.collect())
    }

    /// A link went away. A clean disconnect is reliable when it fires, so this
    /// drains immediately rather than waiting out the heartbeat.
    pub fn link_down(&mut self, link: LinkId) -> Vec<Action> {
        if let Some(session) = self.sessions.get_mut(&link) {
            session.drain();
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

    /// Store an event the user wrote, and offer it to every peer already
    /// Store an event the user wrote. The event channel does the offering.
    ///
    /// Immediate rather than waiting for the next reconciliation: what this
    /// write announces on the store's event channel is drained in `collect`,
    /// so a note written in a crowd propagates while the crowd is still there.
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

        let mut actions = Vec::new();

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
        Node::new(db(), SecretKey::generate()).unwrap()
    }

    fn db() -> Arc<Db> {
        Arc::new(Db::open_in_memory().unwrap())
    }

    fn peripheral(id: u8) -> PeripheralId {
        PeripheralId(id.to_string())
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

        clock::at(1_000 + BACKOFF_SECONDS, || {
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
        let mut node = Node::new(Arc::clone(&db), secret(1)).unwrap();
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
        let mut node = Node::new(Arc::clone(&db), secret(1)).unwrap();
        let event = note(author(1), 100, "hello", Tags::new());

        // A session that goes through the real REQ path.
        let policy = Arc::new(Policy::new(author(1)));
        let mut session =
            Session::open(LinkId(2), Role::Receiver, 4096, policy, secret(2)).unwrap();
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
        let actions = node.publish(&event).unwrap();

        // The Event carrying the note is queued to the peer's link.
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, Action::Send(link, _) if *link == LinkId(2)))
        );
    }
}
