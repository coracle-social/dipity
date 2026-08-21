//! One link's lifecycle, from a GATT connection to a synchronizing peer.
//!
//! A session is keyed on [`LinkId`]. Nothing here performs I/O.
//!
//! The lifecycle itself is [`State`], and each phase's bookkeeping rides on
//! its variant, so a phase cannot exist without the data it needs and no flag
//! can disagree with the state it shadows. Everything else is a behavior the
//! session composes, each owning its own state and clock:
//!
//! | Behavior | Owns |
//! | --- | --- |
//! | [`Wire`] | The encrypted pipe: Noise, fragmentation, write scheduling |
//! | [`Heartbeat`] | Liveness: what was last heard, when to beat next |
//! | [`Gate`] | Consent: admission, the cool-off, the disclosure budget |
//! | [`AuthExchange`] | Mutual NIP-42, and the pubkeys the peer proved |
//! | [`Relay`] | The relay half: the peer's open subscriptions |
//! | [`Client`] | The client half: negotiations, fetches, pending proofs |
//! | [`BlobExchange`] | Blobs, both directions: serving, and the one fetch |
//! | [`SessionSpending`] | The peer's quota use, this session and this window |
//!
//! What stays on the session is what ties them together: the lifecycle, the
//! [`Peer`] the exchange proved, and the policy it is bound under.

pub mod auth;
pub mod gate;
pub mod heartbeat;
pub mod peer;
pub mod recognition;

pub use peer::Peer;
pub use recognition::{Tag, Tags};

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use coracle_lib::events::HashedEvent;
use coracle_lib::filters::Filter;
use coracle_lib::keys::{PublicKey, SecretKey};

use crate::blobs::BlobStore;
use crate::clock;
use crate::db::Db;
use crate::link::{LinkId, Role};
use crate::model::{Identity, Policy};
use crate::spending::{SessionSpending, SpendingLedger};
use crate::sync::blob::BlobExchange;
use crate::sync::client::Client;
use crate::sync::relay::{self, Relay};
use crate::sync::{Message, Quota};
use crate::transport::{Channel, Frame, Wire};

use auth::AuthExchange;
use gate::{Gate, Presence, Verdict};
use heartbeat::Heartbeat;

/// How long a session sits in [`Draining`](State::Draining) before it is
/// closed, however much is still in flight.
pub const DRAIN_CAP_SECONDS: i64 = 300;

/// How long an unanswered consent prompt holds the link open.
///
/// The same five minutes as the drain cap today, and a separate constant
/// because they answer to different rules: this one is how long the user has
/// to look at their phone, and moving it has nothing to do with how long a
/// transfer is given to finish. `docs/discovery.md#the-consent-gate`.
pub const GATE_HOLD_SECONDS: i64 = 300;

/// How long a link has to name somebody before it is given up on.
///
/// The consent gate is only reached once recognition tags arrive, so a peer
/// that completes the handshake and then sends nothing but heartbeats is never
/// gated, never times out, and holds one of the link slots for as long as it
/// keeps beating. Reaching `Identified` is a handful of round trips.
pub const IDENTIFY_CAP_SECONDS: i64 = 60;

/// Control-frame discriminants, one byte ahead of the payload.
///
/// Recognition tags, an AUTH challenge, an AUTH response and the heartbeat
/// share the control channel, so the first byte names what follows. The
/// handshake frames that precede encryption carry no discriminant: they are
/// raw Noise messages and only [`State::Linked`] ever sees them.
mod control {
    /// A list of 32-byte recognition tags.
    pub const TAGS: u8 = 0x01;
    /// A NIP-42 challenge string.
    pub const CHALLENGE: u8 = 0x02;
    /// A signed kind 22242 event answering a challenge.
    pub const AUTH_EVENT: u8 = 0x03;
    /// A liveness beacon, carrying nothing.
    pub const HEARTBEAT: u8 = 0x04;
}

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
    /// Waiting on the user to approve an unadmitted stranger, up to
    /// [`GATE_HOLD_SECONDS`].
    GatePending {
        /// When the hold began.
        since: i64,
        /// Whether the shell has been asked to prompt yet.
        approval_requested: bool,
    },
    /// No new work; in-flight transfers finish or time out.
    Draining {
        /// When the drain began.
        since: i64,
    },
    /// Torn down. Synced data is retained.
    Closed,
}

