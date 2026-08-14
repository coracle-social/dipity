//! One link's lifecycle, from a GATT connection to a synchronizing peer.
//!
//! A session is keyed on [`LinkId`]. Nothing here performs I/O.

pub mod peer;
pub mod recognition;

pub use peer::Peer;
pub use recognition::{Tag, Tags};

use std::sync::Arc;

use anyhow::Result;
use coracle_lib::keys::PublicKey;

use crate::clock;
use crate::db::Db;
use crate::link::{LinkId, Role};
use crate::model::Policy;
use crate::transport::{Codec, Frame, Noise, Outbox};

/// How long a session sits in [`Draining`](State::Draining) before it is closed.
pub const DRAIN_CAP_SECONDS: i64 = 300;

/// How long without a heartbeat before an idle session drains.
pub const HEARTBEAT_TIMEOUT_SECONDS: i64 = 60;

/// Where a link is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// GATT connected, no security.
    Linked,
    /// The channel is encrypted and nobody is identified.
    Secured,
    /// The dialer has named itself. The receiver has disclosed nothing, and
    /// may still leave without doing so.
    DialerIdentified,
    /// Both pubkeys are bound and policy has been evaluated on both sides.
    Identified,
    /// Events, then blobs.
    Syncing,
    /// No new work; in-flight transfers finish or time out.
    Draining,
    /// Torn down. Synced data is retained.
    Closed,
}

impl State {
    /// Whether the peer has proved an identity.
    #[must_use]
    pub fn is_identified(self) -> bool {
        matches!(self, State::Identified | State::Syncing | State::Draining)
    }

    /// Whether new work may start.
    #[must_use]
    pub fn is_open(self) -> bool {
        !matches!(self, State::Draining | State::Closed)
    }
}

/// One link, and everything the core knows about it.
pub struct Session {
    /// The link this session runs over.
    pub link: LinkId,
    /// Which side dialed.
    pub role: Role,
    /// Where the link is in its lifecycle.
    state: State,
    /// The Noise session encrypting it.
    noise: Noise,
    /// Fragmentation, sized to this link's negotiated MTU.
    codec: Codec,
    /// What is waiting to go out, most urgent channel first.
    outbox: Outbox,
    /// The user's policy, shared with every other live session.
    policy: Arc<Policy>,
    /// What the peer has proved, once it has proved anything.
    peer: Option<Peer>,
    /// When a frame was last heard, which the heartbeat measures.
    last_heard: i64,
    /// When the session entered [`Draining`](State::Draining).
    draining_since: Option<i64>,
}

impl Session {
    /// Open a session over a link the shell has just reported up.
    pub fn open(link: LinkId, role: Role, mtu: usize, policy: Arc<Policy>) -> Result<Self> {
        Ok(Self {
            link,
            role,
            state: State::Linked,
            noise: Noise::begin(role),
            codec: Codec::new(mtu)?,
            outbox: Outbox::default(),
            policy,
            peer: None,
            last_heard: clock::now(),
            draining_since: None,
        })
    }

    /// Where the link is in its lifecycle.
    #[must_use]
    pub fn state(&self) -> State {
        self.state
    }

    /// What the peer has proved, or `None` before it has proved anything.
    #[must_use]
    pub fn peer(&self) -> Option<&Peer> {
        self.peer.as_ref()
    }

    /// Record what the peer proved over mutual `AUTH`, binding policy to each
    /// pubkey.
    ///
    /// A peer that turns out to be blocked is refused here: the consent gate
    /// ran before anyone was named.
    pub fn identify(&mut self, pubkeys: impl IntoIterator<Item = PublicKey>) {
        let peer = Peer::bind(self.link, pubkeys, &self.policy);

        self.state = if peer.is_blocked() {
            State::Closed
        } else {
            State::Identified
        };
        self.peer = Some(peer);
    }

    /// Take a policy the user has just changed, and rebind it.
    ///
    /// A session follows the user's preferences rather than caching them,
    /// because policy is read at encounter time with nobody watching.
    pub fn set_policy(&mut self, policy: Arc<Policy>) {
        self.policy = policy;

        if let Some(peer) = self.peer.take() {
            self.identify(peer.into_pubkeys());
        }
    }

    /// Queue a frame, fragmented to this link's MTU.
    ///
    /// Encryption happens here, once the handshake has completed. Everything
    /// above deals in plaintext frames.
    pub fn send(&mut self, frame: &Frame) -> Result<()> {
        let payload = if self.noise.is_complete() {
            self.noise.encrypt(&frame.payload)?
        } else {
            frame.payload.clone()
        };

        let fragments = self.codec.fragment(&Frame {
            channel: frame.channel,
            payload,
        });

        self.outbox.push(frame.channel, fragments);

        Ok(())
    }

    /// The next fragment for the shell to write, if the last one has been
    /// acknowledged.
    pub fn next_write(&mut self) -> Option<Vec<u8>> {
        self.outbox.next_write()
    }

    /// Record that the fragment the shell was handed has been acknowledged.
    pub fn acknowledge_write(&mut self) {
        self.outbox.acknowledge();
    }

