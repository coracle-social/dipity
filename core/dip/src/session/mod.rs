//! One link's lifecycle, from a GATT connection to a synchronizing peer.
//!
//! A session is keyed on [`LinkId`]. Nothing here performs I/O.

pub mod peer;
pub mod recognition;

pub use peer::Peer;
pub use recognition::{Tag, Tags};

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use coracle_lib::events::{EventContent, HashedEvent};
use coracle_lib::filters::Filter;
use coracle_lib::keys::{PublicKey, SecretKey};
use coracle_lib::tags::Tags as NostrTags;

use crate::clock;
use crate::db::Db;
use crate::link::{LinkId, Role};
use crate::model::{Policy, Standing};
use crate::sync::client::{Negotiation, Used};
use crate::sync::relay;
use crate::sync::{Message, Quota, SubscriptionId};
use crate::transport::{Channel, Codec, Frame, Noise, Outbox};

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
    /// This device's identity key, for signing AUTH responses.
    identity: SecretKey,
    /// When a frame was last heard, which the heartbeat measures.
    last_heard: i64,
    /// When the session entered [`Draining`](State::Draining).
    draining_since: Option<i64>,
    /// The challenge this device sent, to match against the peer's response.
    sent_challenge: Option<String>,
    /// The peer's challenge, to sign and return.
    peer_challenge: Option<String>,
    /// What this device has accepted from the peer this session.
    used: Used,
    /// Open subscriptions from the peer's relay half.
    subscriptions: BTreeMap<SubscriptionId, Filter>,
    /// Open negotiations against the peer's relay half.
    negotiations: BTreeMap<SubscriptionId, Negotiation>,
}