impl State {
    /// Whether new work may start.
    #[must_use]
    pub fn is_open(self) -> bool {
        !matches!(self, State::Draining { .. } | State::Closed)
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
    /// When the link came up, which bounds how long it may go unidentified.
    opened_at: i64,
    /// The encrypted pipe the session talks through.
    wire: Wire,
    /// Liveness: when the peer was last heard, and when to beat next.
    heartbeat: Heartbeat,
    /// The consent gate, holding strangers until policy or the user admits
    /// them.
    gate: Gate,
    /// Mutual NIP-42, and every pubkey the peer has proved over it.
    auth: AuthExchange,
    /// The user's policy, shared with every other live session.
    policy: Arc<Policy>,
    /// This device's identity key, for signing AUTH responses.
    identity: SecretKey,
    /// The proved pubkeys bound under policy, once the peer has proved any.
    peer: Option<Peer>,
    /// The relay half: what this device serves, and the subscriptions it
    /// serves to.
    relay: Relay,
    /// The client half: negotiations, fetches, and events awaiting proofs.
    client: Client,
    /// The blob half: serving the peer's requests, driving this device's one
    /// fetch.
    blobs: BlobExchange,
    /// What the peer has spent against its quota, this session and this
    /// window.
    spending: SessionSpending,
}

impl Session {
    /// Open a session over a link the shell has just reported up.
    pub fn open(
        link: LinkId,
        role: Role,
        mtu: usize,
        policy: Arc<Policy>,
        identity: SecretKey,
        blobs: Arc<dyn BlobStore>,
        spending: Arc<SpendingLedger>,
    ) -> Result<Self> {
        Ok(Self {
            link,
            role,
            state: State::Linked,
            opened_at: clock::now(),
            wire: Wire::new(role, mtu)?,
            heartbeat: Heartbeat::new()?,
            gate: Gate::default(),
            auth: AuthExchange::default(),
            policy,
            identity,
            peer: None,
            relay: Relay::default(),
            client: Client::default(),
            blobs: BlobExchange::new(blobs),
            spending: SessionSpending::new(spending),
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

        self.wire.initiate()
    }

    /// Record what the peer proved over `AUTH`, binding policy to each pubkey.
    ///
    /// A peer may authenticate as several pubkeys across several responses, so
    /// this accumulates rather than replacing. A peer that turns out to be
    /// blocked is refused here: the consent gate ran before anyone was named.
    pub fn identify(&mut self, pubkeys: impl IntoIterator<Item = PublicKey>) {
        self.auth.prove(pubkeys);
        self.bind_peer();

        // A newly proved identity moves a secured link to Identified. A
        // session already syncing stays where it is, and a blocked one was
        // closed by the binding above.
        if matches!(
            self.state,
            State::Secured | State::DialerIdentified | State::Identified
        ) {
            self.state = State::Identified;
        }
    }

    /// Bind the peer from the pubkeys it has proved, under the current policy,
    /// closing the session if any of them is blocked.
    ///
    /// Binds only, whatever the phase: a peer that has proved nothing stays
    /// `None`, and the lifecycle moves in [`identify`](Self::identify) alone.
    fn bind_peer(&mut self) {
        if !self.auth.has_proved() {
            return;
        }

        let peer = Peer::bind(self.link, self.auth.proved().copied(), &self.policy);

        if peer.is_blocked() {
            self.state = State::Closed;
        }

        self.peer = Some(peer);
    }

    /// Take a policy the user has just changed and rebind the peer under it,
    /// closing the session if any proved pubkey is now blocked.
    ///
    /// A session follows the user's preferences rather than caching them,
    /// because policy is read at encounter time with nobody watching.
    pub fn set_policy(&mut self, policy: Arc<Policy>) {
        self.policy = policy;
        self.bind_peer();
    }

    /// Where the app is, which the cool-off admission window is measured
    /// against. `None` means it has not been in the foreground this run.
    pub fn set_presence(&mut self, presence: Option<Presence>) {
        self.gate.set_presence(presence);
    }

    /// Answer the consent gate's hold, from the user's decision.
    ///
    /// Approving resumes whatever the gate held back; refusing closes the link.
    pub fn approve(&mut self, db: &Db, approved: bool) -> Result<()> {
        if !matches!(self.state, State::GatePending { .. }) {
            return Ok(());
        }

        if !approved {
            self.state = State::Closed;
            return Ok(());
        }

        self.gate.pass();
        self.state = State::Secured;

        // Resume the deferred turn, if its trigger has already arrived.
        match self.role {
            Role::Receiver if self.auth.challenged() => {
                self.send_recognition_tags(db)?;
                self.challenge_peer()?;
            }
            Role::Dialer => {
                if self.answer_peer_challenge()? {
                    self.state = State::DialerIdentified;
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Whether the shell should be told to prompt for this session, once.
    pub fn request_approval(&mut self) -> bool {
        if let State::GatePending {
            approval_requested, ..
        } = &mut self.state
            && !*approval_requested
        {
            *approval_requested = true;
            return true;
        }

        false
    }

    /// Queue a frame for the peer, encrypted and fragmented by the wire.
    pub fn send(&mut self, frame: &Frame) -> Result<()> {
        self.wire.send(frame)
    }

    /// The next fragment for the shell to write, if the last one has been
    /// acknowledged. The wire seals it here, in the order it goes out.
    pub fn next_write(&mut self) -> Result<Option<Vec<u8>>> {
        self.wire.next_write()
    }

    /// Record that the fragment the shell was handed has been acknowledged.
    pub fn acknowledge_write(&mut self) {
        self.wire.acknowledge_write();
    }

    /// Take one write off the characteristic, returning a decrypted frame once
    /// its last fragment lands.
    pub fn receive(&mut self, write: &[u8]) -> Result<Option<Frame>> {
        self.heartbeat.heard();

        self.wire.receive(write)
    }

    /// Advance the lifecycle on an inbound control frame: the handshake, the
    /// recognition exchange, the consent gate, mutual `AUTH`, the heartbeat.
    ///
    /// Everything up to [`Identified`](State::Identified) happens here.
    pub fn advance(&mut self, db: &Db, frame: &Frame) -> Result<()> {
        match self.state {
            State::Linked => self.advance_handshake(db, frame)?,
            // Control frames keep arriving after identification — heartbeats
            // above all, and a peer proving a second identity — so the secured
            // states share one dispatcher, including the gate hold.
            State::Secured
            | State::DialerIdentified
            | State::Identified
            | State::Syncing
            | State::GatePending { .. } => self.on_control_frame(db, frame)?,
            _ => {}
        }

        Ok(())
    }

    /// The identity this device is acting as on this link.
    #[must_use]
    fn local(&self) -> Identity {
        Identity::from([self.identity.public_key()])
    }

    /// The peer's quota for this session.
    ///
    /// A peer that proved any trusted identity is the same device whatever it
    /// signs with, so the trusted budget is the whole session's.
    #[must_use]
    pub fn quota(&self) -> Quota {
        match self.peer.as_ref() {
            Some(peer) if peer.is_trusted() => Quota::TRUSTED,
            _ => Quota::STRANGER,
        }
    }

    /// Handle one frame on the sync channel, routing it to the blob, relay or
    /// client half.
    pub fn handle_sync(&mut self, db: &Db, frame: &Frame) -> Result<()> {
        let message = Message::decode(&frame.payload)
            .context("the sync channel carried a malformed message")?;

        let Some(peer) = self.peer.as_ref().cloned() else {
            bail!("sync traffic before the peer is identified");
        };

        // Draining finishes what is in flight and starts nothing new: a
        // publish, a fresh subscription or a fresh negotiation waits for the
        // next encounter, while replies to work already underway pass. A
        // Blossom request passes too: the peer's own in-flight fetch asks for
        // its groups one request at a time.
        if matches!(self.state, State::Draining { .. }) {
            match &message {
                Message::Publish(_) | Message::Req(..) | Message::NegOpen(..) => return Ok(()),
                _ => {}
            }
        }

        let quota = self.quota();

        // Blobs answer and drive their own channel: a request is served from
        // the store, a response advances the fetch in flight. Both directions
        // are metered against the same quota the event halves get.
        match &message {
            Message::BlossomRequest(request) => {
                let reply = self.blobs.serve(db, &peer, &self.local(), request, quota)?;

                self.send_sync(&Message::BlossomResponse(Box::new(reply)))?;
                return Ok(());
            }
            Message::BlossomResponse(response) => {
                if let Some(request) =
                    self.blobs
                        .on_response(db, &peer, response.as_ref(), quota)?
                {
                    self.send_sync(&Message::BlossomRequest(Box::new(request)))?;
                }

                // A finished fetch frees the slot for the next wanted blob.
                self.maybe_fetch_blob(db)?;
                return Ok(());
            }
            _ => {}
        }

        let replies = if self.is_for_relay(&message) {
            self.relay.handle(
                db,
                &peer,
                &self.identity,
                message,
                quota,
                &mut self.spending,
            )?
        } else {
            self.client
                .handle(db, &peer, &self.local(), message, quota, &mut self.spending)?
        };

        for reply in replies {
            self.send_sync(&reply)?;
        }

        Ok(())
    }

    /// Whether the relay half answers this message.
    ///
    /// A `NEG-MSG` naming a negotiation this device opened belongs to the
    /// client half; everything the peer initiates belongs to the relay.
    fn is_for_relay(&self, message: &Message) -> bool {
        match message {
            Message::Req(..) | Message::Close(..) | Message::NegOpen(..) | Message::Publish(..) => {
                true
            }
            Message::NegMsg(subscription, _) => !self.client.owns_subscription(subscription),
            _ => false,
        }
    }

    /// Begin a reconciliation against the peer, sending the opening frame.
    ///
    /// The subscription is this device's own, which is how a later reply is
    /// told apart from one this device must answer: a `NEG-MSG` naming it goes
    /// to the client half, anything else to the relay half.
    pub fn begin_negotiation(&mut self, db: &Db, filter: Filter) -> Result<()> {
        if !self.state.is_open() {
            return Ok(());
        }

        // The diff itself is over everything this device holds, but opening one
        // before the peer is bound would start a sync nothing is authorized to
        // answer.
        if self.peer.is_none() {
            bail!("a negotiation needs the peer to be identified");
        }

        let (negotiation, opening) = crate::sync::client::Negotiation::begin(db, filter)?;

        self.client.open_negotiation(negotiation);
        self.send_sync(&opening)?;

        Ok(())
    }

    /// Once both identities are bound, store the pair secret and the
    /// disclosure, and open the reconciliation.
    ///
    /// Runs once: [`identify`](Self::identify) never regresses a session that
    /// is already syncing, so the first arrival here is the only one. The
    /// accept policy gates ingest and the peer's relay half bounds what it
    /// will answer, so a bare filter is the right opening move.
    fn enter_syncing(&mut self, db: &Db) -> Result<()> {
        if self.state != State::Identified {
            return Ok(());
        }

        self.state = State::Syncing;

        self.pair(db)?;
        self.begin_negotiation(db, Filter::new())?;
        self.maybe_fetch_blob(db)
    }

    /// Store the pair secret derived from this session and record the
    /// disclosure of this device's identity, against every pubkey the peer
    /// proved. The first pairing for a pubkey establishes its secret.
    fn pair(&mut self, db: &Db) -> Result<()> {
        let Some(peer) = self.peer.as_ref() else {
            return Ok(());
        };

        let secret = recognition::derive_secret(&self.wire.handshake_hash()?);
        let pubkeys: Vec<PublicKey> = peer.pubkeys().copied().collect();

        crate::db::command::pair_with(db, &pubkeys, &secret, clock::now())
    }

    /// Start the next blob fetch once the session is syncing and none is in
    /// flight.
    fn maybe_fetch_blob(&mut self, db: &Db) -> Result<()> {
        if self.state != State::Syncing {
            return Ok(());
        }

        let quota = self.quota();

        if let Some(request) = self.blobs.poll(db, quota)? {
            self.send_sync(&Message::BlossomRequest(Box::new(request)))?;
        }

        Ok(())
    }

    /// Whether the shell should be told to open L2CAP for this link, once.
    pub fn take_l2cap_request(&mut self) -> bool {
        self.blobs.take_l2cap_request()
    }

    /// The battery level in percent, which gates blob transfers.
    pub fn set_battery(&mut self, level: Option<u8>) {
        self.blobs.set_battery(level);
    }

    /// Offer a just-stored event to every subscription the peer has open, if the
    /// event would have been served on a fresh `REQ`.
    ///
    /// Driven by the store's event channel, so anything that stores an own event —
    /// a publish, or an own event coming home through an ingest — reaches every
    /// connected peer without the writer knowing.
    pub fn offer_event(&mut self, db: &Db, event: &HashedEvent) -> Result<()> {
        if !self.state.is_open() {
            return Ok(());
        }

        let Some(peer) = self.peer.as_ref().cloned() else {
            return Ok(());
        };

        // The same two tests the relay half applies: any of the subscription's
        // filters, and what the peer may be served. Own events are always in
        // the Own register, so the registers need no check here.
        if !peer.may_be_served(event) {
            return Ok(());
        }

        for subscription in self.relay.matching(event) {
            let mut messages = vec![Message::Event(
                subscription.clone(),
                Box::new(event.clone()),
            )];

            relay::attach(
                db,
                &peer,
                &self.identity,
                &subscription,
                event,
                &mut messages,
            )?;

            for message in messages {
                self.send_sync(&message)?;
            }
        }

        Ok(())
    }

    /// Move to [`Draining`](State::Draining): finish what is in flight, start
    /// nothing new.
    pub fn drain(&mut self) {
        if self.state.is_open() {
            self.state = State::Draining {
                since: clock::now(),
            };
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

        match self.state {
            State::Draining { since } => {
                !self.has_work_in_flight() || now - since >= DRAIN_CAP_SECONDS
            }
            State::GatePending { since, .. } => now - since >= GATE_HOLD_SECONDS,
            // A link that has not named anyone cannot sync, and its heartbeat
            // would keep it alive indefinitely, so the pre-identification phase
            // has a deadline of its own.
            State::Linked | State::Secured | State::DialerIdentified => {
                self.heartbeat.timed_out() || now - self.opened_at >= IDENTIFY_CAP_SECONDS
            }
            _ => self.heartbeat.timed_out(),
        }
    }

    /// Whether anything is still moving: bytes queued or in flight, a blob
    /// transfer waiting on its next group, or an answer this device asked the
    /// peer for.
    ///
    /// A drain waits on all three. The outbox alone is not enough — a blob
    /// fetch is a request/response ping-pong, and between our write being
    /// acknowledged and the peer's answer landing there is nothing queued at
    /// all. Ending the drain there is exactly what `docs/discovery.md` forbids:
    /// "Do not kill a working transfer over two missed beacons."
    #[must_use]
    fn has_work_in_flight(&self) -> bool {
        !self.wire.is_idle() || self.blobs.is_fetching() || self.client.is_awaiting()
    }

    /// When this session next needs attention, for the shell to arm a timer on.
    #[must_use]
    pub fn deadline(&self) -> i64 {
        match self.state {
            State::Draining { since } => since + DRAIN_CAP_SECONDS,
            State::GatePending { since, .. } => since + GATE_HOLD_SECONDS,
            State::Linked | State::Secured | State::DialerIdentified => self
                .heartbeat
                .timeout_deadline()
                .min(self.opened_at + IDENTIFY_CAP_SECONDS),
            _ if self.beats() => self
                .heartbeat
                .next_beat_deadline()
                .min(self.heartbeat.timeout_deadline()),
            _ => self.heartbeat.timeout_deadline(),
        }
    }

    /// Send a heartbeat if one is due and the link has been quiet.
    ///
    /// A beat proves liveness to the peer; anything already queued or in
    /// flight proves it too, so a busy link skips the beat rather than piling
    /// one behind a transfer. Only post-handshake states beat.
    pub fn maybe_heartbeat(&mut self) -> Result<()> {
        if !self.beats() || !self.heartbeat.due() {
            return Ok(());
        }

        // Reschedule whether or not a beat goes out, so a long transfer does
        // not emit one the moment it drains.
        self.heartbeat.reschedule()?;

        // Traffic on its way out is the heartbeat; a beat would only wait
        // behind it.
        if self.wire.is_idle() {
            self.send_control(control::HEARTBEAT, &[])?;
        }

        Ok(())
    }

    /// Whether the heartbeat runs in this state: after the handshake, before
    /// teardown, and through the gate hold that keeps a pending link alive.
    fn beats(&self) -> bool {
        matches!(
            self.state,
            State::Secured
                | State::DialerIdentified
                | State::Identified
                | State::Syncing
                | State::GatePending { .. }
        )
    }

    // ========================================================================
    // Handshake
    // ========================================================================

    /// Process a handshake message and advance to [`Secured`](State::Secured)
    /// when it completes. The wire queues whatever reply the pattern calls
    /// for.
    fn advance_handshake(&mut self, db: &Db, frame: &Frame) -> Result<()> {
        if self.wire.read_handshake(&frame.payload)? {
            self.state = State::Secured;

            // The dialer sends its recognition tags and AUTH challenge first.
            if self.role == Role::Dialer {
                self.send_recognition_tags(db)?;
                self.challenge_peer()?;
            }
        }

        Ok(())
    }

    // ========================================================================
    // Secured: recognition, then the gate, then mutual AUTH
    // ========================================================================

    /// Dispatch one post-handshake control frame by its discriminant byte.
    fn on_control_frame(&mut self, db: &Db, frame: &Frame) -> Result<()> {
        let Some((&discriminant, payload)) = frame.payload.split_first() else {
            bail!("an empty frame arrived on the control channel");
        };

        match discriminant {
            control::TAGS => self.on_tags(db, payload)?,
            control::CHALLENGE => self.on_challenge(db, payload)?,
            control::AUTH_EVENT => self.on_auth_response(payload)?,
            control::HEARTBEAT => {
                // The write itself proved liveness in `receive`; there is
                // nothing else a beacon carries.
            }
            other => bail!("an unknown control frame {other} arrived"),
        }

        // Both identities are bound once the peer is verified and this device
        // has disclosed its own, in whichever order the two happened.
        if self.peer.is_some() && self.auth.disclosed() {
            self.enter_syncing(db)?;
        }

        Ok(())
    }

    /// Resolve the peer's recognition tags and run the consent gate on the
    /// result, before anyone has disclosed a pubkey.
    fn on_tags(&mut self, db: &Db, payload: &[u8]) -> Result<()> {
        // Recognition happens once, before anyone is named. A list arriving
        // later is a peer trying to re-run the gate on an exchange that has
        // moved past it, which would knock a syncing session back into a hold
        // and raise a second prompt for a peer the user already admitted.
        if self.state != State::Secured {
            bail!("recognition tags arrived after the exchange moved past the gate");
        }

        let resolved = self.resolve_recognition(db, payload)?;

        match self.gate.evaluate(db, &self.policy, &resolved)? {
            Verdict::Pass => {}
            Verdict::Blocked => self.state = State::Closed,
            Verdict::Pending => {
                self.state = State::GatePending {
                    since: clock::now(),
                    approval_requested: false,
                };
            }
        }

        Ok(())
    }

    /// Take the peer's challenge. The receiver challenges back without
    /// answering; the dialer answers, identifying first. A challenge arriving
    /// before the gate passes waits; approval resumes the turn.
    fn on_challenge(&mut self, db: &Db, payload: &[u8]) -> Result<()> {
        self.auth.receive_challenge(payload)?;

        if !self.gate.passed() {
            return Ok(());
        }

        match self.role {
            // The receiver names no pubkey yet: it sends its own tags and
            // challenge, and answers only after it has seen the dialer.
            Role::Receiver => {
                self.send_recognition_tags(db)?;
                self.challenge_peer()?;
            }
            // The dialer identifies itself first.
            Role::Dialer => {
                if self.answer_peer_challenge()? {
                    self.state = State::DialerIdentified;
                }
            }
        }

        Ok(())
    }

    /// Verify the peer's AUTH response. The receiver discloses here, once it
    /// has seen the dialer.
    ///
    /// Nothing is accepted while the gate holds: a held stranger cannot
    /// identify itself into being served before the user has answered.
    fn on_auth_response(&mut self, payload: &[u8]) -> Result<()> {
        if !self.gate.passed() {
            bail!("an AUTH response arrived before the gate passed");
        }

        let pubkey = self.auth.verify(payload, self.wire.local_static_key())?;

        self.identify([pubkey]);

        if self.state == State::Closed {
            return Ok(());
        }

        // The receiver now discloses, having evaluated the dialer's identity.
        if self.role == Role::Receiver {
            self.answer_peer_challenge()?;
        }

        Ok(())
    }

    /// Send a NIP-42 challenge to the peer.
    fn challenge_peer(&mut self) -> Result<()> {
        let challenge = self.auth.make_challenge()?;

        self.send_control(control::CHALLENGE, challenge.as_bytes())
    }

    /// Answer the peer's challenge, disclosing this device's identity — if one
    /// is waiting and this device has not disclosed already.
    fn answer_peer_challenge(&mut self) -> Result<bool> {
        let remote = self
            .wire
            .remote_static_key()
            .ok_or_else(|| anyhow::anyhow!("the handshake has not completed"))?;

        match self.auth.answer(&self.identity, remote)? {
            Some(payload) => {
                self.send_control(control::AUTH_EVENT, &payload)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    // ========================================================================
    // Recognition
    // ========================================================================

    /// Send this device's recognition tags: always the constant count, so the
    /// length discloses neither how many peers this device has paired with nor
    /// that it has paired with any.
    fn send_recognition_tags(&mut self, db: &Db) -> Result<()> {
        let hash = self.wire.handshake_hash()?;
        let secrets = crate::db::query::pair_secrets(db)?;
        let tags = recognition::select(&secrets, &hash)?;

        self.send_control(control::TAGS, &tags.encode())
    }

    /// Trial-MAC the offered tags against every pair secret this device holds,
    /// returning every pubkey they resolve to.
    fn resolve_recognition(&self, db: &Db, payload: &[u8]) -> Result<Vec<PublicKey>> {
        let offered = recognition::Tags::decode(payload)?;
        let hash = self.wire.handshake_hash()?;
        let secrets = crate::db::query::pair_secrets(db)?;

        Ok(recognition::resolve(&offered, &secrets, &hash))
    }

    // ========================================================================
    // Sending helpers
    // ========================================================================

    /// Queue a control frame, encrypted, with its discriminant ahead of the
    /// payload.
    fn send_control(&mut self, discriminant: u8, payload: &[u8]) -> Result<()> {
        let mut framed = Vec::with_capacity(payload.len() + 1);
        framed.push(discriminant);
        framed.extend_from_slice(payload);

        self.send(&Frame {
            channel: Channel::Control,
            payload: framed,
        })
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

    use crate::db::query as db_query;
    use crate::fixtures::{author, note, secret};
    use crate::model::Query;
    use coracle_lib::tags::Tags as NostrTags;

    fn policy() -> Policy {
        Policy::new(author(1))
    }

    fn identity() -> SecretKey {
        secret(1)
    }

    fn blobs() -> Arc<crate::blobs::MemoryBlobStore> {
        Arc::new(crate::blobs::MemoryBlobStore::default())
    }

    fn spending() -> Arc<SpendingLedger> {
        Arc::new(SpendingLedger::default())
    }

    fn session(policy: Policy) -> Session {
        Session::open(
            LinkId(1),
            Role::Dialer,
            64,
            Arc::new(policy),
            identity(),
            blobs(),
            spending(),
        )
        .unwrap()
    }

    /// A session past its handshake, where identification is legal.
    fn secured_session(policy: Policy) -> Session {
        secured_pair(policy).0
    }

    /// A pair whose wires have run the Noise handshake, both moved to
    /// [`State::Secured`] without the recognition exchange that follows it.
    ///
    /// The peer is what opens what the session seals, so a test that reads a
    /// payload off the characteristic needs the other end of the cipher.
    fn secured_pair(policy: Policy) -> (Session, Session) {
        let mut session = session(policy);
        let mut peer = Session::open(
            LinkId(2),
            Role::Receiver,
            64,
            Arc::new(Policy::new(author(2))),
            secret(2),
            blobs(),
            spending(),
        )
        .unwrap();

        session.wire.initiate().unwrap();

        while !(session.wire.is_secured() && peer.wire.is_secured()) {
            pump_handshake(&mut session, &mut peer);
            pump_handshake(&mut peer, &mut session);
        }

        session.state = State::Secured;
        peer.state = State::Secured;

        (session, peer)
    }

    /// Move every queued handshake fragment from one session to the other,
    /// leaving the lifecycle above the wire alone.
    fn pump_handshake(from: &mut Session, into: &mut Session) {
        while let Some(write) = from.next_write().unwrap() {
            from.acknowledge_write();

            if let Some(frame) = into.wire.receive(&write).unwrap() {
                into.wire.read_handshake(&frame.payload).unwrap();
            }
        }
    }

    /// Move fragments from one session to the other until a whole frame lands.
    fn pump_frame(from: &mut Session, into: &mut Session) -> Frame {
        while let Some(write) = from.next_write().unwrap() {
            from.acknowledge_write();

            if let Some(frame) = into.wire.receive(&write).unwrap() {
                return frame;
            }
        }

        panic!("nothing was queued to pump");
    }

    #[test]
    fn nothing_is_proved_before_the_peer_names_itself() {
        assert!(session(policy()).peer().is_none());
    }

    #[test]
    fn identifying_binds_the_policy_to_every_pubkey_the_peer_proved() {
        let mut session = secured_session(policy());

        session.identify([author(2), author(3)]);

        let peer = session.peer().unwrap();

        assert_eq!(peer.pubkeys().count(), 2);
        assert_eq!(session.state(), State::Identified);
    }

    #[test]
    fn proving_one_pubkey_twice_does_not_lengthen_the_set() {
        let mut session = secured_session(policy());

        session.identify([author(2), author(2)]);

        assert_eq!(session.peer().unwrap().pubkeys().count(), 1);
    }

    #[test]
    fn a_peer_holding_a_blocked_identity_is_refused_whatever_else_it_proved() {
        let mut policy = policy();
        policy.graph.blocked.insert(author(3));

        let mut session = secured_session(policy);

        session.identify([author(2), author(3)]);

        assert!(session.peer().unwrap().is_blocked());
        assert_eq!(session.state(), State::Closed);
    }

    #[test]
    fn a_policy_change_leaves_an_unidentified_session_alone() {
        // Mid-handshake, nothing proved: the user toggling a preference must
        // not move the lifecycle or fabricate a peer.
        let mut session = session(policy());

        session.set_policy(Arc::new(policy()));

        assert_eq!(session.state(), State::Linked);
        assert!(session.peer().is_none());
    }

    #[test]
    fn a_policy_change_leaves_a_draining_session_draining() {
        let mut session = secured_session(policy());
        session.identify([author(2)]);
        session.drain();

        session.set_policy(Arc::new(policy()));

        assert!(matches!(session.state(), State::Draining { .. }));
    }

    #[test]
    fn a_silent_session_ages_out_of_the_heartbeat_window() {
        let session = clock::at(1_000, || session(policy()));
        let timeout = 1_000 + heartbeat::TIMEOUT_SECONDS;

        assert!(clock::at(timeout - 1, || !session.expired()));
        assert!(clock::at(timeout, || session.expired()));
        assert_eq!(session.deadline(), timeout);
    }

    #[test]
    fn draining_gives_an_in_flight_transfer_until_the_cap() {
        let mut session = secured_session(policy());

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
        while session.next_write().unwrap().is_some() {
            session.acknowledge_write();
        }

        assert!(clock::at(1_001, || session.expired()));
    }

    #[test]
    fn a_drain_waits_on_a_fetch_the_outbox_cannot_see() {
        // A blob fetch is a request/response ping-pong: once our request has
        // been acknowledged and before the peer answers, the outbox is empty
        // and nothing is in flight as far as the wire knows. Ending the drain
        // there kills a working transfer, which is what the doc forbids.
        let db = Db::open_in_memory().unwrap();

        // Trusted, because a stranger has no blob budget to fetch against.
        let mut policy = policy();
        policy.graph.trusted.insert(author(2));
        let mut session = secured_session(policy);
        session.identify([author(2)]);
        session.state = State::Syncing;

        // Something the peer holds that this device wants.
        let bytes = b"the quick brown fox";
        let hash = crate::model::BlobHash::digest(bytes);
        let event = note(
            author(1),
            1,
            "with a blob",
            NostrTags::new().add(
                "imeta",
                [format!("x {hash}"), format!("size {}", bytes.len())],
            ),
        );
        crate::db::command::publish_event(&db, &event, &author(1), 1).unwrap();

        session.maybe_fetch_blob(&db).unwrap();

        // Drain, then let the outbox run dry the way the shell would.
        clock::at(1_000, || session.drain());
        while session.next_write().unwrap().is_some() {
            session.acknowledge_write();
        }

        assert!(
            clock::at(1_001, || !session.expired()),
            "the drain gave up on a transfer that is still in flight"
        );
        assert!(
            clock::at(1_000 + DRAIN_CAP_SECONDS, || session.expired()),
            "the drain outlived its hard cap"
        );
    }

    #[test]
    fn a_link_that_never_names_anyone_is_given_up_on() {
        // The gate is only reached once tags arrive, so a peer that completes
        // the handshake and then only beats is never gated and never times
        // out. Without a deadline it holds a link slot indefinitely.
        let mut session = clock::at(1_000, || secured_session(policy()));

        // Beating keeps the heartbeat satisfied the whole time.
        clock::at(1_000 + IDENTIFY_CAP_SECONDS - 1, || {
            session.heartbeat.heard();
            assert!(!session.expired());
        });

        clock::at(1_000 + IDENTIFY_CAP_SECONDS, || {
            session.heartbeat.heard();
            assert!(session.expired(), "an unidentified link outlived its cap");
        });
    }

    #[test]
    fn tags_arriving_after_the_gate_are_refused() {
        // Re-running recognition on a session that is already syncing would
        // knock it back into a hold and prompt the user a second time for a
        // peer they already admitted.
        let db = Db::open_in_memory().unwrap();
        let mut session = secured_session(policy());
        session.identify([author(2)]);
        session.state = State::Syncing;

        let payload = recognition::select(&[], &[0u8; 32]).unwrap().encode();
        let mut framed = vec![control::TAGS];
        framed.extend_from_slice(&payload);

        let frame = Frame {
            channel: Channel::Control,
            payload: framed,
        };

        assert!(session.advance(&db, &frame).is_err());
        assert_eq!(session.state(), State::Syncing);
    }

    #[test]
    fn blocking_a_peer_mid_session_closes_it() {
        let mut session = secured_session(policy());

        session.identify([author(2)]);
        assert_eq!(session.state(), State::Identified);

        let mut blocked = policy();
        blocked.graph.blocked.insert(author(2));

        session.set_policy(Arc::new(blocked));

        assert!(session.peer().unwrap().is_blocked());
        assert_eq!(session.state(), State::Closed);
    }

    #[test]
    fn a_trusted_identity_earns_the_trusted_budget_whichever_key_it_proved() {
        let mut policy = policy();
        policy.graph.trusted.insert(author(9));
        let mut session = secured_session(policy);

        // The trusted pubkey sorts after the stranger one, which is what the
        // old "first policy" reading would have picked instead.
        session.identify([author(2), author(9)]);

        assert_eq!(session.quota(), Quota::TRUSTED);
    }

    #[test]
    fn a_quiet_session_beats_within_the_jittered_bounds() {
        let (mut session, mut peer) = clock::at(1_000, || secured_pair(policy()));
        session.identify([author(2)]);

        // Before the minimum interval nothing is due.
        clock::at(1_000 + heartbeat::MIN_INTERVAL_SECONDS - 1, || {
            session.maybe_heartbeat().unwrap();
            assert!(session.next_write().unwrap().is_none());
        });

        // Past the maximum it is, and the beat carries the discriminant alone.
        clock::at(1_000 + heartbeat::MAX_INTERVAL_SECONDS + 1, || {
            session.maybe_heartbeat().unwrap();
        });

        assert_eq!(
            pump_frame(&mut session, &mut peer),
            Frame {
                channel: Channel::Control,
                payload: vec![control::HEARTBEAT],
            }
        );
    }

    #[test]
    fn a_busy_link_skips_the_beat_and_treats_traffic_as_liveness() {
        let mut session = clock::at(1_000, || secured_session(policy()));
        session.identify([author(2)]);

        // Queued traffic stands in for the beat.
        session
            .send(&crate::transport::Frame {
                channel: crate::transport::Channel::Sync,
                payload: vec![0; 8],
            })
            .unwrap();

        clock::at(1_000 + heartbeat::MAX_INTERVAL_SECONDS + 1, || {
            session.maybe_heartbeat().unwrap();
        });

        // The first fragment out is the queued traffic, not a beat.
        let fragment = session.next_write().unwrap().unwrap();
        assert_eq!(fragment[0], Channel::Sync as u8);
    }

    #[test]
    fn a_heartbeat_is_absorbed_without_moving_the_session() {
        let db = Db::open_in_memory().unwrap();
        let mut session = secured_session(policy());
        session.identify([author(2)]);

        let frame = Frame {
            channel: crate::transport::Channel::Control,
            payload: vec![control::HEARTBEAT],
        };
        session.advance(&db, &frame).unwrap();

        assert_eq!(session.state(), State::Identified);
    }

    #[test]
    fn an_unknown_control_frame_is_an_error() {
        let db = Db::open_in_memory().unwrap();
        let mut session = secured_session(policy());
        session.identify([author(2)]);

        let frame = Frame {
            channel: crate::transport::Channel::Control,
            payload: vec![0xEE],
        };

        assert!(session.advance(&db, &frame).is_err());
    }

    #[test]
    fn approval_is_requested_once_and_refusal_closes() {
        let db = Db::open_in_memory().unwrap();
        let mut session = session(policy());
        session.state = State::GatePending {
            since: clock::now(),
            approval_requested: false,
        };

        assert!(session.request_approval());
        assert!(!session.request_approval(), "the prompt is one-shot");

        session.approve(&db, false).unwrap();
        assert_eq!(session.state(), State::Closed);
    }

    #[test]
    fn an_auth_response_during_the_gate_hold_is_refused() {
        let db = Db::open_in_memory().unwrap();
        let mut session = session(policy());
        session.state = State::GatePending {
            since: clock::now(),
            approval_requested: false,
        };

        // A held stranger volunteering an identity must not be accepted: that
        // would bypass the hold and open the relay half without approval.
        let frame = Frame {
            channel: crate::transport::Channel::Control,
            payload: vec![control::AUTH_EVENT, 0x7B],
        };

        assert!(session.advance(&db, &frame).is_err());
        assert!(session.peer().is_none());
    }

    #[test]
    fn a_draining_session_takes_no_new_work() {
        let db = Db::open_in_memory().unwrap();
        let mut session = secured_session(policy());

        session.identify([author(2)]);
        session.drain();

        let publish = Frame {
            channel: crate::transport::Channel::Sync,
            payload: Message::Publish(Box::new(note(author(2), 100, "new", NostrTags::new())))
                .encode(),
        };

        session.handle_sync(&db, &publish).unwrap();

        // No OK travels back and nothing is stored: the publish waits for the
        // next encounter.
        assert!(session.next_write().unwrap().is_none());
        assert!(
            db_query::list_events(&db, &Query::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_negotiation_reply_goes_to_the_client_half() {
        let db = Db::open_in_memory().unwrap();
        let (mut session, mut peer) = secured_pair(policy());

        session.identify([author(2)]);

        // This device opens a negotiation, so the subscription is this side's;
        // the session's own bookkeeping is what routes the reply.
        session
            .begin_negotiation(&db, coracle_lib::filters::Filter::new())
            .unwrap();

        let opening = pump_frame(&mut session, &mut peer);
        let Message::NegOpen(subscription, _, frame) = Message::decode(&opening.payload).unwrap()
        else {
            panic!("expected the opening NEG-OPEN");
        };

        // The peer's relay answers with an empty set under the same filter.
        let incoming = coracle_lib::sync::Message::decode(&frame).unwrap();
        let reply = coracle_lib::sync::reconcile_responder(
            &coracle_lib::sync::SyncSet::from_items([]),
            &incoming,
            coracle_lib::sync::FrameBudget::bytes(1024),
        );

        let reply = Frame {
            channel: crate::transport::Channel::Sync,
            payload: Message::NegMsg(subscription, reply.encode()).encode(),
        };

        // Routed to the client half rather than misread as a reply to a
        // negotiation this device never hosted, so something travels back.
        session.handle_sync(&db, &reply).unwrap();
        assert!(session.next_write().unwrap().is_some());
    }

    #[test]
    fn a_full_handshake_moves_a_pair_to_secured() {
        // A big MTU so every handshake message is one fragment.
        let mut dialer = Session::open(
            LinkId(1),
            Role::Dialer,
            4096,
            Arc::new(policy()),
            secret(1),
            blobs(),
            spending(),
        )
        .unwrap();
        let mut receiver = Session::open(
            LinkId(2),
            Role::Receiver,
            4096,
            Arc::new(policy()),
            secret(2),
            blobs(),
            spending(),
        )
        .unwrap();

        dialer.initiate().unwrap();
        let msg1 = dialer.next_write().unwrap().unwrap();
        dialer.acknowledge_write();

        let frame = receiver.receive(&msg1).unwrap().unwrap();
        receiver
            .advance(&Db::open_in_memory().unwrap(), &frame)
            .unwrap();

        let reply = receiver.next_write().unwrap().unwrap();
        receiver.acknowledge_write();

        let frame = dialer.receive(&reply).unwrap().unwrap();
        dialer
            .advance(&Db::open_in_memory().unwrap(), &frame)
            .unwrap();

        // The dialer queues its final handshake message, then its recognition
        // tags and AUTH challenge after. Only the first is part of the exchange
        // under test.
        let final_msg = dialer.next_write().unwrap().unwrap();
        dialer.acknowledge_write();

        let frame = receiver.receive(&final_msg).unwrap().unwrap();
        receiver
            .advance(&Db::open_in_memory().unwrap(), &frame)
            .unwrap();

        assert!(dialer.wire.is_secured());
        assert!(receiver.wire.is_secured());
        assert_eq!(dialer.state(), State::Secured);
        assert_eq!(receiver.state(), State::Secured);
    }

    /// Pump one queued fragment from `sender` into `receiver`, advancing it.
    fn pump(sender: &mut Session, receiver: &mut Session, db: &Db) {
        let fragment = sender
            .next_write()
            .unwrap()
            .expect("a queued fragment to pump");
        sender.acknowledge_write();

        let frame = receiver
            .receive(&fragment)
            .unwrap()
            .expect("a complete frame from one fragment");

        receiver.advance(db, &frame).unwrap();
    }

    /// Run the full secured exchange — handshake, recognition, mutual AUTH —
    /// pumping every fragment both ways until both sides are Syncing.
    fn full_exchange(dialer: &mut Session, receiver: &mut Session, db: &Db) {
        dialer.initiate().unwrap();
        pump(dialer, receiver, db); // handshake msg1
        pump(receiver, dialer, db); // handshake reply
        pump(dialer, receiver, db); // msg3
        pump(dialer, receiver, db); // dialer tags
        pump(dialer, receiver, db); // dialer challenge
        pump(receiver, dialer, db); // receiver tags
        pump(receiver, dialer, db); // receiver challenge
        pump(dialer, receiver, db); // dialer response
        pump(receiver, dialer, db); // receiver response

        assert_eq!(dialer.state(), State::Syncing);
        assert_eq!(receiver.state(), State::Syncing);
    }

    fn pair(mtu: usize, role: Role, key: u8) -> Session {
        Session::open(
            LinkId(1),
            role,
            mtu,
            Arc::new(policy()),
            secret(key),
            blobs(),
            spending(),
        )
        .unwrap()
    }

    #[test]
    fn a_pair_reaches_syncing_through_mutual_auth() {
        let db = Db::open_in_memory().unwrap();
        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);

        // The cool-off admits the strangers, or the gate holds the pair.
        dialer.set_presence(Some(Presence::Foreground));
        receiver.set_presence(Some(Presence::Foreground));

        dialer.initiate().unwrap();
        pump(&mut dialer, &mut receiver, &db); // handshake msg1
        pump(&mut receiver, &mut dialer, &db); // handshake reply

        // Dialer is now Secured and has queued: msg3, tags, challenge.
        pump(&mut dialer, &mut receiver, &db); // msg3: receiver completes handshake
        pump(&mut dialer, &mut receiver, &db); // tags: receiver resolves, admits
        pump(&mut dialer, &mut receiver, &db); // challenge: receiver tags + challenges back

        // Receiver queued: tags, own challenge. The dialer passes the gate on
        // the tags and answers the challenge, identifying first.
        pump(&mut receiver, &mut dialer, &db); // tags: dialer resolves, admits
        pump(&mut receiver, &mut dialer, &db); // challenge: dialer answers (identifies)

        // The dialer's response lets the receiver identify it and disclose in
        // turn; its own response then completes the dialer.
        pump(&mut dialer, &mut receiver, &db); // response: receiver identifies + discloses
        pump(&mut receiver, &mut dialer, &db); // response: dialer identifies

        assert_eq!(dialer.state(), State::Syncing);
        assert_eq!(receiver.state(), State::Syncing);
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

        // Each side has queued its reconciliation opening on the sync channel.
        // The payload is encrypted, but the frame header is not.
        let opening = dialer.next_write().unwrap().unwrap();
        assert_eq!(opening[0], Channel::Sync as u8);
    }

    #[test]
    fn pairing_stores_the_same_secret_on_both_sides() {
        let db = Db::open_in_memory().unwrap();
        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);

        dialer.set_presence(Some(Presence::Foreground));
        receiver.set_presence(Some(Presence::Foreground));

        full_exchange(&mut dialer, &mut receiver, &db);

        // Each side stored a secret for the other, and both derived the same
        // bytes from the shared handshake hash.
        let secrets = crate::db::query::pair_secrets(&db).unwrap();
        assert_eq!(secrets.len(), 2);
        assert_eq!(secrets[0].1, secrets[1].1);
    }

    #[test]
    fn a_paired_peer_is_recognized_on_the_next_encounter() {
        let db = Db::open_in_memory().unwrap();

        // First encounter pairs them, admitted by the cool-off.
        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);
        dialer.set_presence(Some(Presence::Foreground));
        receiver.set_presence(Some(Presence::Foreground));
        full_exchange(&mut dialer, &mut receiver, &db);

        // Second encounter: no cool-off, but the tags resolve, so the gate
        // passes silently and the pair reaches Syncing anyway.
        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);
        full_exchange(&mut dialer, &mut receiver, &db);
    }

    #[test]
    fn a_stranger_without_admission_is_held_and_can_be_approved() {
        let db = Db::open_in_memory().unwrap();
        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);

        // Only the dialer's cool-off runs, so it admits the receiver while the
        // receiver holds the dialer for the user.
        dialer.set_presence(Some(Presence::Foreground));

        dialer.initiate().unwrap();
        pump(&mut dialer, &mut receiver, &db); // msg1
        pump(&mut receiver, &mut dialer, &db); // reply
        pump(&mut dialer, &mut receiver, &db); // msg3
        pump(&mut dialer, &mut receiver, &db); // dialer tags: the gate holds
        pump(&mut dialer, &mut receiver, &db); // dialer challenge: held too

        assert!(matches!(receiver.state(), State::GatePending { .. }));
        assert!(
            !receiver.auth.disclosed(),
            "nothing disclosed before approval"
        );

        // The user admits the stranger, and the exchange completes.
        receiver.approve(&db, true).unwrap();
        pump(&mut receiver, &mut dialer, &db); // receiver tags: dialer admits
        pump(&mut receiver, &mut dialer, &db); // receiver challenge: dialer answers
        pump(&mut dialer, &mut receiver, &db); // dialer response: receiver discloses
        pump(&mut receiver, &mut dialer, &db); // receiver response: dialer identifies

        assert_eq!(dialer.state(), State::Syncing);
        assert_eq!(receiver.state(), State::Syncing);
    }
}