    /// Take one write off the characteristic, returning a decrypted frame once
    /// its last fragment lands.
    pub fn receive(&mut self, write: &[u8]) -> Result<Option<Frame>> {
        self.last_heard = clock::now();

        let Some(frame) = self.codec.absorb(write)? else {
            return Ok(None);
        };

        if !self.noise.is_complete() {
            return Ok(Some(frame));
        }

        Ok(Some(Frame {
            channel: frame.channel,
            payload: self.noise.decrypt(&frame.payload)?,
        }))
    }

    /// Advance the lifecycle on an inbound control frame: the handshake, the
    /// recognition exchange, the consent gate, mutual `AUTH`, the heartbeat.
    ///
    /// Everything up to [`Identified`](State::Identified) happens here.
    /// [`Syncing`](State::Syncing) hands the sync channel to
    /// [`crate::sync`].
    pub fn advance(&mut self, _db: &Db, _frame: &Frame) -> Result<()> {
        todo!("handshake, recognition, gate, mutual AUTH")
    }

    /// Move to [`Draining`](State::Draining): finish what is in flight, start
    /// nothing new.
    pub fn drain(&mut self) {
        if self.state.is_open() {
            self.state = State::Draining;
            self.draining_since = Some(clock::now());
        }
    }

    /// Tear the session down.
    pub fn close(&mut self) {
        self.state = State::Closed;
    }

    /// Whether the session has run out of time.
    #[must_use]
    pub fn expired(&self) -> bool {
        let now = clock::now();

        match self.draining_since {
            Some(since) => self.outbox.is_idle() || now - since >= DRAIN_CAP_SECONDS,
            None => now - self.last_heard >= HEARTBEAT_TIMEOUT_SECONDS,
        }
    }

    /// When this session next needs attention, for the shell to arm a timer on.
    #[must_use]
    pub fn deadline(&self) -> i64 {
        match self.draining_since {
            Some(since) => since + DRAIN_CAP_SECONDS,
            None => self.last_heard + HEARTBEAT_TIMEOUT_SECONDS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::author;

    fn policy() -> Policy {
        Policy::new(author(1))
    }

    fn session(policy: Policy) -> Session {
        Session::open(LinkId(1), Role::Dialer, 64, Arc::new(policy)).unwrap()
    }

    #[test]
    fn nothing_is_proved_before_the_peer_names_itself() {
        assert!(session(policy()).peer().is_none());
    }

    #[test]
    fn identifying_binds_the_policy_to_every_pubkey_the_peer_proved() {
        let mut session = session(policy());

        session.identify([author(2), author(3)]);

        let peer = session.peer().unwrap();

        assert_eq!(peer.pubkeys().count(), 2);
        assert_eq!(peer.policies.len(), 2);
        assert_eq!(peer.identity, author(1));
        assert_eq!(session.state(), State::Identified);
    }

    #[test]
    fn proving_one_pubkey_twice_does_not_lengthen_the_list() {
        let mut session = session(policy());

        session.identify([author(2), author(2)]);

        assert_eq!(session.peer().unwrap().policies.len(), 1);
    }

    #[test]
    fn a_peer_holding_a_blocked_identity_is_refused_whatever_else_it_proved() {
        let mut policy = policy();
        policy.graph.blocked.insert(author(3));

        let mut session = session(policy);

        session.identify([author(2), author(3)]);

        assert!(session.peer().unwrap().is_blocked());
        assert_eq!(session.state(), State::Closed);
    }

    #[test]
    fn a_silent_session_ages_out_of_the_heartbeat_window() {
        let session = clock::at(1_000, || session(policy()));
        let timeout = 1_000 + HEARTBEAT_TIMEOUT_SECONDS;

        assert!(clock::at(timeout - 1, || !session.expired()));
        assert!(clock::at(timeout, || session.expired()));
        assert_eq!(session.deadline(), timeout);
    }

    #[test]
    fn draining_gives_an_in_flight_transfer_until_the_cap() {
        let mut session = session(policy());

        session
            .send(&crate::transport::Frame {
                channel: crate::transport::Channel::Blob,
                payload: vec![0; 32],
            })
            .unwrap();
        clock::at(1_000, || session.drain());

        let cap = 1_000 + DRAIN_CAP_SECONDS;

        assert!(clock::at(cap - 1, || !session.expired()));
        assert!(clock::at(cap, || session.expired()));

        // An idle outbox ends the drain early: there is nothing left to finish.
        while session.next_write().is_some() {
            session.acknowledge_write();
        }

        assert!(clock::at(1_001, || session.expired()));
    }

    #[test]
    fn blocking_a_peer_mid_session_closes_it() {
        let mut session = session(policy());

        session.identify([author(2)]);
        assert_eq!(session.state(), State::Identified);

        let mut blocked = policy();
        blocked.graph.blocked.insert(author(2));

        session.set_policy(Arc::new(blocked));

        assert!(session.peer().unwrap().is_blocked());
        assert_eq!(session.state(), State::Closed);
    }
}
