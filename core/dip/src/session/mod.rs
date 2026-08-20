//! One link's lifecycle, from a GATT connection to a synchronizing peer.
//!
//! A session is keyed on [`LinkId`]. Nothing here performs I/O.

pub mod peer;
pub mod recognition;

pub use peer::Peer;
pub use recognition::{Tag, Tags};

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use coracle_lib::events::{EventContent, HashedEvent};
use coracle_lib::filters::Filter;
use coracle_lib::keys::{PublicKey, SecretKey};
use coracle_lib::tags::Tags as NostrTags;

use crate::blobstore::BlobStore;
use crate::clock;
use crate::db::Db;
use crate::link::{LinkId, Role};
use crate::model::{Blob, DISCLOSURE_WINDOW_SECONDS, Policy, Standing};
use crate::sync::blob::{BLOB_GROUP_BYTES, BlobFetch};
use crate::sync::client::Used;
use crate::sync::relay;
use crate::sync::{Message, Quota, SubscriptionId};
use crate::transport::{Channel, Codec, Frame, Noise, Outbox};

/// How long a session sits in [`Draining`](State::Draining) before it is closed.
pub const DRAIN_CAP_SECONDS: i64 = 300;

/// How long without a heartbeat before an idle session drains.
pub const HEARTBEAT_TIMEOUT_SECONDS: i64 = 60;

/// The low bound on the jittered heartbeat interval. `docs/discovery.md`.
pub const HEARTBEAT_MIN_SECONDS: i64 = 15;
/// The high bound on the jittered heartbeat interval.
pub const HEARTBEAT_MAX_SECONDS: i64 = 30;

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
    /// Waiting on the user to approve an unadmitted stranger, up to the drain
    /// cap.
    GatePending,
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

/// What the consent gate decided about a peer.
enum Gate {
    /// Admitted, or already paired.
    Pass,
    /// Blocked; drop the link before either side names itself.
    Blocked,
    /// An unadmitted stranger; hold for the user.
    Pending,
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
    /// When the next heartbeat is due, so a quiet session still proves it is
    /// alive before the peer's own timeout drains it.
    next_heartbeat_at: i64,
    /// When the session entered [`Draining`](State::Draining).
    draining_since: Option<i64>,
    /// The challenge this device sent, to match against the peer's response.
    sent_challenge: Option<String>,
    /// The peer's challenge, to sign and return.
    peer_challenge: Option<String>,
    /// Whether this device has disclosed its own identity this session, by
    /// answering the peer's challenge.
    disclosed: bool,
    /// Whether the recognition gate has passed for this peer.
    gate_passed: bool,
    /// When the session entered [`GatePending`](State::GatePending), if it did.
    pending_since: Option<i64>,
    /// Whether the shell has been asked to approve this session yet.
    approval_requested: bool,
    /// When the app was last foregrounded, for the cool-off admission window.
    cool_off_since: Option<i64>,
    /// Every pubkey the peer has proved, accumulated across `AUTH` responses.
    proved_pubkeys: BTreeSet<PublicKey>,
    /// What this device has accepted from the peer this session.
    used: Used,
    /// Open subscriptions from the peer's relay half, with every filter each
    /// one holds.
    subscriptions: BTreeMap<SubscriptionId, Vec<Filter>>,
    /// The client half's state: negotiations, fetches, and the events awaiting
    /// their proofs.
    client: crate::sync::client::Client,
    /// Where blob bytes go, provided by the shell.
    blobs: Arc<dyn BlobStore>,
    /// The blob this session is fetching, if one is in flight.
    blob_fetch: Option<BlobFetch>,
    /// The battery level in percent, as the shell last reported it.
    battery: Option<u8>,
    /// Whether the shell has been asked to open L2CAP for this link.
    l2cap_requested: bool,
    /// Whether the one reconciliation this session opens has already been
    /// opened.
    sync_started: bool,
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
            next_heartbeat_at: clock::now() + jittered_heartbeat_interval(),
            draining_since: None,
            sent_challenge: None,
            peer_challenge: None,
            disclosed: false,
            gate_passed: false,
            pending_since: None,
            approval_requested: false,
            cool_off_since: None,
            proved_pubkeys: BTreeSet::new(),
            used: Used::default(),
            subscriptions: BTreeMap::new(),
            client: crate::sync::client::Client::default(),
            blobs,
            blob_fetch: None,
            battery: None,
            l2cap_requested: false,
            sync_started: false,
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

    /// Record what the peer proved over `AUTH`, binding policy to each pubkey.
    ///
    /// A peer may authenticate as several pubkeys across several responses, so
    /// this accumulates rather than replacing. A peer that turns out to be
    /// blocked is refused here: the consent gate ran before anyone was named.
    pub fn identify(&mut self, pubkeys: impl IntoIterator<Item = PublicKey>) {
        self.proved_pubkeys.extend(pubkeys);
        self.rebind();
    }

