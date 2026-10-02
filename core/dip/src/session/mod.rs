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
pub mod l2cap;
pub mod peer;
pub mod recognition;
pub mod sas;
pub mod transfer;

pub use peer::Peer;
pub use recognition::{Tag, Tags};

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use coracle_lib::events::HashedEvent;
use coracle_lib::filters::Filter;
use coracle_lib::keys::PublicKey;
use zeroize::{Zeroize, Zeroizing};

use crate::blobs::BlobStore;
use crate::clock;
use crate::db::Db;
use crate::keys::{Batch, KeyCustody};
use crate::link::{LinkId, Role};
use crate::model::{Identity, Policy, Standing};
use crate::sync::blob::BlobExchange;
use crate::sync::client::Client;
use crate::sync::relay::{self, Relay};
use crate::sync::spending::{SessionSpending, SpendingLedger};
use crate::sync::{Message, Quota};
use crate::transport::{Channel, Frame, Pipe, Wire};

use auth::AuthExchange;
use gate::{Gate, Presence, Verdict};
use heartbeat::Heartbeat;
use l2cap::{Step, Upgrade};
use transfer::IdentityTransfer;

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
/// Recognition tags, mutual `AUTH` and the heartbeat share the control
/// channel, so the first byte names what follows. The handshake frames that
/// precede encryption carry no discriminant: they are raw Noise messages and
/// only [`State::Linked`] ever sees them.
mod control {
    /// A list of 32-byte recognition tags.
    pub const TAGS: u8 = 0x01;
    /// A NIP-42 message, encoded as the relay protocol encodes it: either
    /// `["AUTH", <challenge>]` or `["AUTH", <event>]`. The payload says which,
    /// so both directions share one discriminant.
    pub const AUTH: u8 = 0x02;
    /// A liveness beacon, carrying nothing.
    pub const HEARTBEAT: u8 = 0x03;
    /// One step of an identity transfer, its own discriminant ahead of the
    /// payload. `docs/keys.md#login-with-device`.
    pub const TRANSFER: u8 = 0x04;
    /// One step of the L2CAP upgrade.
    /// `docs/transport.md#the-l2cap-bandwidth-upgrade`.
    pub const L2CAP: u8 = 0x05;
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

/// Why a session ended, which grades the dial that opened it.
///
/// The distinction the scheduler needs is whose decision the teardown was: a
/// peer that left comes back and is worth redialing at once, while one this
/// device refused would only be refused again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The peer stopped answering, or the shell reported the link gone.
    WalkedAway,
    /// This device tore the link down: policy blocks the peer, the consent
    /// gate lapsed or was refused, or a frame arrived the wire could not
    /// carry.
    Refused,
}

/// One link, and everything the core knows about it.
pub struct Session {
    /// The link this session runs over.
    pub link: LinkId,
    /// Which side dialed.
    pub role: Role,
    /// Where the link is in its lifecycle.
    pub state: State,
    /// Why the session ended, read once the state is `Closed`.
    pub ending: Ending,
    /// When the link has to have named somebody by. Sending recognition tags
    /// hands the peer a gate the user may hold, so it extends this by the hold.
    identify_by: i64,
    /// The encrypted pipe the session talks through.
    wire: Wire,
    /// Liveness: when the peer was last heard, and when to beat next.
    heartbeat: Heartbeat,
    /// The consent gate, holding strangers until policy or the user admits
    /// them.
    pub gate: Gate,
    /// Mutual NIP-42, and every pubkey the peer has proved over it.
    auth: AuthExchange,
    /// The user's policy, shared with every other live session.
    policy: Arc<Policy>,
    /// Where this device's identity key is read from, for signing AUTH
    /// responses and for moving the identity to another device.
    custody: Arc<dyn KeyCustody>,
    /// The pubkey that key names, derived once so the session can say who
    /// it is acting as without reading the key back out.
    identity: PublicKey,
    /// The proved pubkeys bound under policy, once the peer has proved any.
    pub peer: Option<Peer>,
    /// Who the peer's recognition tags resolved to, which is who this device
    /// already shares a working pair secret with.
    recognized: Vec<PublicKey>,
    /// The proved pubkeys the shell has already been told about.
    announced: Identity,
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
    /// Where the L2CAP bandwidth upgrade has got to on this link.
    upgrade: Upgrade,
    /// Moving this device's identity to the peer, if a user asked for it.
    pub transfer: IdentityTransfer,
}

