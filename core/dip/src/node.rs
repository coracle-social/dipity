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
}

impl Node {
    /// Build a node over a store the shell has opened and a key it has read out
    /// of the Keychain or Keystore.
    pub fn new(db: Arc<Db>, identity: SecretKey) -> Result<Self> {
        let policy = Arc::new(query::policy(&db, &identity.public_key())?);

        Ok(Self {
            db,
            identity,
            policy,
            sessions: BTreeMap::new(),
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
    pub fn peripheral_seen(&mut self, _peripheral: &PeripheralId, _rssi: i16) -> Vec<Action> {
        todo!("scheduler: RSSI floor, rate limit, backoff, link cap")
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

        self.collect()
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
    /// connected.
    ///
    /// Immediate rather than waiting for the next reconciliation, so a note
    /// written in a crowd propagates while the crowd is still there.
    pub fn publish(&mut self, _event: &HashedEvent) -> Result<Vec<Action>> {
        todo!("db::command::publish_event, then offer on every syncing session")
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
}