    /// Rebind the peer from the pubkeys it has proved, under the current
    /// policy, closing the session if any of them is blocked.
    fn rebind(&mut self) {
        self.peer = Some(Peer::bind(
            self.link,
            self.proved_pubkeys.iter().copied(),
            &self.policy,
        ));

        self.state = if self.peer.as_ref().is_some_and(Peer::is_blocked) {
            self.sync_started = false;
            State::Closed
        } else {
            State::Identified
        };
    }

    /// Take a policy the user has just changed, and rebind it.
    ///
    /// A session follows the user's preferences rather than caching them,
    /// because policy is read at encounter time with nobody watching.
    pub fn set_policy(&mut self, policy: Arc<Policy>) {
        self.policy = policy;
        self.rebind();

        // A rebind leaves an already-syncing session at Identified; put it
        // back without re-opening the reconciliation it has already done.
        if self.state == State::Identified && self.sync_started {
            self.state = State::Syncing;
        }
    }

    /// When the app was last foregrounded, which the cool-off admission window
    /// is measured from. `None` means the cool-off is not running.
    pub fn set_cool_off_since(&mut self, since: Option<i64>) {
        self.cool_off_since = since;
    }
    /// Answer the consent gate's hold, from the user's decision.
    ///
    /// Approving resumes whatever the gate held back; refusing closes the link.
    pub fn approve(&mut self, db: &Db, approved: bool) -> Result<()> {
        if self.state != State::GatePending {
            return Ok(());
        }

        if !approved {
            self.state = State::Closed;
            return Ok(());
        }

        self.gate_passed = true;
        self.pending_since = None;
        self.state = State::Secured;

        // Resume the deferred turn, if its trigger has already arrived.
        match self.role {
            Role::Receiver if self.peer_challenge.is_some() => {
                self.send_recognition_tags(db)?;
                self.send_auth_challenge()?;
            }
            Role::Dialer => {
                if let Some(challenge) = self.peer_challenge.clone() {
                    self.send_auth_response(&challenge)?;
                    self.disclosed = true;
                    self.state = State::DialerIdentified;
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Whether the shell should be told to prompt for this session, once.
    pub fn request_approval(&mut self) -> bool {
        if self.state == State::GatePending && !self.approval_requested {
            self.approval_requested = true;
            return true;
        }

        false
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
            State::Linked => self.advance_handshake(db, frame)?,
            // Control frames keep arriving after identification — heartbeats
            // above all, and a peer proving a second identity — so the secured
            // states share one dispatcher, including the gate hold.
            State::Secured
            | State::DialerIdentified
            | State::Identified
            | State::Syncing
            | State::GatePending => self.on_control_frame(db, frame)?,
            _ => {}
        }

        Ok(())
    }

    /// The peer's quota for this session.
    ///
    /// A peer that proved any trusted identity is the same device whatever it
    /// signs with, so the trusted budget is the whole session's.
    #[must_use]
    pub fn quota(&self) -> Quota {
        match self.peer.as_ref() {
            Some(peer)
                if peer
                    .policies
                    .iter()
                    .any(|policy| policy.standing() == Standing::Trusted) =>
            {
                Quota::TRUSTED
            }
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

        // Draining finishes what is in flight and starts nothing new: a
        // publish, a fresh subscription or a fresh negotiation waits for the
        // next encounter, while replies to work already underway pass.
        if self.state == State::Draining {
            match &message {
                Message::Publish(_) | Message::Req(..) | Message::NegOpen(..) => return Ok(()),
                _ => {}
            }
        }

        let quota = self.quota();

        // Blobs answer and drive their own channel: a request is served from
        // the store, a response advances whatever fetch it belongs to.
        match &message {
            Message::BlossomRequest(request) => {
                let reply = crate::sync::blob::handle_request(db, &*self.blobs, &peer, request)?;

                self.send_sync(&Message::BlossomResponse(Box::new(reply)))?;
                return Ok(());
            }
            Message::BlossomResponse(response) => {
                self.on_blossom_response(db, response.as_ref())?;
                self.maybe_fetch_blob(db)?;
                return Ok(());
            }
            _ => {}
        }

        if self.try_relay(db, &peer, &message, quota)? {
            return Ok(());
        }

        let replies = crate::sync::client::handle(
            db,
            &peer,
            message,
            quota,
            &mut self.used,
            &mut self.client,
        )?;

        for reply in replies {
            self.send_sync(&reply)?;
        }

        Ok(())
    }

    /// Begin a reconciliation against the peer, sending the opening frame.
    ///
    /// The subscription is this device's own, which is how a later reply is
    /// told apart from one this device must answer: a `NEG-MSG` naming this
    /// map goes to the client half, anything else to the relay half.
    pub fn begin_negotiation(&mut self, db: &Db, filter: Filter) -> Result<()> {
        if self.state == State::Draining {
            return Ok(());
        }

        let Some(peer) = self.peer.as_ref().cloned() else {
            bail!("a negotiation needs the peer to be identified");
        };

        let (negotiation, opening) = crate::sync::client::Negotiation::begin(db, &peer, filter)?;
        let subscription = negotiation.subscription_id().clone();

        self.client.negotiations.insert(subscription, negotiation);
        self.send_sync(&opening)?;

        Ok(())
    }

    /// Once both identities are bound, store the pair secret and the
    /// disclosure, and open the reconciliation.
    ///
    /// The accept policy gates ingest and the peer's relay half bounds what it
    /// will answer, so a bare filter is the right opening move.
    pub fn enter_syncing(&mut self, db: &Db) -> Result<()> {
        if self.state != State::Identified {
            return Ok(());
        }

        self.state = State::Syncing;

        if self.sync_started {
            return Ok(());
        }
        self.sync_started = true;

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

        let secret = recognition::derive_secret(&self.noise.handshake_hash()?);
        let pubkeys: Vec<PublicKey> = peer.pubkeys().copied().collect();

        crate::db::command::pair_with(db, &pubkeys, &secret, clock::now())
    }

    /// Start the next blob fetch if none is in flight, probing it with a
    /// `HEAD` before any bytes move.
    fn maybe_fetch_blob(&mut self, db: &Db) -> Result<()> {
        if self.state != State::Syncing || self.blob_fetch.is_some() {
            return Ok(());
        }

        // Low battery skips the transfer; it is the one thing a fetch cannot
        // be interrupted for on a phone.
        if self
            .battery
            .is_some_and(|level| level < crate::sync::blob::BLOB_MIN_BATTERY)
        {
            return Ok(());
        }

        let Some(wanted) = crate::db::query::wanted_blobs(db, 1)?.into_iter().next() else {
            return Ok(());
        };

        // Partial bytes from a dead transfer cannot be trusted without
        // per-chunk verification, so a re-fetch starts clean.
        if self.blobs.has(&wanted.sha256)? {
            self.blobs.delete(&wanted.sha256)?;
        }

        let request_id = format!("blob-{}", &wanted.sha256[..16]);
        let mut fetch = BlobFetch::begin(wanted.clone(), request_id);

        self.send_blob_request(&fetch.blob, "HEAD", &fetch.request_id, &[])?;
        fetch.probing = true;
        self.blob_fetch = Some(fetch);

        Ok(())
    }

    /// Advance the in-flight fetch on one `BLOSSOM-RES`.
    fn on_blossom_response(
        &mut self,
        db: &Db,
        response: &crate::sync::message::BlossomResponse,
    ) -> Result<()> {
        let mut fetch = match &self.blob_fetch {
            Some(fetch) if fetch.request_id == response.id => fetch.clone(),
            _ => return Ok(()),
        };

        if fetch.probing {
            // The HEAD answers: either the length, or that the peer lacks it.
            if response.status == 404 {
                self.blob_fetch = None;
                return Ok(());
            }

            let total = response
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.parse::<u64>().ok());

            let Some(total) = total else {
                self.blob_fetch = None;
                return Ok(());
            };

            fetch.total = Some(total);
            fetch.probing = false;

            if fetch.stored >= total {
                self.finish_blob(db, &fetch)?;
                return Ok(());
            }

            self.ask_blob_group(&mut fetch)?;
        } else if response.status == 206 {
            // One group of verified bytes arrived.
            self.blobs.append(&fetch.blob.sha256, &response.body)?;
            fetch.stored += response.body.len() as u64;

            crate::db::command::record_blob_progress(
                db,
                &fetch.blob.sha256,
                fetch.stored as i64,
                None,
            )?;

            if fetch.total.is_some_and(|total| fetch.stored >= total) {
                self.finish_blob(db, &fetch)?;
                return Ok(());
            } else {
                self.ask_blob_group(&mut fetch)?;
            }
        } else {
            // Anything else ends this fetch.
            self.blob_fetch = None;
            return Ok(());
        }

        self.blob_fetch = Some(fetch);

        Ok(())
    }

    /// Mark a fetch whole after verifying the assembled bytes.
    fn finish_blob(&mut self, db: &Db, fetch: &BlobFetch) -> Result<()> {
        if crate::sync::blob::verifies(&fetch.blob, &*self.blobs)? {
            crate::db::command::complete_blob(
                db,
                &fetch.blob.sha256,
                fetch.stored as i64,
                clock::now(),
            )?;
        }

        self.blob_fetch = None;

        Ok(())
    }

    /// Ask for the next group of a probe-completed fetch. Marking the link for a
    /// bulk channel: once bytes move, the setup round trip is worth it.
    fn ask_blob_group(&mut self, fetch: &mut BlobFetch) -> Result<()> {
        let start = fetch.stored;
        let end = start + BLOB_GROUP_BYTES - 1;
        let id = fetch.request_id.clone();

        self.l2cap_requested = true;
        self.send_blob_request(
            &fetch.blob,
            "GET",
            &id,
            &[("range".to_string(), format!("bytes={start}-{end}"))],
        )
    }

    /// Whether the shell should be told to open L2CAP for this link, once.
    pub fn take_l2cap_request(&mut self) -> bool {
        std::mem::take(&mut self.l2cap_requested)
    }

    /// The battery level in percent, which gates blob transfers.
    pub fn set_battery(&mut self, level: Option<u8>) {
        self.battery = level;
    }

    /// Queue one Blossom request on the sync channel.
    fn send_blob_request(
        &mut self,
        blob: &Blob,
        method: &str,
        id: &str,
        headers: &[(String, String)],
    ) -> Result<()> {
        let request = crate::sync::message::BlossomRequest {
            id: id.to_string(),
            method: method.to_string(),
            path: format!("/{}", blob.sha256),
            headers: headers.to_vec(),
            body: Vec::new(),
        };

        self.send_sync(&Message::BlossomRequest(Box::new(request)))
    }

    /// Offer a just-stored event to every subscription the peer has open, if the
    /// event would have been served on a fresh `REQ`.
    ///
    /// Driven by the store's event channel, so anything that stores an own event —
    /// a publish, or an own event coming home through an ingest — reaches every
    /// connected peer without the writer knowing.
    pub fn offer_event(&mut self, event: &HashedEvent) -> Result<()> {
        if self.state == State::Draining {
            return Ok(());
        }

        let Some(peer) = self.peer.as_ref().cloned() else {
            return Ok(());
        };

        let subscriptions: Vec<SubscriptionId> = self.subscriptions.keys().cloned().collect();

        for subscription in subscriptions {
            let Some(filters) = self.subscriptions.get(&subscription).cloned() else {
                continue;
            };

            // The same two tests the relay half applies: any of the sub's
            // filters, and what the peer may be served. Own events are always
            // in the Own register, so the registers need no check here.
            if filters.iter().any(|filter| filter.matches(event)) && peer.may_be_served(event) {
                self.send_sync(&Message::Event(
                    subscription.clone(),
                    Box::new(event.clone()),
                ))?;
                self.attach_own_signature(&peer, &subscription, event)?;
            }
        }

        Ok(())
    }

    /// Attach this device's signature over its own event, naming the peer, so
    /// the peer can forward it one more hop.
    fn attach_own_signature(
        &mut self,
        peer: &Peer,
        subscription: &SubscriptionId,
        event: &HashedEvent,
    ) -> Result<()> {
        let Some(peer_key) = peer.pubkeys().next().copied() else {
            return Ok(());
        };

        let signature = crate::model::RecipientSignature::sign(&self.identity, event.id, peer_key);

        self.send_sync(&Message::RecipientSignature(
            subscription.clone(),
            event.id,
            Box::new(signature.sig),
        ))
    }

    /// Try the relay half. Returns `true` when the message was consumed.
    fn try_relay(&mut self, db: &Db, peer: &Peer, message: &Message, quota: Quota) -> Result<bool> {
        match message {
            Message::Req(..) | Message::Close(..) | Message::NegOpen(..) | Message::Publish(..) => {
                let replies = relay::handle(
                    db,
                    peer,
                    &self.identity,
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
            // A NEG-MSG names the negotiation it belongs to: one this device
            // opened goes to the client half, whatever else it looks like.
            // One the peer opened is registered on our relay half.
            Message::NegMsg(subscription, _)
                if self.client.negotiations.contains_key(subscription) =>
            {
                Ok(false)
            }
            Message::NegMsg(..) => {
                let replies = relay::handle(
                    db,
                    peer,
                    &self.identity,
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
            None => {
                // A gate hold has the same cap as a drain: the doc holds an
                // unanswered prompt for up to five minutes.
                if let Some(since) = self.pending_since {
                    return now - since >= DRAIN_CAP_SECONDS;
                }

                now - self.last_heard >= HEARTBEAT_TIMEOUT_SECONDS
            }
        }
    }

    /// When this session next needs attention, for the shell to arm a timer on.
    #[must_use]
    pub fn deadline(&self) -> i64 {
        match self.draining_since {
            Some(since) => since + DRAIN_CAP_SECONDS,
            None => {
                if let Some(since) = self.pending_since {
                    return since + DRAIN_CAP_SECONDS;
                }

                if self.beats() {
                    self.next_heartbeat_at
                        .min(self.last_heard + HEARTBEAT_TIMEOUT_SECONDS)
                } else {
                    self.last_heard + HEARTBEAT_TIMEOUT_SECONDS
                }
            }
        }
    }

    /// Send a heartbeat if one is due and the link has been quiet.
    ///
    /// A beat proves liveness to the peer; anything already queued or in
    /// flight proves it too, so a busy link skips the beat rather than piling
    /// one behind a transfer. Only post-handshake states beat.
    pub fn maybe_heartbeat(&mut self) -> Result<()> {
        if !self.beats() {
            return Ok(());
        }

        let now = clock::now();

        if now < self.next_heartbeat_at {
            return Ok(());
        }

        // Traffic on its way out is the heartbeat; a beat would only wait
        // behind it. Reset the timer either way, so a long transfer does not
        // emit a beat the moment it drains.
        if !self.outbox.is_idle() {
            self.next_heartbeat_at = now + jittered_heartbeat_interval();
            return Ok(());
        }

        self.send_control(control::HEARTBEAT, &[])?;
        self.next_heartbeat_at = now + jittered_heartbeat_interval();

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
                | State::GatePending
        )
    }

    // ========================================================================
    // Handshake
    // ========================================================================

    /// Process a handshake message and advance to [`Secured`] when it
    /// completes.
    fn advance_handshake(&mut self, db: &Db, frame: &Frame) -> Result<()> {
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
                self.send_recognition_tags(db)?;
                self.send_auth_challenge()?;
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
            control::AUTH_EVENT => self.on_auth_response(db, payload)?,
            control::HEARTBEAT => {
                // The write itself refreshed `last_heard` in `receive`; there
                // is nothing else a liveness beacon carries.
            }
            other => bail!("an unknown control frame {other} arrived"),
        }

        // Both identities are bound once the peer is verified and this device
        // has disclosed its own, in whichever order the two happened.
        if self.peer.is_some() && self.disclosed {
            self.enter_syncing(db)?;
        }

        Ok(())
    }

    /// Resolve the peer's recognition tags and run the consent gate on the
    /// result, before anyone has disclosed a pubkey.
    fn on_tags(&mut self, db: &Db, payload: &[u8]) -> Result<()> {
        let resolved = self.resolve_recognition(db, payload)?;

        match self.evaluate_gate(db, resolved)? {
            Gate::Pass => self.gate_passed = true,
            Gate::Blocked => self.state = State::Closed,
            Gate::Pending => {
                self.state = State::GatePending;
                self.pending_since = Some(clock::now());
            }
        }

        Ok(())
    }

    /// Take the peer's challenge. The receiver challenges back without
    /// answering; the dialer answers, identifying first.
    fn on_challenge(&mut self, db: &Db, payload: &[u8]) -> Result<()> {
        let challenge =
            String::from_utf8(payload.to_vec()).context("an AUTH challenge is not UTF-8")?;

        self.peer_challenge = Some(challenge.clone());

        // A held gate defers the turn; approval resumes it.
        if self.state == State::GatePending {
            return Ok(());
        }

        if !self.gate_passed {
            return Ok(());
        }

        match self.role {
            // The receiver names no pubkey yet: it sends its own tags and
            // challenge, and answers only after it has seen the dialer.
            Role::Receiver => {
                self.send_recognition_tags(db)?;
                self.send_auth_challenge()?;
            }
            // The dialer identifies itself first.
            Role::Dialer => {
                self.send_auth_response(&challenge)?;
                self.disclosed = true;
                self.state = State::DialerIdentified;
            }
        }

        Ok(())
    }

    /// Verify the peer's AUTH response. The receiver discloses here, once it
    /// has seen the dialer.
    fn on_auth_response(&mut self, _db: &Db, payload: &[u8]) -> Result<()> {
        let event = serde_json::from_slice::<coracle_lib::events::Event>(payload)
            .context("parsing an AUTH response")?;

        self.verify_auth_response(event)?;

        if self.state == State::Closed {
            return Ok(());
        }

        // The receiver now discloses, having evaluated the dialer's identity.
        if self.role == Role::Receiver && !self.disclosed {
            if let Some(challenge) = self.peer_challenge.clone() {
                self.send_auth_response(&challenge)?;
                self.disclosed = true;
            }
        }

        Ok(())
    }

    /// What the consent gate does with a resolved peer.
    fn evaluate_gate(&self, db: &Db, resolved: Option<PublicKey>) -> Result<Gate> {
        match resolved {
            // Paired peers pass silently; a blocked one keeps its tags so the
            // connection can be dropped here, before either side names itself.
            Some(pubkey) => Ok(
                if self.policy.graph.standing(&pubkey) == Standing::Blocked {
                    Gate::Blocked
                } else {
                    Gate::Pass
                },
            ),
            // A stranger is admitted by discoverability and the budget, or it
            // waits on the user.
            None => Ok(if self.admitted(db)? {
                Gate::Pass
            } else {
                Gate::Pending
            }),
        }
    }

    /// Whether an unrecognized peer may proceed without a prompt: the cool-off
    /// window or a discoverable time admits it, and the disclosure budget has
    /// headroom.
    fn admitted(&self, db: &Db) -> Result<bool> {
        let now = clock::now();
        let discoverable = self.policy.is_discoverable_at(clock::minute_of_day());
        let cool_off = self
            .cool_off_since
            .is_some_and(|since| now - since < self.policy.cool_off_minutes * 60);

        if !(discoverable || cool_off) {
            return Ok(false);
        }

        let spent = crate::db::query::disclosures_since(db, now - DISCLOSURE_WINDOW_SECONDS)?;

        Ok(spent < self.policy.disclosure_budget)
    }

    // ========================================================================
    // Recognition
    // ========================================================================

    /// Send this device's recognition tags, padded to the constant count so the
    /// length does not disclose how many peers this device has paired with.
    fn send_recognition_tags(&mut self, db: &Db) -> Result<()> {
        let hash = self.noise.handshake_hash()?;
        let secrets = crate::db::query::pair_secrets(db)?;

        let mut tags: Vec<recognition::Tag> = secrets
            .iter()
            .map(|(_, secret)| recognition::tag(secret, &hash))
            .collect();

        while tags.len() < recognition::TAG_COUNT {
            let mut tag = [0u8; 32];
            let _ = getrandom::getrandom(&mut tag);
            tags.push(tag);
        }

        let payload = serde_json::to_vec(&tags).context("serializing recognition tags")?;

        self.send_control(control::TAGS, &payload)
    }

    /// Trial-MAC the offered tags against every pair secret this device holds,
    /// returning the pubkey one of them resolves to.
    fn resolve_recognition(&self, db: &Db, payload: &[u8]) -> Result<Option<PublicKey>> {
        let offered: recognition::Tags =
            serde_json::from_slice(payload).context("decoding recognition tags")?;
        let hash = self.noise.handshake_hash()?;
        let secrets = crate::db::query::pair_secrets(db)?;

        Ok(recognition::resolve(&offered, &secrets, &hash))
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
        self.send_control(control::CHALLENGE, challenge.as_bytes())
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

        self.send_control(control::AUTH_EVENT, &payload)
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

/// A fresh heartbeat interval, jittered between the documented bounds.
fn jittered_heartbeat_interval() -> i64 {
    let mut byte = [0u8; 1];
    let _ = getrandom::getrandom(&mut byte);

    HEARTBEAT_MIN_SECONDS
        + i64::from(byte[0] % (HEARTBEAT_MAX_SECONDS - HEARTBEAT_MIN_SECONDS + 1) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::query as db_query;
    use crate::fixtures::{author, note, secret};
    use crate::model::Query;

    fn policy() -> Policy {
        Policy::new(author(1))
    }

    fn identity() -> SecretKey {
        secret(1)
    }

    fn blobs() -> Arc<crate::blobstore::MemoryBlobStore> {
        Arc::new(crate::blobstore::MemoryBlobStore::default())
    }

    fn session(policy: Policy) -> Session {
        Session::open(
            LinkId(1),
            Role::Dialer,
            64,
            Arc::new(policy),
            identity(),
            blobs(),
        )
        .unwrap()
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
    fn a_trusted_identity_earns_the_trusted_budget_whichever_key_it_proved() {
        let mut policy = policy();
        policy.graph.trusted.insert(author(9));
        let mut session = session(policy);

        // The trusted pubkey sorts after the stranger one, which is what the
        // old "first policy" reading would have picked instead.
        session.identify([author(2), author(9)]);

        assert_eq!(session.quota(), Quota::TRUSTED);
    }

    #[test]
    fn a_quiet_session_beats_within_the_jittered_bounds() {
        let mut session = clock::at(1_000, || session(policy()));
        session.identify([author(2)]);

        // Before the minimum interval nothing is due.
        clock::at(1_000 + HEARTBEAT_MIN_SECONDS - 1, || {
            session.maybe_heartbeat().unwrap();
            assert!(session.next_write().is_none());
        });

        // Past the maximum it is, and the beat carries the discriminant alone.
        clock::at(1_000 + HEARTBEAT_MAX_SECONDS + 1, || {
            session.maybe_heartbeat().unwrap();
        });
        let fragment = session.next_write().unwrap();
        assert_eq!(
            &fragment[..],
            &[Channel::Control as u8, 0, control::HEARTBEAT]
        );
    }

    #[test]
    fn a_busy_link_skips_the_beat_and_treats_traffic_as_liveness() {
        let mut session = clock::at(1_000, || session(policy()));
        session.identify([author(2)]);

        // Queued traffic stands in for the beat.
        session
            .send(&crate::transport::Frame {
                channel: crate::transport::Channel::Sync,
                payload: vec![0; 8],
            })
            .unwrap();

        clock::at(1_000 + HEARTBEAT_MAX_SECONDS + 1, || {
            session.maybe_heartbeat().unwrap();
        });

        // The first fragment out is the queued traffic, not a beat.
        let fragment = session.next_write().unwrap();
        assert_eq!(fragment[0], Channel::Sync as u8);
    }

    #[test]
    fn a_heartbeat_is_absorbed_without_moving_the_session() {
        let db = Db::open_in_memory().unwrap();
        let mut session = session(policy());
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
        let mut session = session(policy());
        session.identify([author(2)]);

        let frame = Frame {
            channel: crate::transport::Channel::Control,
            payload: vec![0xEE],
        };

        assert!(session.advance(&db, &frame).is_err());
    }

    #[test]
    fn a_wanted_blob_is_driven_through_head_and_get() {
        use crate::blobstore::MemoryBlobStore;
        use crate::db::command as db_command;
        use crate::sync::message::BlossomResponse;
        use sha2::{Digest as _, Sha256};

        let db = Db::open_in_memory().unwrap();
        let store = Arc::new(MemoryBlobStore::default());
        let mut session = Session::open(
            LinkId(1),
            Role::Dialer,
            4096,
            Arc::new(policy()),
            secret(1),
            store.clone(),
        )
        .unwrap();
        session.state = State::Syncing;

        // A stored event references a blob this device does not hold.
        let bytes = b"the quick brown fox";
        let hash = hex::encode(Sha256::digest(bytes));
        let event = note(
            author(1),
            1,
            "with a blob",
            NostrTags::new().add(
                "imeta",
                [format!("x {hash}"), format!("size {}", bytes.len())],
            ),
        );
        db_command::publish_event(&db, &event, &author(1), 1).unwrap();

        // The fetch begins with a HEAD.
        session.maybe_fetch_blob(&db).unwrap();
        let head = session.next_write().unwrap();
        session.acknowledge_write();
        let Message::BlossomRequest(head_req) = Message::decode(&head[2..]).unwrap() else {
            panic!("expected a HEAD request");
        };
        assert_eq!(head_req.method, "HEAD");

        // The HEAD answers with the length, and a GET follows.
        session
            .on_blossom_response(
                &db,
                &BlossomResponse {
                    id: head_req.id.clone(),
                    status: 200,
                    headers: vec![("content-length".into(), "19".into())],
                    body: vec![],
                },
            )
            .unwrap();

        let get = session.next_write().unwrap();
        session.acknowledge_write();
        let Message::BlossomRequest(get_req) = Message::decode(&get[2..]).unwrap() else {
            panic!("expected a GET request");
        };
        assert_eq!(get_req.method, "GET");

        // The GET answers with the bytes, the whole blob now held and complete.
        session
            .on_blossom_response(
                &db,
                &BlossomResponse {
                    id: get_req.id.clone(),
                    status: 206,
                    headers: vec![],
                    body: bytes.to_vec(),
                },
            )
            .unwrap();

        assert_eq!(store.len(&hash).unwrap(), Some(19));
        assert!(
            crate::db::query::get_blob(&db, &hash)
                .unwrap()
                .unwrap()
                .complete
        );
        assert!(session.blob_fetch.is_none());
    }

    #[test]
    fn the_battery_floor_gates_blob_fetches() {
        use crate::db::command as db_command;
        use sha2::{Digest as _, Sha256};

        let db = Db::open_in_memory().unwrap();
        let mut session = session(policy());
        session.state = State::Syncing;

        let bytes = b"below the floor";
        let hash = hex::encode(Sha256::digest(bytes));
        let event = note(
            author(1),
            1,
            "with a blob",
            NostrTags::new().add("imeta", [format!("x {hash}")]),
        );
        db_command::publish_event(&db, &event, &author(1), 1).unwrap();

        session.set_battery(Some(crate::sync::blob::BLOB_MIN_BATTERY - 1));
        session.maybe_fetch_blob(&db).unwrap();
        assert!(
            session.next_write().is_none(),
            "a low battery fetches nothing"
        );

        session.set_battery(Some(crate::sync::blob::BLOB_MIN_BATTERY));
        session.maybe_fetch_blob(&db).unwrap();
        assert!(session.next_write().is_some());
    }

    #[test]
    fn a_draining_session_takes_no_new_work() {
        let db = Db::open_in_memory().unwrap();
        let mut session = session(policy());

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
        assert!(session.next_write().is_none());
        assert!(
            db_query::list_events(&db, &Query::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_negotiation_reply_goes_to_the_client_half() {
        let db = Db::open_in_memory().unwrap();
        let mut session = session(policy());

        session.identify([author(2)]);

        // This device opens a negotiation, so the subscription is this side's;
        // the session's own bookkeeping is what routes the reply.
        session
            .begin_negotiation(&db, coracle_lib::filters::Filter::new())
            .unwrap();
        let opening = session.next_write().unwrap();
        session.acknowledge_write();
        let Message::NegOpen(subscription, _, frame) = Message::decode(&opening[2..]).unwrap()
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
        assert!(session.next_write().is_some());
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
        )
        .unwrap();
        let mut receiver = Session::open(
            LinkId(2),
            Role::Receiver,
            4096,
            Arc::new(policy()),
            secret(2),
            blobs(),
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
        )
        .unwrap()
    }

    #[test]
    fn a_pair_reaches_syncing_through_mutual_auth() {
        let db = Db::open_in_memory().unwrap();
        let mut dialer = Session::open(
            LinkId(1),
            Role::Dialer,
            4096,
            Arc::new(policy()),
            secret(1),
            blobs(),
        )
        .unwrap();
        let mut receiver = Session::open(
            LinkId(2),
            Role::Receiver,
            4096,
            Arc::new(policy()),
            secret(2),
            blobs(),
        )
        .unwrap();

        // The cool-off admits the strangers, or the gate holds the pair.
        let now = clock::now();
        dialer.set_cool_off_since(Some(now));
        receiver.set_cool_off_since(Some(now));

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
        let opening = dialer.next_write().unwrap();
        assert_eq!(opening[0], Channel::Sync as u8);
    }

    #[test]
    fn pairing_stores_the_same_secret_on_both_sides() {
        let db = Db::open_in_memory().unwrap();
        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);

        let now = clock::now();
        dialer.set_cool_off_since(Some(now));
        receiver.set_cool_off_since(Some(now));

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
        let now = clock::now();
        dialer.set_cool_off_since(Some(now));
        receiver.set_cool_off_since(Some(now));
        full_exchange(&mut dialer, &mut receiver, &db);

        // Second encounter: no cool-off, but the tags resolve, so the gate
        // passes silently and the pair reaches Syncing anyway.
        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);
        full_exchange(&mut dialer, &mut receiver, &db);
    }

    #[test]
    fn the_gate_blocks_a_resolved_peer() {
        let db = Db::open_in_memory().unwrap();
        let mut policy = policy();
        policy.graph.blocked.insert(author(2));
        let session = session(policy);

        assert!(matches!(
            session.evaluate_gate(&db, Some(author(2))).unwrap(),
            Gate::Blocked
        ));
    }

    #[test]
    fn the_gate_passes_a_paired_peer_whatever_the_discoverability() {
        let db = Db::open_in_memory().unwrap();
        let session = session(policy());

        assert!(matches!(
            session.evaluate_gate(&db, Some(author(2))).unwrap(),
            Gate::Pass
        ));
    }

    #[test]
    fn the_gate_admits_a_stranger_during_the_cool_off() {
        let db = Db::open_in_memory().unwrap();
        let mut session = session(policy());
        session.set_cool_off_since(Some(clock::now()));

        assert!(matches!(
            session.evaluate_gate(&db, None).unwrap(),
            Gate::Pass
        ));
    }

    #[test]
    fn the_gate_holds_a_stranger_without_admission() {
        let db = Db::open_in_memory().unwrap();
        let session = session(policy());

        assert!(matches!(
            session.evaluate_gate(&db, None).unwrap(),
            Gate::Pending
        ));
    }

    #[test]
    fn the_gate_honors_the_disclosure_budget() {
        let db = Db::open_in_memory().unwrap();
        let mut policy = policy();
        policy.disclosure_budget = 1;
        let mut session = session(policy);
        session.set_cool_off_since(Some(clock::now()));

        // One stranger already disclosed to spends the budget of one.
        crate::db::command::pair_with(&db, &[author(9)], &[0u8; 32], clock::now()).unwrap();

        assert!(matches!(
            session.evaluate_gate(&db, None).unwrap(),
            Gate::Pending
        ));
    }

    #[test]
    fn approval_is_requested_once_and_refusal_closes() {
        let db = Db::open_in_memory().unwrap();
        let mut session = session(policy());
        session.state = State::GatePending;

        assert!(session.request_approval());
        assert!(!session.request_approval(), "the prompt is one-shot");

        session.approve(&db, false).unwrap();
        assert_eq!(session.state(), State::Closed);
    }

    #[test]
    fn a_stranger_without_admission_is_held_and_can_be_approved() {
        let db = Db::open_in_memory().unwrap();
        let mut dialer = pair(4096, Role::Dialer, 1);
        let mut receiver = pair(4096, Role::Receiver, 2);

        // Only the dialer's cool-off runs, so it admits the receiver while the
        // receiver holds the dialer for the user.
        dialer.set_cool_off_since(Some(clock::now()));

        dialer.initiate().unwrap();
        pump(&mut dialer, &mut receiver, &db); // msg1
        pump(&mut receiver, &mut dialer, &db); // reply
        pump(&mut dialer, &mut receiver, &db); // msg3
        pump(&mut dialer, &mut receiver, &db); // dialer tags: the gate holds
        pump(&mut dialer, &mut receiver, &db); // dialer challenge: held too

        assert_eq!(receiver.state(), State::GatePending);
        assert!(!receiver.disclosed, "nothing disclosed before approval");

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