impl Session {
    /// Open a session over a link the shell has just reported up.
    pub fn open(
        link: LinkId,
        role: Role,
        mtu: usize,
        policy: Arc<Policy>,
        custody: Arc<dyn KeyCustody>,
        blobs: Arc<dyn BlobStore>,
        spending: Arc<SpendingLedger>,
    ) -> Result<Self> {
        Ok(Self {
            link,
            role,
            state: State::Linked,
            ending: Ending::WalkedAway,
            identify_by: clock::now() + IDENTIFY_CAP_SECONDS,
            wire: Wire::new(role, mtu)?,
            heartbeat: Heartbeat::new()?,
            gate: Gate::default(),
            auth: AuthExchange::default(),
            policy,
            identity: custody.identity()?.public_key(),
            custody,
            peer: None,
            recognized: Vec::new(),
            announced: Identity::default(),
            relay: Relay::default(),
            client: Client::default(),
            blobs: BlobExchange::new(blobs),
            spending: SessionSpending::new(spending),
            upgrade: Upgrade::new(role),
            transfer: IdentityTransfer::default(),
        })
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

        // A newly proved identity moves a secured link to Identified; a syncing one stays.
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
        if self.auth.proved.is_empty() {
            return;
        }

        let peer = Peer::bind(self.link, self.auth.proved.iter().copied(), &self.policy);

        if peer.policy.is_blocked() {
            self.close(Ending::Refused);
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

    /// Answer the consent gate's hold, from the user's decision.
    ///
    /// Approving resumes whatever the gate held back; refusing closes the link.
    pub fn approve(&mut self, db: &Db, approved: bool) -> Result<()> {
        if !matches!(self.state, State::GatePending { .. }) {
            return Ok(());
        }

        if !approved {
            self.close(Ending::Refused);
            return Ok(());
        }

        self.gate.passed = true;
        self.state = State::Secured;
        self.identify_by = self.identify_by.max(clock::now() + IDENTIFY_CAP_SECONDS);

        // Resume the deferred turn, if its trigger has already arrived.
        match self.role {
            Role::Receiver => {
                if self.auth.peer_challenge.is_some() {
                    self.send_recognition_tags(db)?;
                    self.challenge_peer()?;
                }
            }
            Role::Dialer => {
                if self.answer_peer_challenge(db)? {
                    self.state = State::DialerIdentified;
                }
            }
        }

        Ok(())
    }

    /// The comparison value to put in front of the user, once per held gate.
    ///
    /// The gate runs before either side has named a pubkey, so this is the only
    /// thing the two users have to check. A transcript that is not available
    /// yet defers the prompt rather than asking without one: a gate with
    /// nothing to compare authenticates nobody.
    pub fn request_approval(&mut self) -> Option<u32> {
        if !matches!(
            self.state,
            State::GatePending {
                approval_requested: false,
                ..
            }
        ) {
            return None;
        }

        let handshake_hash = self.wire.noise.handshake_hash().ok()?;

        if let State::GatePending {
            approval_requested, ..
        } = &mut self.state
        {
            *approval_requested = true;
        }

        Some(sas::sas(
            sas::PAIRING_LABEL,
            &handshake_hash,
            sas::PAIRING_SPACE,
        ))
    }

    /// Whichever pubkeys the peer has proved that the shell has not been told
    /// about yet.
    ///
    /// A pet name typed at the gate is for the person standing there, and this
    /// is what says whose key that turned out to be. A peer may prove more
    /// identities later in the session, so this answers the difference rather
    /// than firing once.
    pub fn take_identified(&mut self) -> Identity {
        let fresh: Identity = self
            .auth
            .proved
            .difference(&self.announced)
            .copied()
            .collect();

        self.announced.extend(fresh.iter().copied());

        fresh
    }

    /// Queue a frame for the peer, encrypted and fragmented by the wire.
    pub fn send(&mut self, frame: &Frame) -> Result<()> {
        self.wire.send(frame)
    }

    /// The next fragment for the shell to write on `pipe`, if the last one
    /// there has been acknowledged. The wire seals it here, in the order it
    /// goes out.
    pub fn next_write(&mut self, pipe: Pipe) -> Result<Option<Vec<u8>>> {
        self.wire.next_write(pipe)
    }

    /// Record that the fragment the shell was handed on `pipe` has been
    /// acknowledged.
    pub fn acknowledge_write(&mut self, pipe: Pipe) {
        self.wire.acknowledge_write(pipe);
    }

    /// Take one write off the characteristic, returning a decrypted frame once
    /// its last fragment lands.
    pub fn receive(&mut self, write: &[u8]) -> Result<Option<Frame>> {
        self.heartbeat.heard();

        self.wire.receive(write)
    }

    /// Take a slice off the L2CAP stream and handle every frame it completes.
    ///
    /// The bulk pipe carries the blob channel and nothing else — the wire
    /// refuses a fragment that says otherwise — so there is no dispatch to
    /// make here, unlike the characteristic.
    pub fn receive_bulk(&mut self, db: &Db, read: &[u8]) -> Result<()> {
        self.heartbeat.heard();

        for frame in self.wire.receive_bulk(read)? {
            self.handle_blob(db, &frame)?;
        }

        Ok(())
    }

    /// Advance the lifecycle on an inbound control frame: the handshake, the
    /// recognition exchange, the consent gate, mutual `AUTH`, the heartbeat.
    ///
    /// Everything up to [`Identified`](State::Identified) happens here.
    pub fn advance(&mut self, db: &Db, frame: &Frame) -> Result<()> {
        match self.state {
            State::Linked => self.advance_handshake(db, frame)?,
            // Control frames keep arriving after identification, so the secured states share one.
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
        Identity::from([self.identity])
    }

    /// The peer's quota for this session.
    ///
    /// A peer that proved any trusted identity is the same device whatever it
    /// signs with, so the trusted budget is the whole session's.
    #[must_use]
    pub fn quota(&self) -> Quota {
        match self.peer.as_ref() {
            Some(peer) if peer.policy.standing == Standing::Trusted => Quota::TRUSTED,
            _ => Quota::STRANGER,
        }
    }

    /// Handle one frame on the sync channel, routing it to the blob, relay or
    /// client half.
    pub fn handle_sync(&mut self, db: &Db, frame: &Frame) -> Result<()> {
        let message = match Message::decode(&frame.payload) {
            Ok(message) => message,
            // One message this build cannot read is one message lost, not the link.
            Err(error) => {
                log::warn!(
                    "dropping a malformed sync message on {:?}: {error:#}",
                    self.link
                );
                return Ok(());
            }
        };

        let Some(peer) = self.peer.as_ref().cloned() else {
            bail!("sync traffic before the peer is identified");
        };

        // Draining finishes what is in flight and starts nothing new.
        if matches!(self.state, State::Draining { .. }) {
            match &message {
                Message::Publish(_) | Message::Req(..) | Message::NegOpen(..) => return Ok(()),
                _ => {}
            }
        }

        let quota = self.quota();

        let replies = if self.is_for_relay(&message) {
            self.relay.handle(
                db,
                &peer,
                &Batch::new(self.identity, &*self.custody),
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

    /// Handle one frame on the blob channel, which carries the two Blossom
    /// verbs and nothing else.
    ///
    /// Blobs are metered against the same quota as the event halves; what the
    /// channel buys them is a queue of their own, and an L2CAP channel to ride
    /// wherever one opens.
    pub fn handle_blob(&mut self, db: &Db, frame: &Frame) -> Result<()> {
        let message = match Message::decode(&frame.payload) {
            Ok(message) => message,
            Err(error) => {
                log::warn!(
                    "dropping a malformed blob message on {:?}: {error:#}",
                    self.link
                );
                return Ok(());
            }
        };

        let Some(peer) = self.peer.as_ref().cloned() else {
            bail!("blob traffic before the peer is identified");
        };

        let quota = self.quota();

        match message {
            Message::BlossomRequest(request) => {
                let reply = self
                    .blobs
                    .serve(db, &peer, &self.local(), &request, quota)?;

                self.send_blob(&Message::BlossomResponse(Box::new(reply)))
            }
            Message::BlossomResponse(response) => {
                let before = self.blobs.fetched_bytes();
                let next = self.blobs.on_response(db, &peer, &response, quota)?;
                let taken = self.blobs.fetched_bytes().saturating_sub(before);

                if taken > 0 {
                    self.spending.record_blob(&peer, taken);
                }

                if let Some(request) = next {
                    self.send_blob(&Message::BlossomRequest(Box::new(request)))?;
                }

                // A finished fetch frees the slot for the next wanted blob.
                self.maybe_fetch_blob(db)
            }
            other => bail!("{other:?} arrived on the blob channel"),
        }
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
            Message::NegMsg(subscription, _) => {
                !self.client.negotiations.contains_key(subscription)
            }
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

        // Opening a diff before the peer is bound starts a sync nothing is authorized to answer.
        if self.peer.is_none() {
            bail!("a negotiation needs the peer to be identified");
        }

        let (negotiation, opening) = crate::sync::client::Negotiation::begin(db, filter)?;

        self.client.open_negotiation(negotiation);
        self.send_sync(&opening)?;

        Ok(())
    }

    /// Once both identities are bound, store the pair secret and open the
    /// reconciliation.
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

        if let Some(peer) = &self.peer {
            self.blobs.carry_fetched(self.spending.blob_spent(peer));
        }

        self.pair(db)?;
        self.begin_negotiation(db, Filter::new())?;
        self.maybe_fetch_blob(db)
    }

    /// Store the pair secret derived from this session against every pubkey the
    /// peer proved and recognition did not.
    ///
    /// A recognized peer already shares a secret with this device and keeps it.
    /// One that was not either never paired or holds a secret this device does
    /// not, because one side missed the end of an earlier session, and both
    /// sides replacing theirs here is what brings the two back into agreement.
    fn pair(&mut self, db: &Db) -> Result<()> {
        let Some(peer) = self.peer.as_ref() else {
            return Ok(());
        };

        let secret = recognition::derive_secret(&self.wire.noise.handshake_hash()?);
        let pubkeys: Vec<PublicKey> = peer
            .pubkeys
            .iter()
            .filter(|pubkey| !self.recognized.contains(pubkey))
            .copied()
            .collect();

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
            self.send_blob(&Message::BlossomRequest(Box::new(request)))?;
        }

        Ok(())
    }

    // ----------------------- The L2CAP upgrade: docs/transport.md, session/l2cap

    /// Start the upgrade if the blob half has asked for one, and answer with
    /// whatever the shell has to do next, once.
    pub fn poll_l2cap(&mut self) -> Result<Option<Step>> {
        if self.blobs.take_l2cap_request() {
            self.want_l2cap()?;
        }

        Ok(self.upgrade.poll())
    }

    /// The shell published a channel, so the PSM goes to the peer.
    pub fn l2cap_published(&mut self, psm: u16) -> Result<()> {
        let announcement = self.upgrade.published(psm);

        self.send_control(control::L2CAP, &announcement)
    }

    /// The channel is up: bulk moves onto it at its MTU.
    ///
    /// A channel too small to carry a fragment is one this link does without,
    /// rather than a reason to end it: the upgrade is bandwidth.
    pub fn l2cap_opened(&mut self, mtu: usize) -> Result<()> {
        if let Err(error) = self.wire.open_bulk(mtu) {
            log::warn!("staying on GATT on {:?}: {error:#}", self.link);

            return self.l2cap_unavailable();
        }

        self.upgrade.opened();

        Ok(())
    }

    /// The channel went away, or the shell could not make one.
    ///
    /// Either way this link finishes on GATT. What the blob channel had on the
    /// bulk pipe is lost, so a fetch waiting on it starts again from the store.
    pub fn l2cap_unavailable(&mut self) -> Result<()> {
        if self.wire.bulk_is_open() {
            self.blobs.abandon_in_flight();
        }

        self.wire.close_bulk();

        let owed = self.upgrade.unavailable();

        self.tell_l2cap(owed)
    }

    /// Bulk is wanted on this link, from whichever end this is.
    fn want_l2cap(&mut self) -> Result<()> {
        let owed = self.upgrade.wanted();

        self.tell_l2cap(owed)
    }

    /// One step of the upgrade exchange, off the control channel.
    fn on_l2cap(&mut self, payload: &[u8]) -> Result<()> {
        let owed = self.upgrade.receive(payload)?;

        self.tell_l2cap(owed)
    }

    /// Send what the upgrade owes the peer, if it owes anything.
    fn tell_l2cap(&mut self, owed: Option<Vec<u8>>) -> Result<()> {
        match owed {
            Some(payload) => self.send_control(control::L2CAP, &payload),
            None => Ok(()),
        }
    }

    // ------------------------------------ Login with device: docs/keys.md

    /// Offer this device's identity to the peer, which is what the user asked
    /// for by tapping.
    ///
    /// Refused unless the app is in the foreground: this is the one flow that
    /// does not run during a background wake, whatever the session state says.
    pub fn offer_identity(&mut self) -> Result<()> {
        if !self.may_transfer() {
            bail!("an identity transfer needs an authenticated session and the app in front");
        }

        let payload = self.transfer.offer()?;

        self.send_control(control::TRANSFER, &payload)
    }

    /// The user answered the prompt, which on both devices is the same
    /// question: does the other one show this number.
    pub fn answer_transfer(&mut self, confirmed: bool) -> Result<()> {
        if confirmed && !self.may_transfer() {
            return self.cancel_transfer();
        }

        let step = self.transfer.answer(confirmed, &self.custody.identity()?)?;

        self.send_transfer(step)
    }

    /// Give up a running identity transfer, telling the peer.
    pub fn cancel_transfer(&mut self) -> Result<()> {
        let step = self.transfer.cancel();

        self.send_transfer(step)
    }

    /// The comparison value to put in front of the user, once per prompt.
    ///
    /// It is the Noise transcript both ends hold and nothing else, so a session
    /// that has not completed one has nothing to show.
    pub fn take_transfer_prompt(&mut self) -> Option<u32> {
        let handshake_hash = self.wire.noise.handshake_hash().ok()?;

        self.transfer.take_prompt(&handshake_hash)
    }

    /// One step of an identity transfer, off the control channel.
    fn on_transfer(&mut self, payload: &[u8]) -> Result<()> {
        let attended = self.may_transfer();
        let step = self
            .transfer
            .receive(payload, &self.custody.identity()?, attended)?;

        self.send_transfer(step)
    }

    /// Send whatever step the exchange answered with, if it answered with one.
    ///
    /// One of those steps carries the identity key, so the payload is wiped
    /// once it is queued; the wire seals its own copy as it leaves.
    fn send_transfer(&mut self, payload: Option<Vec<u8>>) -> Result<()> {
        let Some(payload) = payload.map(Zeroizing::new) else {
            return Ok(());
        };

        self.send_control(control::TRANSFER, &payload)
    }

    /// Whether this device is in a position to move an identity at all: a
    /// session both ends authenticated, and a user in front of the screen.
    fn may_transfer(&self) -> bool {
        self.state == State::Syncing && self.gate.presence == Some(Presence::Foreground)
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

        // The same two tests the relay half applies: the filters, and what the peer may see.
        if !peer.policy.should_gossip(event) {
            return Ok(());
        }

        let subscriptions = self.relay.matching(event);
        let custody = Arc::clone(&self.custody);
        let signer = Batch::new(self.identity, &*custody);

        for subscription in &subscriptions {
            let mut messages = vec![Message::Event(
                subscription.clone(),
                Box::new(event.clone()),
            )];

            relay::attach(db, &peer, &signer, subscription, event, &mut messages)?;

            for message in messages {
                self.send_sync(&message)?;
            }
        }

        if !subscriptions.is_empty() {
            relay::record_shares(db, &peer, std::slice::from_ref(event))?;
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

    /// Tear the session down, recording whose decision it was.
    pub fn close(&mut self, ending: Ending) {
        self.ending = ending;
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
            // A link that has not named anyone cannot sync, so the phase has its own deadline.
            State::Linked | State::Secured | State::DialerIdentified => {
                self.heartbeat.timed_out() || now >= self.identify_by
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
        !self.wire.outbox.is_idle() || self.blobs.is_fetching() || self.client.is_awaiting()
    }

    /// When this session next needs attention, for the shell to arm a timer on.
    #[must_use]
    pub fn deadline(&self) -> i64 {
        match self.state {
            State::Draining { since } => since + DRAIN_CAP_SECONDS,
            State::GatePending { since, .. } => since + GATE_HOLD_SECONDS,
            State::Linked | State::Secured | State::DialerIdentified => {
                self.heartbeat.timeout_deadline().min(self.identify_by)
            }
            _ if self.beats() => self
                .heartbeat
                .next_beat_at
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

        // Reschedule whether or not a beat goes out, so a long transfer does not emit one.
        self.heartbeat.reschedule()?;

        // Traffic on its way out is the heartbeat; a beat would only wait behind it.
        if self.wire.outbox.is_idle() {
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

    // -------------------------------------------------------------- Handshake

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

    // ------------------ Secured: recognition, then the gate, then mutual AUTH

    /// Dispatch one post-handshake control frame by its discriminant byte.
    fn on_control_frame(&mut self, db: &Db, frame: &Frame) -> Result<()> {
        let Some((&discriminant, payload)) = frame.payload.split_first() else {
            bail!("an empty frame arrived on the control channel");
        };

        match discriminant {
            control::TAGS => self.on_tags(db, payload)?,
            control::AUTH => self.on_auth(db, payload)?,
            control::HEARTBEAT => {
                // The write itself proved liveness in `receive`; a beacon carries nothing else.
            }
            control::TRANSFER => self.on_transfer(payload)?,
            control::L2CAP => self.on_l2cap(payload)?,
            other => bail!("an unknown control frame {other} arrived"),
        }

        // Both identities bind once the peer is verified and this device has disclosed.
        if self.peer.is_some() && self.auth.disclosed {
            self.enter_syncing(db)?;
        }

        Ok(())
    }

    /// Resolve the peer's recognition tags and run the consent gate on the
    /// result, before anyone has disclosed a pubkey.
    fn on_tags(&mut self, db: &Db, payload: &[u8]) -> Result<()> {
        // Recognition happens once: a later list would knock a syncing session back to a hold.
        if self.state != State::Secured {
            bail!("recognition tags arrived after the exchange moved past the gate");
        }

        let resolved = self.resolve_recognition(db, payload)?;

        self.recognized.clone_from(&resolved);

        match self.gate.evaluate(db, &self.policy, &resolved)? {
            Verdict::Pass => {}
            Verdict::Blocked => self.close(Ending::Refused),
            Verdict::Pending => {
                self.state = State::GatePending {
                    since: clock::now(),
                    approval_requested: false,
                };
            }
        }

        Ok(())
    }

    /// Dispatch one NIP-42 message, which is a challenge from the peer or its
    /// response to ours.
    fn on_auth(&mut self, db: &Db, payload: &[u8]) -> Result<()> {
        match Message::decode(payload).context("the control channel carried a malformed AUTH")? {
            Message::AuthChallenge(challenge) => self.on_challenge(db, challenge),
            Message::AuthResponse(event) => self.on_auth_response(db, &event),
            other => bail!("{other:?} is not a NIP-42 message"),
        }
    }

    /// Take the peer's challenge. The receiver challenges back without
    /// answering; the dialer answers, identifying first. A challenge arriving
    /// before the gate passes waits; approval resumes the turn.
    fn on_challenge(&mut self, db: &Db, challenge: String) -> Result<()> {
        self.auth.receive_challenge(challenge);

        if !self.gate.passed {
            return Ok(());
        }

        match self.role {
            // The receiver names no pubkey yet, and answers only after it has seen the dialer.
            Role::Receiver => {
                self.send_recognition_tags(db)?;
                self.challenge_peer()?;
            }
            // The dialer identifies itself first.
            Role::Dialer => {
                if self.answer_peer_challenge(db)? {
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
    fn on_auth_response(&mut self, db: &Db, event: &coracle_lib::events::Event) -> Result<()> {
        if !self.gate.passed {
            bail!("an AUTH response arrived before the gate passed");
        }

        let pubkey = self.auth.verify(event, self.wire.noise.local_static)?;

        self.identify([pubkey]);

        if self.state == State::Closed {
            return Ok(());
        }

        // The receiver now discloses, having evaluated the dialer's identity.
        if self.role == Role::Receiver {
            self.answer_peer_challenge(db)?;
        }

        Ok(())
    }

    /// Send a NIP-42 challenge to the peer.
    fn challenge_peer(&mut self) -> Result<()> {
        let challenge = self.auth.make_challenge()?;

        self.send_control(control::AUTH, &Message::AuthChallenge(challenge).encode())
    }

    /// Answer the peer's challenge, disclosing this device's identity — if one
    /// is waiting and this device has not disclosed already.
    ///
    /// This is where the disclosure budget is spent, because it is where the
    /// identity leaves the device. The dialer discloses first and cannot name
    /// the recipient yet, so a peer that collects the auth event and walks away
    /// costs a unit all the same. `docs/policy.md#discoverability`.
    fn answer_peer_challenge(&mut self, db: &Db) -> Result<bool> {
        let remote = self
            .wire
            .noise
            .remote_static
            .ok_or_else(|| anyhow::anyhow!("the handshake has not completed"))?;

        match self.auth.answer(&self.custody.identity()?, remote)? {
            Some(event) => {
                let message = Message::AuthResponse(Box::new(event));

                self.send_control(control::AUTH, &message.encode())?;

                if self.gate.spends_budget {
                    crate::db::command::record_disclosure(db, clock::now())?;
                }

                Ok(true)
            }
            None => Ok(false),
        }
    }

    // ------------------------------------------------------------ Recognition

    /// Send this device's recognition tags: always the constant count, so the
    /// length discloses neither how many peers this device has paired with nor
    /// that it has paired with any.
    fn send_recognition_tags(&mut self, db: &Db) -> Result<()> {
        let hash = self.wire.noise.handshake_hash()?;
        let secrets = crate::db::query::pair_secrets(db)?;
        let tags = recognition::select(&secrets, &hash)?;

        self.identify_by = self
            .identify_by
            .max(clock::now() + GATE_HOLD_SECONDS + IDENTIFY_CAP_SECONDS);

        self.send_control(control::TAGS, &tags.encode())
    }

    /// Trial-MAC the offered tags against every pair secret this device holds,
    /// returning every pubkey they resolve to.
    fn resolve_recognition(&self, db: &Db, payload: &[u8]) -> Result<Vec<PublicKey>> {
        let offered = recognition::Tags::decode(payload)?;
        let hash = self.wire.noise.handshake_hash()?;
        let secrets = crate::db::query::pair_secrets(db)?;

        Ok(recognition::resolve(&offered, &secrets, &hash))
    }

    // -------------------------------------------------------- Sending helpers

    /// Queue a control frame, encrypted, with its discriminant ahead of the
    /// payload.
    fn send_control(&mut self, discriminant: u8, payload: &[u8]) -> Result<()> {
        let mut frame = Frame {
            channel: Channel::Control,
            payload: Vec::with_capacity(payload.len() + 1),
        };
        frame.payload.push(discriminant);
        frame.payload.extend_from_slice(payload);

        let sent = self.send(&frame);

        // A control frame can carry the identity key, so the copy here goes with the call.
        frame.payload.zeroize();

        sent
    }

    /// Queue a sync message as a frame on the sync channel.
    fn send_sync(&mut self, message: &Message) -> Result<()> {
        let payload = message.encode();

        self.send(&Frame {
            channel: Channel::Sync,
            payload,
        })
    }

    /// Queue a Blossom message as a frame on the blob channel, which is what
    /// rides L2CAP once one is open.
    fn send_blob(&mut self, message: &Message) -> Result<()> {
        let payload = message.encode();

        self.send(&Frame {
            channel: Channel::Blob,
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gate::Presence;

    use crate::db::query as db_query;
    use coracle_lib::keys::SecretKey;

    use crate::fixtures::{author, custody, note, secret};
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
            custody(identity()),
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
            custody(secret(2)),
            blobs(),
            spending(),
        )
        .unwrap();

        session.wire.initiate().unwrap();

        while !(session.wire.noise.is_complete() && peer.wire.noise.is_complete()) {
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
        while let Some(write) = from.next_write(Pipe::Gatt).unwrap() {
            from.acknowledge_write(Pipe::Gatt);

            if let Some(frame) = into.wire.receive(&write).unwrap() {
                into.wire.read_handshake(&frame.payload).unwrap();
            }
        }
    }

    /// Move fragments from one session to the other until a whole frame lands.
    fn pump_frame(from: &mut Session, into: &mut Session) -> Frame {
        while let Some(write) = from.next_write(Pipe::Gatt).unwrap() {
            from.acknowledge_write(Pipe::Gatt);

            if let Some(frame) = into.wire.receive(&write).unwrap() {
                return frame;
            }
        }

        panic!("nothing was queued to pump");
    }

    #[test]
    fn nothing_is_proved_before_the_peer_names_itself() {
        assert!(session(policy()).peer.is_none());
    }

    #[test]
    fn identifying_binds_the_policy_to_every_pubkey_the_peer_proved() {
        let mut session = secured_session(policy());

        session.identify([author(2), author(3)]);

        let peer = session.peer.as_ref().unwrap();

        assert_eq!(peer.pubkeys.len(), 2);
        assert_eq!(session.state, State::Identified);
    }

    #[test]
    fn proving_one_pubkey_twice_does_not_lengthen_the_set() {
        let mut session = secured_session(policy());

        session.identify([author(2), author(2)]);

        assert_eq!(session.peer.as_ref().unwrap().pubkeys.len(), 1);
    }

    #[test]
    fn a_peer_holding_a_blocked_identity_is_refused_whatever_else_it_proved() {
        let mut policy = policy();
        policy.graph.blocked.insert(author(3));

        let mut session = secured_session(policy);

        session.identify([author(2), author(3)]);

        assert!(session.peer.as_ref().unwrap().policy.is_blocked());
        assert_eq!(session.state, State::Closed);
    }

    #[test]
    fn a_policy_change_leaves_an_unidentified_session_alone() {
        // Mid-handshake, nothing proved: a preference toggle must not move the lifecycle.
        let mut session = session(policy());

        session.set_policy(Arc::new(policy()));

        assert_eq!(session.state, State::Linked);
        assert!(session.peer.as_ref().is_none());
    }

    #[test]
    fn a_policy_change_leaves_a_draining_session_draining() {
        let mut session = secured_session(policy());
        session.identify([author(2)]);
        session.drain();

        session.set_policy(Arc::new(policy()));

        assert!(matches!(session.state, State::Draining { .. }));
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
        while session.next_write(Pipe::Gatt).unwrap().is_some() {
            session.acknowledge_write(Pipe::Gatt);
        }

        assert!(clock::at(1_001, || session.expired()));
    }

    #[test]
    fn a_drain_waits_on_a_fetch_the_outbox_cannot_see() {
        // A blob fetch leaves the outbox empty between request and answer; the drain must wait.
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
        while session.next_write(Pipe::Gatt).unwrap().is_some() {
            session.acknowledge_write(Pipe::Gatt);
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
        // A peer that completes the handshake and only beats is never gated and never times out.
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
        // Re-running recognition on a syncing session would prompt the user a second time.
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
        assert_eq!(session.state, State::Syncing);
    }

    #[test]
    fn blocking_a_peer_mid_session_closes_it() {
        let mut session = secured_session(policy());

        session.identify([author(2)]);
        assert_eq!(session.state, State::Identified);

        let mut blocked = policy();
        blocked.graph.blocked.insert(author(2));

        session.set_policy(Arc::new(blocked));

        assert!(session.peer.as_ref().unwrap().policy.is_blocked());
        assert_eq!(session.state, State::Closed);
    }

    #[test]
    fn a_trusted_identity_earns_the_trusted_budget_whichever_key_it_proved() {
        let mut policy = policy();
        policy.graph.trusted.insert(author(9));
        let mut session = secured_session(policy);

        // The trusted pubkey sorts after the stranger one, which a "first policy" read would pick.
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
            assert!(session.next_write(Pipe::Gatt).unwrap().is_none());
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
        let fragment = session.next_write(Pipe::Gatt).unwrap().unwrap();
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

        assert_eq!(session.state, State::Identified);
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
        let mut session = secured_session(policy());
        session.state = State::GatePending {
            since: clock::now(),
            approval_requested: false,
        };

        assert!(session.request_approval().is_some());
        assert_eq!(session.request_approval(), None, "the prompt is one-shot");

        session.approve(&db, false).unwrap();
        assert_eq!(session.state, State::Closed);
    }

    #[test]
    fn a_gate_with_no_transcript_behind_it_asks_nothing() {
        let mut session = session(policy());
        session.state = State::GatePending {
            since: clock::now(),
            approval_requested: false,
        };

        assert_eq!(session.request_approval(), None);
        assert!(
            session.request_approval().is_none(),
            "and it is still owed once the handshake completes"
        );
    }

    #[test]
    fn both_ends_of_a_gate_compare_the_same_value() {
        let (mut session, mut peer) = secured_pair(policy());

        for end in [&mut session, &mut peer] {
            end.state = State::GatePending {
                since: clock::now(),
                approval_requested: false,
            };
        }

        assert_eq!(session.request_approval(), peer.request_approval());
    }

    #[test]
    fn a_peer_is_announced_once_per_pubkey_it_proves() {
        let mut session = secured_session(policy());

        assert!(session.take_identified().is_empty());

        session.identify([author(2)]);

        assert_eq!(
            session.take_identified(),
            Identity::from([author(2)]),
            "the first proof is announced"
        );
        assert!(session.take_identified().is_empty());

        session.identify([author(2), author(3)]);

        assert_eq!(
            session.take_identified(),
            Identity::from([author(3)]),
            "and a later one announces only what is new"
        );
    }

    #[test]
    fn an_auth_response_during_the_gate_hold_is_refused() {
        let db = Db::open_in_memory().unwrap();
        let mut session = session(policy());
        session.state = State::GatePending {
            since: clock::now(),
            approval_requested: false,
        };

        // A held stranger volunteering an identity is refused by the gate, not the decoder.
        let hashed = crate::fixtures::event(author(2), 22_242, 100, "", NostrTags::new());
        let signed = hashed
            .clone()
            .with_sig(secret(2).sign(hashed.id.as_bytes()));
        let mut payload = vec![control::AUTH];
        payload.extend_from_slice(&Message::AuthResponse(Box::new(signed)).encode());

        let frame = Frame {
            channel: crate::transport::Channel::Control,
            payload,
        };

        assert!(session.advance(&db, &frame).is_err());
        assert!(session.peer.as_ref().is_none());
    }

    #[test]
    fn auth_travels_as_the_nip_42_message() {
        // NIP-42's own `["AUTH", …]` on the control channel. `docs/nips/p2p-auth.md`.
        let db = Db::open_in_memory().unwrap();
        let (mut dialer, mut receiver) = secured_pair(policy());

        dialer.challenge_peer().unwrap();

        let frame = pump_frame(&mut dialer, &mut receiver);
        let (discriminant, payload) = frame.payload.split_first().unwrap();

        assert_eq!(*discriminant, control::AUTH);
        assert!(matches!(
            Message::decode(payload).unwrap(),
            Message::AuthChallenge(_)
        ));

        // The response side: the dialer identifies first, so it answers a challenge outright.
        dialer.gate.passed = true;
        dialer
            .advance(
                &db,
                &Frame {
                    channel: crate::transport::Channel::Control,
                    payload: [
                        &[control::AUTH][..],
                        &Message::AuthChallenge("abc123".into()).encode(),
                    ]
                    .concat(),
                },
            )
            .unwrap();

        let response = pump_frame(&mut dialer, &mut receiver);
        let (discriminant, payload) = response.payload.split_first().unwrap();

        assert_eq!(*discriminant, control::AUTH);
        assert!(matches!(
            Message::decode(payload).unwrap(),
            Message::AuthResponse(_)
        ));
        assert_eq!(dialer.state, State::DialerIdentified);
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

        // No OK travels back and nothing is stored: the publish waits for the next encounter.
        assert!(session.next_write(Pipe::Gatt).unwrap().is_none());
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

        // This device opens the negotiation, so its own bookkeeping routes the reply.
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

        // Routed to the client half rather than misread as a reply this device never hosted.
        session.handle_sync(&db, &reply).unwrap();
        assert!(session.next_write(Pipe::Gatt).unwrap().is_some());
    }

    #[test]
    fn a_full_handshake_moves_a_pair_to_secured() {
        // A big MTU so every handshake message is one fragment.
        let mut dialer = Session::open(
            LinkId(1),
            Role::Dialer,
            4096,
            Arc::new(policy()),
            custody(secret(1)),
            blobs(),
            spending(),
        )
        .unwrap();
        let mut receiver = Session::open(
            LinkId(2),
            Role::Receiver,
            4096,
            Arc::new(policy()),
            custody(secret(2)),
            blobs(),
            spending(),
        )
        .unwrap();

        dialer.initiate().unwrap();
        let msg1 = dialer.next_write(Pipe::Gatt).unwrap().unwrap();
        dialer.acknowledge_write(Pipe::Gatt);

        let frame = receiver.receive(&msg1).unwrap().unwrap();
        receiver
            .advance(&Db::open_in_memory().unwrap(), &frame)
            .unwrap();

        let reply = receiver.next_write(Pipe::Gatt).unwrap().unwrap();
        receiver.acknowledge_write(Pipe::Gatt);

        let frame = dialer.receive(&reply).unwrap().unwrap();
        dialer
            .advance(&Db::open_in_memory().unwrap(), &frame)
            .unwrap();

        // Only the dialer's final handshake message is part of the exchange under test.
        let final_msg = dialer.next_write(Pipe::Gatt).unwrap().unwrap();
        dialer.acknowledge_write(Pipe::Gatt);

        let frame = receiver.receive(&final_msg).unwrap().unwrap();
        receiver
            .advance(&Db::open_in_memory().unwrap(), &frame)
            .unwrap();

        assert!(dialer.wire.noise.is_complete());
        assert!(receiver.wire.noise.is_complete());
        assert_eq!(dialer.state, State::Secured);
        assert_eq!(receiver.state, State::Secured);
    }

    /// Pump one queued fragment from `sender` into `receiver`, advancing it.
    fn pump(sender: &mut Session, receiver: &mut Session, db: &Db) {
        let fragment = sender
            .next_write(Pipe::Gatt)
            .unwrap()
            .expect("a queued fragment to pump");
        sender.acknowledge_write(Pipe::Gatt);

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

        assert_eq!(dialer.state, State::Syncing);
        assert_eq!(receiver.state, State::Syncing);
    }

    fn pair(mtu: usize, role: Role, key: u8) -> Session {
        Session::open(
            LinkId(1),
            role,
            mtu,
            Arc::new(policy()),
            custody(secret(key)),
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
        dialer.gate.presence = Some(Presence::Foreground);
        receiver.gate.presence = Some(Presence::Foreground);

        dialer.initiate().unwrap();
        pump(&mut dialer, &mut receiver, &db); // handshake msg1
        pump(&mut receiver, &mut dialer, &db); // handshake reply

        // Dialer is now Secured and has queued: msg3, tags, challenge.
        pump(&mut dialer, &mut receiver, &db); // msg3: receiver completes handshake
        pump(&mut dialer, &mut receiver, &db); // tags: receiver resolves, admits
        pump(&mut dialer, &mut receiver, &db); // challenge: receiver tags + challenges back

        // Receiver queued tags and its own challenge; the dialer passes the gate and answers.
        pump(&mut receiver, &mut dialer, &db); // tags: dialer resolves, admits
        pump(&mut receiver, &mut dialer, &db); // challenge: dialer answers (identifies)

        // The dialer's response lets the receiver identify it and disclose in turn.
        pump(&mut dialer, &mut receiver, &db); // response: receiver identifies + discloses
        pump(&mut receiver, &mut dialer, &db); // response: dialer identifies

        assert_eq!(dialer.state, State::Syncing);
        assert_eq!(receiver.state, State::Syncing);
        assert_eq!(
            dialer
                .peer
                .as_ref()
                .unwrap()
                .pubkeys
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![author(2)]
        );
        assert_eq!(
            receiver
                .peer
                .as_ref()
                .unwrap()
                .pubkeys
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![author(1)]
        );

        // The payload is encrypted; the frame header is not.
        let opening = dialer.next_write(Pipe::Gatt).unwrap().unwrap();
        assert_eq!(opening[0], Channel::Sync as u8);
    }

    #[test]
    fn pairing_stores_the_same_secret_on_both_sides() {
        let db = Db::open_in_memory().unwrap();
        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);

        dialer.gate.presence = Some(Presence::Foreground);
        receiver.gate.presence = Some(Presence::Foreground);

        full_exchange(&mut dialer, &mut receiver, &db);

        // Each side stored a secret for the other, both derived from the shared handshake hash.
        let secrets = crate::db::query::pair_secrets(&db).unwrap();
        assert_eq!(secrets.len(), 2);
        assert_eq!(secrets[0].1, secrets[1].1);
    }

    /// [`full_exchange`] between two devices that each keep their own store.
    fn exchange_apart(
        dialer: &mut Session,
        receiver: &mut Session,
        dialer_db: &Db,
        receiver_db: &Db,
    ) {
        dialer.initiate().unwrap();
        pump(dialer, receiver, receiver_db); // handshake msg1
        pump(receiver, dialer, dialer_db); // handshake reply
        pump(dialer, receiver, receiver_db); // msg3
        pump(dialer, receiver, receiver_db); // dialer tags
        pump(dialer, receiver, receiver_db); // dialer challenge
        pump(receiver, dialer, dialer_db); // receiver tags
        pump(receiver, dialer, dialer_db); // receiver challenge
        pump(dialer, receiver, receiver_db); // dialer response
        pump(receiver, dialer, dialer_db); // receiver response

        assert_eq!(dialer.state, State::Syncing);
        assert_eq!(receiver.state, State::Syncing);
    }

    #[test]
    fn pair_secrets_that_disagree_are_replaced_on_the_next_encounter() {
        let dialer_db = Db::open_in_memory().unwrap();
        let receiver_db = Db::open_in_memory().unwrap();

        // The receiver holds a secret the dialer never learned, as if the dialer missed the end of a session.
        crate::db::command::pair_with(&receiver_db, &[author(1)], &[9u8; 32], 0).unwrap();

        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);
        dialer.gate.presence = Some(Presence::Foreground);
        receiver.gate.presence = Some(Presence::Foreground);
        exchange_apart(&mut dialer, &mut receiver, &dialer_db, &receiver_db);

        let held = |db: &Db| crate::db::query::pair_secrets(db).unwrap()[0].1;
        assert_eq!(held(&dialer_db), held(&receiver_db));

        // Agreeing again, they recognize each other with nobody in front of the screen.
        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);
        exchange_apart(&mut dialer, &mut receiver, &dialer_db, &receiver_db);
    }

    #[test]
    fn a_paired_peer_is_recognized_on_the_next_encounter() {
        let db = Db::open_in_memory().unwrap();

        // First encounter pairs them, admitted by the cool-off.
        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);
        dialer.gate.presence = Some(Presence::Foreground);
        receiver.gate.presence = Some(Presence::Foreground);
        full_exchange(&mut dialer, &mut receiver, &db);

        // Second encounter: no cool-off, but the tags resolve, so the gate passes silently.
        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);
        full_exchange(&mut dialer, &mut receiver, &db);
    }

    #[test]
    fn a_stranger_without_admission_is_held_and_can_be_approved() {
        let db = Db::open_in_memory().unwrap();
        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);

        // Only the dialer's cool-off runs, so the receiver holds the dialer for the user.
        dialer.gate.presence = Some(Presence::Foreground);

        dialer.initiate().unwrap();
        pump(&mut dialer, &mut receiver, &db); // msg1
        pump(&mut receiver, &mut dialer, &db); // reply
        pump(&mut dialer, &mut receiver, &db); // msg3
        pump(&mut dialer, &mut receiver, &db); // dialer tags: the gate holds
        pump(&mut dialer, &mut receiver, &db); // dialer challenge: held too

        assert!(matches!(receiver.state, State::GatePending { .. }));
        assert!(
            !receiver.auth.disclosed,
            "nothing disclosed before approval"
        );

        // The user admits the stranger, and the exchange completes.
        receiver.approve(&db, true).unwrap();
        pump(&mut receiver, &mut dialer, &db); // receiver tags: dialer admits
        pump(&mut receiver, &mut dialer, &db); // receiver challenge: dialer answers
        pump(&mut dialer, &mut receiver, &db); // dialer response: receiver discloses
        pump(&mut receiver, &mut dialer, &db); // receiver response: dialer identifies

        assert_eq!(dialer.state, State::Syncing);
        assert_eq!(receiver.state, State::Syncing);

        // The dialer's cool-off admitted a stranger, so its disclosure is the budget's.
        assert_eq!(crate::db::query::disclosures_since(&db, 0).unwrap(), 1);
    }

    #[test]
    fn the_side_waiting_on_a_held_gate_outlasts_the_hold() {
        let db = Db::open_in_memory().unwrap();
        let mut dialer = clock::at(1_000, || pair(4096, Role::Dialer, 1));
        let mut receiver = clock::at(1_000, || pair(4096, Role::Receiver, 2));

        dialer.gate.presence = Some(Presence::Foreground);

        clock::at(1_000, || {
            dialer.initiate().unwrap();
            pump(&mut dialer, &mut receiver, &db); // msg1
            pump(&mut receiver, &mut dialer, &db); // reply
            pump(&mut dialer, &mut receiver, &db); // msg3
            pump(&mut dialer, &mut receiver, &db); // dialer tags: the gate holds
            pump(&mut dialer, &mut receiver, &db); // dialer challenge: held too
        });

        assert!(matches!(receiver.state, State::GatePending { .. }));

        // The dialer waits out the whole hold rather than its identify cap.
        clock::at(1_000 + GATE_HOLD_SECONDS - 1, || {
            dialer.heartbeat.heard();
            assert!(
                !dialer.expired(),
                "the waiting side gave up inside the hold"
            );
        });
    }

    #[test]
    fn a_gate_approved_late_still_completes() {
        let db = Db::open_in_memory().unwrap();
        let mut dialer = clock::at(1_000, || pair(4096, Role::Dialer, 1));
        let mut receiver = clock::at(1_000, || pair(4096, Role::Receiver, 2));

        dialer.gate.presence = Some(Presence::Foreground);

        clock::at(1_000, || {
            dialer.initiate().unwrap();
            pump(&mut dialer, &mut receiver, &db); // msg1
            pump(&mut receiver, &mut dialer, &db); // reply
            pump(&mut dialer, &mut receiver, &db); // msg3
            pump(&mut dialer, &mut receiver, &db); // dialer tags: the gate holds
            pump(&mut dialer, &mut receiver, &db); // dialer challenge: held too
        });

        let late = 1_000 + GATE_HOLD_SECONDS - 1;

        clock::at(late, || {
            receiver.approve(&db, true).unwrap();
            receiver.heartbeat.heard();
            assert!(
                !receiver.expired(),
                "an approval past the identify cap closed the link"
            );

            pump(&mut receiver, &mut dialer, &db); // receiver tags: dialer admits
            pump(&mut receiver, &mut dialer, &db); // receiver challenge: dialer answers
            pump(&mut dialer, &mut receiver, &db); // dialer response: receiver discloses
            pump(&mut receiver, &mut dialer, &db); // receiver response: dialer identifies
        });

        assert_eq!(dialer.state, State::Syncing);
        assert_eq!(receiver.state, State::Syncing);
    }

    #[test]
    fn a_peer_that_collects_the_auth_event_and_walks_away_spends_the_budget() {
        // The harvester is passive and never reaches Syncing, so the disclosure is what costs.
        let db = Db::open_in_memory().unwrap();
        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);

        dialer.gate.presence = Some(Presence::Foreground);
        receiver.gate.presence = Some(Presence::Foreground);

        dialer.initiate().unwrap();
        pump(&mut dialer, &mut receiver, &db); // handshake msg1
        pump(&mut receiver, &mut dialer, &db); // handshake reply
        pump(&mut dialer, &mut receiver, &db); // msg3
        pump(&mut dialer, &mut receiver, &db); // dialer tags
        pump(&mut dialer, &mut receiver, &db); // dialer challenge
        pump(&mut receiver, &mut dialer, &db); // receiver tags: dialer admits
        pump(&mut receiver, &mut dialer, &db); // receiver challenge: dialer answers

        assert_eq!(dialer.state, State::DialerIdentified);
        assert_eq!(crate::db::query::disclosures_since(&db, 0).unwrap(), 1);
    }

    #[test]
    fn a_recognized_peer_spends_nothing_on_a_later_encounter() {
        let db = Db::open_in_memory().unwrap();

        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);
        dialer.gate.presence = Some(Presence::Foreground);
        receiver.gate.presence = Some(Presence::Foreground);
        full_exchange(&mut dialer, &mut receiver, &db);

        // Both sides were strangers and both disclosed.
        assert_eq!(crate::db::query::disclosures_since(&db, 0).unwrap(), 2);

        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);
        full_exchange(&mut dialer, &mut receiver, &db);

        assert_eq!(crate::db::query::disclosures_since(&db, 0).unwrap(), 2);
    }

    #[test]
    fn the_upgrade_exchange_crosses_on_the_control_channel() {
        let db = Db::open_in_memory().unwrap();
        let (mut dialer, mut receiver) = secured_pair(policy());

        // The central cannot publish, so wanting bulk is a request rather than a PSM.
        dialer.want_l2cap().unwrap();
        assert_eq!(dialer.poll_l2cap().unwrap(), None);

        pump(&mut dialer, &mut receiver, &db);
        assert_eq!(receiver.poll_l2cap().unwrap(), Some(Step::Publish));

        receiver.l2cap_published(0x0080).unwrap();
        pump(&mut receiver, &mut dialer, &db);
        assert_eq!(dialer.poll_l2cap().unwrap(), Some(Step::Open(0x0080)));

        dialer.l2cap_opened(512).unwrap();
        assert!(dialer.wire.bulk_is_open());
    }

    #[test]
    fn a_central_asked_to_publish_says_it_cannot_rather_than_dropping_the_link() {
        let db = Db::open_in_memory().unwrap();
        let (mut dialer, mut receiver) = secured_pair(policy());

        receiver
            .send_control(control::L2CAP, &[l2cap::message::REQUEST])
            .unwrap();
        pump(&mut receiver, &mut dialer, &db);

        assert_eq!(dialer.state, State::Secured);

        // The refusal travels back, so the receiver stops waiting on a channel.
        pump(&mut dialer, &mut receiver, &db);
        receiver.want_l2cap().unwrap();
        assert_eq!(receiver.poll_l2cap().unwrap(), None);
    }

    #[test]
    fn a_refused_upgrade_leaves_both_ends_on_gatt() {
        let db = Db::open_in_memory().unwrap();
        let (mut dialer, mut receiver) = secured_pair(policy());

        dialer.want_l2cap().unwrap();
        pump(&mut dialer, &mut receiver, &db);
        receiver.poll_l2cap().unwrap();

        receiver.l2cap_unavailable().unwrap();
        pump(&mut receiver, &mut dialer, &db);

        // The dialer heard it: a PSM arriving afterwards is not acted on.
        receiver.l2cap_published(0x0080).unwrap();
        pump(&mut receiver, &mut dialer, &db);

        assert_eq!(dialer.poll_l2cap().unwrap(), None);
        assert!(!dialer.wire.bulk_is_open());
        assert!(!receiver.wire.bulk_is_open());
    }

    #[test]
    fn losing_the_channel_mid_write_leaves_nothing_in_flight() {
        let (mut dialer, _receiver) = secured_pair(policy());

        dialer.l2cap_opened(512).unwrap();
        dialer
            .send(&Frame {
                channel: Channel::Blob,
                payload: b"bytes".to_vec(),
            })
            .unwrap();
        assert!(dialer.wire.next_write(Pipe::Bulk).unwrap().is_some());

        // Nobody acknowledges a write on a channel that is gone, so the close has to.
        dialer.l2cap_unavailable().unwrap();

        assert!(dialer.wire.outbox.is_idle());
        assert!(!dialer.has_work_in_flight());
    }

    /// Two sessions synchronizing, both with a user in front of the screen.
    fn attended_pair(db: &Db) -> (Session, Session) {
        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);

        dialer.gate.presence = Some(Presence::Foreground);
        receiver.gate.presence = Some(Presence::Foreground);
        full_exchange(&mut dialer, &mut receiver, db);

        (dialer, receiver)
    }

    #[test]
    fn a_sync_message_this_build_cannot_read_is_dropped_rather_than_the_link() {
        let db = Db::open_in_memory().unwrap();
        let (_dialer, mut receiver) = attended_pair(&db);

        let garbage = Frame {
            channel: Channel::Sync,
            payload: br#"["NOT-A-VERB", 1, 2]"#.to_vec(),
        };

        receiver.handle_sync(&db, &garbage).unwrap();

        assert_eq!(receiver.state, State::Syncing);
    }

    #[test]
    fn an_identity_moves_over_the_control_channel() {
        let db = Db::open_in_memory().unwrap();
        let (mut source, mut target) = attended_pair(&db);

        source.offer_identity().unwrap();
        pump(&mut source, &mut target, &db);

        // The number is the transcript's, so both ends derive the same one.
        let shown = source.take_transfer_prompt().unwrap();
        assert_eq!(target.take_transfer_prompt(), Some(shown));

        target.answer_transfer(true).unwrap();
        pump(&mut target, &mut source, &db);

        source.answer_transfer(true).unwrap();
        pump(&mut source, &mut target, &db);

        assert_eq!(
            target.transfer.take_identity().map(|key| key.to_hex()),
            Some(secret(1).to_hex())
        );
        assert_eq!(
            target.transfer.take_outcome(),
            Some(transfer::Outcome::Received)
        );
    }

    #[test]
    fn a_backgrounded_device_neither_offers_an_identity_nor_takes_one() {
        let db = Db::open_in_memory().unwrap();
        let (mut source, mut target) = attended_pair(&db);

        source.offer_identity().unwrap();
        target.gate.presence = Some(Presence::Background { since: 0 });
        pump(&mut source, &mut target, &db);

        // Nobody is looking at the target's screen, so it declines rather than holding.
        assert!(!target.transfer.running());
        assert_eq!(target.take_transfer_prompt(), None);

        pump(&mut target, &mut source, &db);
        assert_eq!(
            source.transfer.take_outcome(),
            Some(transfer::Outcome::Refused)
        );

        source.gate.presence = Some(Presence::Background { since: 0 });
        assert!(source.offer_identity().is_err());
    }
}