impl Session {
    /// Open a session over a link the shell has just reported up.
    pub fn open(
        link: LinkId,
        role: Role,
        mtu: usize,
        policy: Arc<Policy>,
        identity: SecretKey,
    ) -> Result<Self> {
        Ok(Self {
            link,
            role,
            state: State::Linked,
            noise: Noise::begin(role),
            codec: Codec::new(mtu)?,
            outbox: Outbox::default(),
            policy,
            peer: None,
            identity,
            last_heard: clock::now(),
            draining_since: None,
            sent_challenge: None,
            peer_challenge: None,
            used: Used::default(),
            subscriptions: BTreeMap::new(),
            negotiations: BTreeMap::new(),
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

    /// The dialer's first move: send the Noise handshake opening.
    pub fn initiate(&mut self) -> Result<()> {
        if self.role != Role::Dialer {
            bail!("only the dialer initiates");
        }

        let message = self.noise.first_handshake_message()?;

        self.send_plain(&message)
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
    pub fn advance(&mut self, db: &Db, frame: &Frame) -> Result<()> {
        match self.state {
            State::Linked => self.advance_handshake(frame)?,
            // The two AUTH directions are independent, so a challenge can
            // arrive after a response already made us Identified.
            State::Secured | State::DialerIdentified | State::Identified => {
                self.on_auth_frame(db, frame)?
            }
            _ => {}
        }

        Ok(())
    }

    /// The peer's quota for this session.
    #[must_use]
    pub fn quota(&self) -> Quota {
        match self.peer.as_ref().and_then(|peer| peer.policies.first()) {
            Some(policy) if policy.standing() == Standing::Trusted => Quota::TRUSTED,
            _ => Quota::STRANGER,
        }
    }

    /// Handle one frame on the sync channel.
    pub fn handle_sync(&mut self, db: &Db, frame: &Frame) -> Result<()> {
        let message = Message::decode(&frame.payload)
            .context("the sync channel carried a malformed message")?;

        let Some(peer) = self.peer.as_ref().cloned() else {
            bail!("sync traffic before the peer is identified");
        };

        let quota = self.quota();

        if self.try_relay(db, &peer, &message, quota)? {
            return Ok(());
        }

        let replies = crate::sync::client::handle(
            db,
            &peer,
            message,
            quota,
            &mut self.used,
            &mut self.negotiations,
        )?;

        for reply in replies {
            self.send_sync(&reply)?;
        }

        Ok(())
    }

    /// Offer a freshly stored event to every subscription the peer has open,
    /// pushing what the relay half would serve.
    pub fn offer(&mut self, db: &Db, _event: &HashedEvent) -> Result<()> {
        let Some(peer) = self.peer.as_ref().cloned() else {
            return Ok(());
        };

        let subscriptions: Vec<SubscriptionId> = self.subscriptions.keys().cloned().collect();

        for subscription in subscriptions {
            let Some(filter) = self.subscriptions.get(&subscription).cloned() else {
                continue;
            };

            // serve recomputes from the store, which now holds the event, so
            // the offer is exactly what a fresh REQ would have returned.
            let replies = relay::serve(db, &peer, &subscription, &[filter])?;

            for reply in replies {
                self.send_sync(&reply)?;
            }
        }

        Ok(())
    }

    /// Try the relay half. Returns `true` when the message was consumed.
    fn try_relay(&mut self, db: &Db, peer: &Peer, message: &Message, quota: Quota) -> Result<bool> {
        match message {
            Message::Req(..)
            | Message::Close(..)
            | Message::NegOpen(..)
            | Message::NegMsg(..)
            | Message::Publish(..) => {
                let replies = relay::handle(
                    db,
                    peer,
                    message.clone(),
                    quota,
                    &mut self.used,
                    &mut self.subscriptions,
                )?;

                for reply in replies {
                    self.send_sync(&reply)?;
                }

                Ok(true)
            }
            _ => Ok(false),
        }
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

    // ========================================================================
    // Handshake
    // ========================================================================

    /// Process a handshake message and advance to [`Secured`] when it
    /// completes.
    fn advance_handshake(&mut self, frame: &Frame) -> Result<()> {
        let reply = self.noise.read_handshake(&frame.payload)?;

        // A handshake reply goes out plaintext: the peer's handshake state
        // decrypts it, and transport encryption starts only after the
        // handshake is fully complete on both sides.
        if let Some(reply) = reply {
            self.send_plain(&reply)?;
        }

        if self.noise.is_complete() {
            self.state = State::Secured;

            // The dialer sends its recognition tags and AUTH challenge first.
            if self.role == Role::Dialer {
                self.send_recognition_tags()?;
                self.send_auth_challenge()?;
            }
        }

        Ok(())
    }

    // ========================================================================
    // Secured: recognition, then mutual AUTH
    // ========================================================================

    /// Dispatch one post-handshake control frame by what it carries.
    ///
    /// Recognition tags, an AUTH challenge and an AUTH response share the
    /// control channel and carry no opcode, so the payload shape is the
    /// discriminator: the tags are an array of 32-byte arrays, a challenge is
    /// a UTF-8 string, and a response is an event.
    fn on_auth_frame(&mut self, db: &Db, frame: &Frame) -> Result<()> {
        // An AUTH response is JSON, and JSON is valid UTF-8, so the event
        // parse must come before the challenge string.
        if serde_json::from_slice::<Vec<[u8; 32]>>(&frame.payload).is_ok() {
            self.resolve_recognition(db, frame)?;
        } else if let Ok(event) =
            serde_json::from_slice::<coracle_lib::events::Event>(&frame.payload)
        {
            self.verify_auth_response(event)?;

            // Once a device has verified the peer and answered the peer's
            // challenge, both identities are bound.
            if self.peer.is_some() && self.peer_challenge.is_some() {
                self.state = State::Identified;
            }
        } else if let Ok(challenge) = String::from_utf8(frame.payload.clone()) {
            self.peer_challenge = Some(challenge.clone());
            self.send_auth_response(&challenge)?;

            // The receiver answers the dialer's challenge and challenges back
            // in the same turn; the dialer has already challenged.
            if self.role == Role::Receiver {
                self.send_auth_challenge()?;
                self.state = State::DialerIdentified;
            }
        } else {
            bail!("an unknown frame arrived on the control channel");
        }

        Ok(())
    }

    // ========================================================================
    // Recognition
    // ========================================================================

    /// Send this device's recognition tags, padded to the constant count.
    fn send_recognition_tags(&mut self) -> Result<()> {
        // Pair secrets are not yet stored; send only padding so the count
        // does not disclose how many peers this device has paired with.
        let padding: Vec<[u8; 32]> = (0..recognition::TAG_COUNT)
            .map(|_| {
                let mut tag = [0u8; 32];
                let _ = getrandom::getrandom(&mut tag);
                tag
            })
            .collect();

        let payload = serde_json::to_vec(&padding).context("serializing recognition tags")?;

        self.send_control(&payload)
    }

    /// Trial-MAC the offered tags against every pair secret this device holds.
    fn resolve_recognition(&mut self, _db: &Db, frame: &Frame) -> Result<Option<PublicKey>> {
        let _offered: Vec<[u8; 32]> =
            serde_json::from_slice(&frame.payload).context("decoding recognition tags")?;

        // Pair secrets are not yet stored, so the peer is always a stranger.
        Ok(None)
    }

    // ========================================================================
    // AUTH
    // ========================================================================

    /// Send a NIP-42 challenge to the peer.
    fn send_auth_challenge(&mut self) -> Result<()> {
        let mut challenge = [0u8; 16];
        getrandom::getrandom(&mut challenge).context("generating an AUTH challenge")?;

        let challenge = hex::encode(challenge);
        self.sent_challenge = Some(challenge.clone());
        self.send_control(challenge.as_bytes())
    }

    /// Answer the peer's challenge with a signed kind 22242 event.
    fn send_auth_response(&mut self, challenge: &str) -> Result<()> {
        // The relay tag names the party that challenged us — the peer — so
        // the response binds to the channel their static key established.
        let remote = self
            .noise
            .remote_static_key()
            .ok_or_else(|| anyhow::anyhow!("the handshake has not completed"))?;
        let relay = format!("noise://{}", hex::encode(remote));

        let hashed = EventContent::new()
            .with_content("")
            .with_tags(
                NostrTags::new()
                    .add("challenge", [challenge])
                    .add("relay", [relay.as_str()]),
            )
            .with_kind(22_242)
            .with_created_at(clock::now())
            .with_pubkey(self.identity.public_key())
            .with_id();

        let id = hashed.id;
        let event = hashed.with_sig(self.identity.sign(id.as_bytes()));
        let payload = serde_json::to_vec(&event).context("serializing AUTH response")?;

        self.send_control(&payload)
    }

    /// Verify the peer's AUTH response against our challenge and this channel.
    fn verify_auth_response(&mut self, event: coracle_lib::events::Event) -> Result<()> {
        if event.kind != 22_242 {
            bail!("the AUTH response is not kind 22242");
        }

        if event.verify().is_err() {
            bail!("the AUTH response has an invalid signature");
        }

        let challenge = self.sent_challenge.as_ref().ok_or_else(|| {
            anyhow::anyhow!("an AUTH response arrived before a challenge was sent")
        })?;

        let tag_challenge = event
            .tags
            .value("challenge")
            .ok_or_else(|| anyhow::anyhow!("the AUTH response has no challenge tag"))?;
        if tag_challenge != challenge.as_str() {
            bail!("the AUTH response carries a different challenge");
        }

        // We issued the challenge, so the response must name our channel.
        let relay = format!("noise://{}", hex::encode(self.noise.local_static_key()));
        let tag_relay = event
            .tags
            .value("relay")
            .ok_or_else(|| anyhow::anyhow!("the AUTH response has no relay tag"))?;
        if tag_relay != relay.as_str() {
            bail!("the AUTH response names a different channel");
        }

        self.identify([event.pubkey]);

        Ok(())
    }

    // ========================================================================
    // Sending helpers
    // ========================================================================

    /// Queue a raw payload on the control channel, encrypted.
    fn send_control(&mut self, payload: &[u8]) -> Result<()> {
        self.send(&Frame {
            channel: Channel::Control,
            payload: payload.to_vec(),
        })
    }

    /// Queue a handshake payload on the control channel without encryption.
    /// Only the handshake path uses this.
    fn send_plain(&mut self, payload: &[u8]) -> Result<()> {
        let fragments = self.codec.fragment(&Frame {
            channel: Channel::Control,
            payload: payload.to_vec(),
        });

        self.outbox.push(Channel::Control, fragments);

        Ok(())
    }

    /// Queue a sync message as a frame on the sync channel.
    fn send_sync(&mut self, message: &Message) -> Result<()> {
        let payload = message.encode();

        self.send(&Frame {
            channel: Channel::Sync,
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::{author, secret};

    fn policy() -> Policy {
        Policy::new(author(1))
    }

    fn identity() -> SecretKey {
        secret(1)
    }

    fn session(policy: Policy) -> Session {
        Session::open(LinkId(1), Role::Dialer, 64, Arc::new(policy), identity()).unwrap()
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

    #[test]
    fn a_full_handshake_moves_a_pair_to_secured() {
        // A big MTU so every handshake message is one fragment.
        let mut dialer =
            Session::open(LinkId(1), Role::Dialer, 4096, Arc::new(policy()), secret(1)).unwrap();
        let mut receiver = Session::open(
            LinkId(2),
            Role::Receiver,
            4096,
            Arc::new(policy()),
            secret(2),
        )
        .unwrap();

        dialer.initiate().unwrap();
        let msg1 = dialer.next_write().unwrap();
        dialer.acknowledge_write();

        let frame = receiver.receive(&msg1).unwrap().unwrap();
        receiver
            .advance(&Db::open_in_memory().unwrap(), &frame)
            .unwrap();

        let reply = receiver.next_write().unwrap();
        receiver.acknowledge_write();

        let frame = dialer.receive(&reply).unwrap().unwrap();
        dialer
            .advance(&Db::open_in_memory().unwrap(), &frame)
            .unwrap();

        // The dialer queues its final handshake message, then its recognition
        // tags and AUTH challenge after. Only the first is part of the exchange
        // under test.
        let final_msg = dialer.next_write().unwrap();
        dialer.acknowledge_write();

        let frame = receiver.receive(&final_msg).unwrap().unwrap();
        receiver
            .advance(&Db::open_in_memory().unwrap(), &frame)
            .unwrap();

        assert!(dialer.noise.is_complete());
        assert!(receiver.noise.is_complete());
        assert_eq!(dialer.state(), State::Secured);
        assert_eq!(receiver.state(), State::Secured);
    }

    /// Pump one queued fragment from `sender` into `receiver`, advancing it.
    fn pump(sender: &mut Session, receiver: &mut Session, db: &Db) {
        let fragment = sender.next_write().expect("a queued fragment to pump");
        sender.acknowledge_write();

        let frame = receiver
            .receive(&fragment)
            .unwrap()
            .expect("a complete frame from one fragment");

        receiver.advance(db, &frame).unwrap();
    }

    #[test]
    fn a_pair_reaches_identified_through_mutual_auth() {
        let db = Db::open_in_memory().unwrap();
        let mut dialer =
            Session::open(LinkId(1), Role::Dialer, 4096, Arc::new(policy()), secret(1)).unwrap();
        let mut receiver = Session::open(
            LinkId(2),
            Role::Receiver,
            4096,
            Arc::new(policy()),
            secret(2),
        )
        .unwrap();

        dialer.initiate().unwrap();
        pump(&mut dialer, &mut receiver, &db); // handshake msg1
        pump(&mut receiver, &mut dialer, &db); // handshake reply

        // Dialer is now Secured and has queued: msg3, tags, challenge.
        pump(&mut dialer, &mut receiver, &db); // msg3: receiver completes handshake
        pump(&mut dialer, &mut receiver, &db); // tags: receiver resolves (stranger)
        pump(&mut dialer, &mut receiver, &db); // challenge: receiver answers + challenges back

        // Receiver queued: response, own challenge. Dialer is Secured.
        pump(&mut receiver, &mut dialer, &db); // response: dialer verifies, identifies
        pump(&mut receiver, &mut dialer, &db); // own challenge: dialer answers

        // Dialer is Identified. Its response to the receiver's challenge
        // completes the receiver.
        pump(&mut dialer, &mut receiver, &db); // response: receiver verifies

        assert_eq!(dialer.state(), State::Identified);
        assert_eq!(receiver.state(), State::Identified);
        assert_eq!(
            dialer
                .peer()
                .unwrap()
                .pubkeys()
                .copied()
                .collect::<Vec<_>>(),
            vec![author(2)]
        );
        assert_eq!(
            receiver
                .peer()
                .unwrap()
                .pubkeys()
                .copied()
                .collect::<Vec<_>>(),
            vec![author(1)]
        );
    }
}
