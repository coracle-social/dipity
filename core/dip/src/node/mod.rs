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
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, bail};
use coracle_lib::events::HashedEvent;
use coracle_lib::keys::{PublicKey, SecretKey};
use tokio::sync::broadcast::{self, error::TryRecvError};

use crate::backup;
use crate::blobs::{BlobStore, FileBlobStore, verified};
use crate::clock;
use crate::db::blob::channel as blob_channel;
use crate::db::blob::channel::BlobChange;
use crate::db::command as db_command;
use crate::db::event::channel::{self, EventChange};
use crate::db::pref::channel::{self as pref_channel, PrefChange};
use crate::db::{Db, query};
use crate::keys::KeyCustody;
use crate::link::{LinkId, PeripheralId, Role};
use crate::model::{BLOCK, Blob, BlobHash, MUTE, Policy, TRUST};
use crate::session::gate::Presence;
use crate::session::l2cap::Step;
use crate::session::transfer::Outcome;
use crate::session::{Ending, Session, State};
use crate::transport::{Channel, Pipe};
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
    ///
    /// The value is what the two users compare, and the gate authenticates
    /// nobody without it. `docs/discovery.md#the-consent-gate`.
    RequestApproval(LinkId, u32),
    /// Who the peer on a link proved to be, once per pubkey it proves.
    ///
    /// The gate runs before either side names a pubkey, so a pet name the user
    /// typed there is for a person rather than for a key. This is what says
    /// which key that person turned out to hold.
    PeerIdentified(LinkId, PublicKey),
    /// Present the share sheet over a key backup the core has written.
    ///
    /// The path is the shell's, not the view's: the view starts the export and
    /// learns only that it was shared or dismissed, never where the file is.
    /// `docs/keys.md#backup`.
    ShareKeyBackup(PathBuf),
    /// Write one bulk fragment to this link's L2CAP channel, length-prefixed
    /// because the channel is a byte stream.
    ///
    /// Answered with [`Node::bulk_write_complete`], which releases the next —
    /// separately from GATT, so a slow ATT write does not stall the transfer.
    SendBulk(LinkId, Vec<u8>),
    /// Publish an L2CAP channel and report its PSM with
    /// [`Node::l2cap_published`], or [`Node::l2cap_unavailable`] if the
    /// platform will not.
    ///
    /// Only the GATT peripheral publishes. Raised once outstanding blob bytes
    /// justify the setup round trip.
    /// `docs/transport.md#the-l2cap-bandwidth-upgrade`.
    PublishL2cap(LinkId),
    /// Open the L2CAP channel the peer published at this PSM, and report the
    /// result with [`Node::l2cap_opened`] or [`Node::l2cap_unavailable`].
    OpenL2cap(LinkId, u16),
    /// Show the user this six-digit comparison value and ask whether the other
    /// device shows the same one.
    ///
    /// Both ends are asked it and either may answer first. Answer with
    /// [`Node::answer_identity_transfer`]; nothing moves until both have said
    /// yes. `docs/keys.md#login-with-device`.
    ConfirmIdentityTransfer(LinkId, u32),
    /// How an identity transfer ended. On [`Outcome::Received`] take the key
    /// with [`Node::take_transferred_identity`], write it to secure storage,
    /// and reopen the node under it.
    IdentityTransfer(LinkId, Outcome),
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
    /// Where the nostr identity is read from, one use at a time
    custody: Arc<dyn KeyCustody>,
    /// The pubkey that identity names, derived once at open
    identity: PublicKey,
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
    /// This store's preference channel, so a written preference rebinds every
    /// live session without the writer knowing a node exists.
    pref_changes: broadcast::Receiver<PrefChange>,
    /// When the retention sweep last ran, so ticking often sweeps rarely.
    events_swept_at: Option<i64>,
    /// The backup file waiting on a share sheet, so the core can delete it
    /// whichever way the sheet ends.
    key_backup: Option<PathBuf>,
    /// Whether the shell has been told to scan and advertise, which the first
    /// tick does once. `docs/discovery.md#session-lifecycle`.
    radio_started: bool,
    /// Keys an identity transfer delivered, kept here rather than on the
    /// session so a link dropping before the shell takes one does not lose it.
    transferred: BTreeMap<LinkId, SecretKey>,
}

impl Node {
    /// Build a node over a store the shell has opened and the Keychain or
    /// Keystore the identity key is read out of, with the blob store of the
    /// shell's choosing.
    pub fn new(
        db: Arc<Db>,
        custody: Arc<dyn KeyCustody>,
        blobs: Arc<dyn BlobStore>,
    ) -> Result<Self> {
        let identity = custody.identity()?.public_key();
        let policy = Arc::new(query::policy(&db, &identity)?);

        let mut node = Self {
            events: channel::subscribe(&db),
            blob_changes: blob_channel::subscribe(&db),
            pref_changes: pref_channel::subscribe(&db),
            db,
            custody,
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
            key_backup: None,
            radio_started: false,
            transferred: BTreeMap::new(),
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
    pub fn open(
        db: Arc<Db>,
        custody: Arc<dyn KeyCustody>,
        directory: impl AsRef<Path>,
    ) -> Result<Self> {
        let blobs = Arc::new(FileBlobStore::open(directory.as_ref().join("blobs"))?);

        Self::new(db, custody, blobs)
    }

    /// This device's pubkey, which is all of the identity the node keeps.
    #[must_use]
    pub fn identity(&self) -> PublicKey {
        self.identity
    }

    /// The policy every live session is bound to, as
    /// [`policy_changed`](Self::policy_changed) last compiled it, and the one
    /// a screen draws.
    #[must_use]
    pub fn policy(&self) -> &Policy {
        &self.policy
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
            Arc::clone(&self.custody),
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
        let received = self.receive_frame(link, write);

        self.after_receiving(link, received)
    }

    /// A slice arrived off this link's L2CAP channel.
    ///
    /// The channel is a byte stream, so a read may hold several fragments, part
    /// of one, or both; the wire keeps what does not complete one.
    pub fn bulk_received(&mut self, link: LinkId, read: &[u8]) -> Vec<Action> {
        let received = self.receive_bulk(link, read);

        self.after_receiving(link, received)
    }

    /// End the link if reading it failed, and collect either way.
    fn after_receiving(&mut self, link: LinkId, received: Result<()>) -> Vec<Action> {
        let Err(error) = received else {
            return self.collect();
        };

        log::debug!("dropping link {link:?}: {error:#}");

        // Close rather than only telling the shell: a session left in the table keeps its slot.
        if let Some(session) = self.sessions.get_mut(&link) {
            session.close(Ending::Refused);
        }

        self.collect()
    }

    /// Reassemble, decrypt, and dispatch every frame a bulk read completed.
    fn receive_bulk(&mut self, link: LinkId, read: &[u8]) -> Result<()> {
        match self.sessions.get_mut(&link) {
            Some(session) => session.receive_bulk(&self.db, read),
            None => Ok(()),
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
            Channel::Blob => session.handle_blob(&self.db, &frame)?,
        }

        Ok(())
    }

    /// The write the shell was handed has been acknowledged, which releases the
    /// next fragment.
    pub fn write_complete(&mut self, link: LinkId) -> Vec<Action> {
        self.acknowledge(link, Pipe::Gatt)
    }

    /// The bulk write the shell was handed has been acknowledged, which
    /// releases the next one on the L2CAP channel alone.
    pub fn bulk_write_complete(&mut self, link: LinkId) -> Vec<Action> {
        self.acknowledge(link, Pipe::Bulk)
    }

    /// The shell published an L2CAP channel; the PSM goes to the peer.
    pub fn l2cap_published(&mut self, link: LinkId, psm: u16) -> Result<Vec<Action>> {
        self.on_session(link, |session| session.l2cap_published(psm))
    }

    /// The L2CAP channel is up, with the MTU it negotiated. Bulk moves onto it.
    pub fn l2cap_opened(&mut self, link: LinkId, mtu: usize) -> Result<Vec<Action>> {
        self.on_session(link, |session| session.l2cap_opened(mtu))
    }

    /// The upgrade will not happen, or the channel that had it went away.
    ///
    /// Both are the same answer: this link finishes on GATT. It is not an
    /// error — L2CAP is bandwidth, and every device without it still syncs.
    pub fn l2cap_unavailable(&mut self, link: LinkId) -> Result<Vec<Action>> {
        self.on_session(link, Session::l2cap_unavailable)
    }

    /// Carry a shell answer to the session it belongs to, and collect.
    ///
    /// The link may already be down — a channel the shell was opening comes
    /// back after a disconnect as readily as before one — and an answer with no
    /// session left to hear it is dropped rather than being an error.
    fn on_session(
        &mut self,
        link: LinkId,
        answer: impl FnOnce(&mut Session) -> Result<()>,
    ) -> Result<Vec<Action>> {
        if let Some(session) = self.sessions.get_mut(&link) {
            answer(session)?;
        }

        Ok(self.collect())
    }

    /// Release the next write on one pipe.
    fn acknowledge(&mut self, link: LinkId, pipe: Pipe) -> Vec<Action> {
        if let Some(session) = self.sessions.get_mut(&link) {
            session.acknowledge_write(pipe);
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

        let mut actions = Vec::new();

        if !self.radio_started {
            self.radio_started = true;
            actions.extend([Action::Scan(true), Action::Advertise(true)]);
        }

        // A queued candidate may now be past its rate limit, its backoff, or the link cap.
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
    ///
    /// Leaving the foreground ends any identity transfer, which is the one
    /// flow that does not run with nobody in front of the screen.
    fn set_presence(&mut self, presence: Presence) -> Vec<Action> {
        self.presence = Some(presence);

        for session in self.sessions.values_mut() {
            session.gate.presence = self.presence;

            if presence != Presence::Foreground
                && let Err(error) = session.cancel_transfer()
            {
                log::error!(
                    "ending an identity transfer on link {:?} failed: {error:#}",
                    session.link
                );
            }
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

    /// Offer this device's identity to the peer on `link`, which is the user
    /// tapping "log in another device".
    ///
    /// Both users are then asked to compare a six-digit number, and the key
    /// moves once both have said yes. `docs/keys.md#login-with-device`.
    pub fn offer_identity(&mut self, link: LinkId) -> Result<Vec<Action>> {
        let Some(session) = self.sessions.get_mut(&link) else {
            bail!("link {link:?} has no session to transfer over");
        };

        session.offer_identity()?;

        Ok(self.collect())
    }

    /// The user answered [`Action::ConfirmIdentityTransfer`].
    pub fn answer_identity_transfer(
        &mut self,
        link: LinkId,
        confirmed: bool,
    ) -> Result<Vec<Action>> {
        let Some(session) = self.sessions.get_mut(&link) else {
            bail!("link {link:?} has no identity transfer to answer");
        };

        session.answer_transfer(confirmed)?;

        Ok(self.collect())
    }

    /// Take the identity [`Outcome::Received`] announced, once.
    ///
    /// The core does not adopt it. The shell writes it to the Keychain or
    /// Keystore and reopens the node under it, which is the same custody path
    /// as a key generated on device. `docs/keys.md#key-custody`.
    pub fn take_transferred_identity(&mut self, link: LinkId) -> Option<SecretKey> {
        self.transferred.remove(&link)
    }

    /// The user changed a preference, so re-read the policy and rebind it on
    /// every live session.
    ///
    /// Every entry point does this on its own once the store announces a
    /// preference or a trust, block or mute list, because a session that cached
    /// a policy would keep serving a peer the user has just blocked. A session
    /// whose peer the new policy blocks closes here.
    pub fn policy_changed(&mut self) -> Result<Vec<Action>> {
        self.rebind_policy()?;

        Ok(self.collect())
    }

    /// Recompile the policy from the store and bind it on every live session.
    fn rebind_policy(&mut self) -> Result<()> {
        self.policy = Arc::new(query::policy(&self.db, &self.identity)?);

        for session in self.sessions.values_mut() {
            session.set_policy(Arc::clone(&self.policy));
        }

        Ok(())
    }

    /// Store an event the user wrote and the media it attaches, then offer both.
    pub fn publish(&mut self, event: &HashedEvent, media: &[&[u8]]) -> Result<Vec<Action>> {
        db_command::publish_event(&self.db, event, &self.identity, clock::now())?;
        self.store_own_media(event, media)?;

        Ok(self.collect())
    }

    /// Write a backup of the identity key into the shell's cache directory and
    /// ask for the share sheet over it.
    ///
    /// `password` encrypts it as a NIP-49 `ncryptsec` and must be at least
    /// [`backup::MINIMUM_PASSWORD_LENGTH`] characters; without one the file
    /// carries a plain `nsec`. Either way the key is encoded here and the
    /// caller is handed no part of it. `docs/keys.md#backup`.
    pub fn export_key(
        &mut self,
        cache: impl AsRef<Path>,
        password: Option<&str>,
    ) -> Result<Vec<Action>> {
        // A retry writes over the last attempt's file, so only the newest is ever on disk.
        let path = backup::write(&self.custody.identity()?, cache.as_ref(), password)?;

        self.key_backup = Some(path.clone());

        let mut actions = self.collect();
        actions.push(Action::ShareKeyBackup(path));

        Ok(actions)
    }

    /// The share sheet closed, shared or dismissed, so the file goes.
    ///
    /// Dismissing counts as not downloaded rather than an error — the view
    /// leaves its gate closed and the user exports again.
    pub fn key_export_finished(&mut self) -> Result<Vec<Action>> {
        if let Some(path) = self.key_backup.take() {
            backup::remove(&path)?;
        }

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
        let preferences_moved = self.drain_preferences();
        let graph_moved = self.offer_saved_events();

        if (preferences_moved || graph_moved)
            && let Err(error) = self.rebind_policy()
        {
            log::error!("recompiling the policy failed: {error:#}");
        }

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

        // Whatever each session wants of the shell, asked once apiece.
        for session in self.sessions.values_mut() {
            // A held gate, and the value the two users compare over it.
            if let Some(code) = session.request_approval() {
                actions.push(Action::RequestApproval(session.link, code));
            }

            // Who the link turned out to be, so a name typed at the gate lands.
            for pubkey in session.take_identified() {
                actions.push(Action::PeerIdentified(session.link, pubkey));
            }

            // An identity transfer asks each user once, and says how it ended.
            if let Some(sas) = session.take_transfer_prompt() {
                actions.push(Action::ConfirmIdentityTransfer(session.link, sas));
            }

            if let Some(outcome) = session.transfer.take_outcome() {
                actions.push(Action::IdentityTransfer(session.link, outcome));
            }

            // Out of the session before it can close, so a link dropping now does not lose the key.
            if let Some(key) = session.transfer.take_identity() {
                self.transferred.insert(session.link, key);
            }

            // A blob transfer is the opening that justifies a bulk channel.
            let link = session.link;

            match session.poll_l2cap() {
                Ok(step) => actions.extend(step.map(|step| match step {
                    Step::Publish => Action::PublishL2cap(link),
                    Step::Open(psm) => Action::OpenL2cap(link, psm),
                })),
                // The upgrade is bandwidth; a link that cannot ask for it still syncs.
                Err(error) => log::error!("asking for L2CAP on link {link:?} failed: {error:#}"),
            }
        }

        for session in self.sessions.values_mut() {
            actions.extend(writes(session));
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
    ///
    /// Answers whether a trust, block or mute list arrived, which the policy is
    /// compiled from, or whether changes were missed and one might have.
    fn offer_saved_events(&mut self) -> bool {
        let identity = self.identity;
        let mut graph_moved = false;

        loop {
            match self.events.try_recv() {
                Ok(EventChange::Stored(event)) => {
                    graph_moved |= [TRUST, BLOCK, MUTE].contains(&event.kind);

                    if event.pubkey != identity {
                        continue;
                    }

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
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                // The next reconciliation covers missed offers; the policy is re-read in case.
                Err(TryRecvError::Lagged(_)) => graph_moved = true,
            }
        }

        graph_moved
    }

    /// Drain the store's preference channel, answering whether anything moved.
    fn drain_preferences(&mut self) -> bool {
        let mut moved = false;

        while let Ok(_) | Err(TryRecvError::Lagged(_)) = self.pref_changes.try_recv() {
            moved = true;
        }

        moved
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

/// Everything one session has ready to write, on each of its [`Pipe`]s.
///
/// Sealing fails only on a wire that can no longer carry the session, so a
/// failure stops that pipe and leaves the other to finish what it has.
fn writes(session: &mut Session) -> Vec<Action> {
    let link = session.link;
    let mut actions = Vec::new();

    for pipe in [Pipe::Gatt, Pipe::Bulk] {
        loop {
            match session.next_write(pipe) {
                Ok(Some(fragment)) => actions.push(match pipe {
                    Pipe::Gatt => Action::Send(link, fragment),
                    Pipe::Bulk => Action::SendBulk(link, fragment),
                }),
                Ok(None) => break,
                Err(error) => {
                    log::error!("sealing a fragment on link {link:?} failed: {error:#}");
                    break;
                }
            }
        }
    }

    actions
}

#[cfg(test)]
mod tests {
    use super::scheduler::{
        DECLINED_BACKOFF_SECONDS, NEVER_ANSWERED_BACKOFF_SECONDS, REFUSED_BACKOFF_SECONDS,
        WALKED_AWAY_BACKOFF_SECONDS,
    };
    use super::*;
    use coracle_lib::tags::Tags;

    use crate::fixtures::{TempDir, author, custody, note, secret, settle};
    use crate::model::{BlobHash, Policy, Query};
    use crate::session::Session;
    use crate::sync::{Message, SubscriptionId};
    use crate::transport::Frame;
    use coracle_lib::filters::Filter;

    fn node() -> Node {
        Node::new(
            db(),
            custody(SecretKey::generate()),
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

    /// Custody that counts how often the key was asked for.
    #[derive(Default)]
    struct Counted {
        key: Option<SecretKey>,
        reads: std::sync::atomic::AtomicUsize,
    }

    impl crate::keys::KeyCustody for Counted {
        fn identity(&self) -> Result<SecretKey> {
            self.reads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            Ok(self.key.clone().unwrap_or_else(SecretKey::generate))
        }
    }

    #[test]
    fn the_key_is_read_out_per_use_rather_than_held_for_the_nodes_life() {
        let counted = Arc::new(Counted {
            key: Some(secret(1)),
            ..Counted::default()
        });
        let reads = || counted.reads.load(std::sync::atomic::Ordering::Relaxed);

        let mut node = Node::new(
            db(),
            Arc::clone(&counted) as Arc<dyn crate::keys::KeyCustody>,
            Arc::new(crate::blobs::MemoryBlobStore::default()),
        )
        .unwrap();

        // Opening derives the pubkey; work that does not sign reads nothing.
        assert_eq!(reads(), 1);
        node.tick();
        node.policy_changed().unwrap();
        assert_eq!(reads(), 1);

        // Writing a backup does, because that is the key leaving the device.
        let cache = TempDir::new("counted");
        node.export_key(&cache.0, None).unwrap();
        assert_eq!(reads(), 2);
    }

    #[test]
    fn a_file_backed_node_puts_blobs_in_the_blob_directory() {
        let dir = TempDir::new("node");
        let node = Node::open(db(), custody(SecretKey::generate()), &dir.0).unwrap();

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
        let mut node = Node::new(Arc::clone(&db), custody(secret(1)), Arc::clone(&blobs)).unwrap();

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

        Node::new(db, custody(secret(1)), Arc::clone(&blobs)).unwrap();

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
                custody(SecretKey::generate()),
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

        let mut node = Node::new(
            Arc::clone(&db),
            custody(SecretKey::generate()),
            blobs.clone(),
        )
        .unwrap();
        let partial = given_held(&mut node, b"half a transfer");
        let orphan = BlobHash::digest(b"nothing references this");

        blobs.append(&orphan, b"nothing references this").unwrap();

        drop(node);
        Node::new(db, custody(SecretKey::generate()), blobs.clone()).unwrap();

        assert!(!blobs.has(&orphan).unwrap());
        // A transfer in flight has a record from the moment its event was stored.
        assert!(blobs.has(&partial).unwrap());
    }

    #[test]
    fn the_first_tick_starts_scanning_and_advertising_once() {
        let mut node = node();

        let first = node.tick();
        assert!(first.contains(&Action::Scan(true)));
        assert!(first.contains(&Action::Advertise(true)));

        let second = node.tick();
        assert!(
            !second
                .iter()
                .any(|action| matches!(action, Action::Scan(_) | Action::Advertise(_)))
        );
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
            custody(secret(1)),
            Arc::new(crate::blobs::MemoryBlobStore::default()),
        )
        .unwrap();
        let event = note(author(1), 100, "hello", Tags::new());

        node.publish(&event, &[]).unwrap();

        let stored = query::list_events(&db, &Query::new()).unwrap();
        assert_eq!(stored, vec![event]);
    }

    #[test]
    fn publishing_a_trust_list_recompiles_the_policy() {
        let mut node = Node::new(
            db(),
            custody(secret(1)),
            Arc::new(crate::blobs::MemoryBlobStore::default()),
        )
        .unwrap();
        let trust = crate::fixtures::event(
            author(1),
            TRUST,
            100,
            "",
            Tags::new().add("p", [author(2).to_hex()]),
        );

        node.publish(&trust, &[]).unwrap();

        assert!(node.policy().graph.trusted.contains(&author(2)));
    }

    #[test]
    fn writing_a_preference_recompiles_the_policy_without_being_told() {
        let db = db();
        let mut node = Node::new(
            Arc::clone(&db),
            custody(secret(1)),
            Arc::new(crate::blobs::MemoryBlobStore::default()),
        )
        .unwrap();

        db_command::set_preference(&db, "policy.retention_days", "7", 100).unwrap();
        node.tick();

        assert_eq!(node.policy().retention_days, 7);
    }

    #[test]
    fn a_saved_own_event_reaches_a_subscribed_peer() {
        let db = db();
        // Node 1 authors, and a peer's session is attached to the same node.
        let mut node = Node::new(
            Arc::clone(&db),
            custody(secret(1)),
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
            custody(secret(2)),
            Arc::new(crate::blobs::MemoryBlobStore::default()),
            spending(),
        )
        .unwrap();
        let mut peer = Session::open(
            LinkId(3),
            Role::Dialer,
            4096,
            Arc::new(Policy::new(author(2))),
            custody(secret(3)),
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
        let opening = dialer.next_write(Pipe::Gatt).unwrap().unwrap();
        dialer.acknowledge_write(Pipe::Gatt);
        let frame = receiver.receive(&opening).unwrap().unwrap();
        receiver.advance(db, &frame).unwrap();

        let reply = receiver.next_write(Pipe::Gatt).unwrap().unwrap();
        receiver.acknowledge_write(Pipe::Gatt);
        let frame = dialer.receive(&reply).unwrap().unwrap();
        dialer.advance(db, &frame).unwrap();

        let closing = dialer.next_write(Pipe::Gatt).unwrap().unwrap();
        dialer.acknowledge_write(Pipe::Gatt);
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
                custody(SecretKey::generate()),
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

    #[test]
    fn exporting_the_key_asks_for_a_share_sheet_and_finishing_takes_the_file_back() {
        let cache = TempDir::new("export");
        let mut node = node();

        let actions = node.export_key(&cache.0, None).unwrap();
        let [Action::ShareKeyBackup(path)] = actions.as_slice() else {
            panic!("expected one share action, got {actions:?}");
        };

        assert!(path.is_file());
        assert!(
            std::fs::read_to_string(path)
                .unwrap()
                .contains(&node.custody.identity().unwrap().to_nsec())
        );

        let path = path.clone();
        node.key_export_finished().unwrap();

        assert!(!path.exists());
    }

    #[test]
    fn a_refused_export_leaves_nothing_to_share() {
        let cache = TempDir::new("export");
        let mut node = node();

        assert!(node.export_key(&cache.0, Some("short")).is_err());
        assert!(node.key_backup.is_none());
    }

    /// Two nodes over one link, synchronizing, with a user in front of both.
    fn attended_pair() -> (Node, Node) {
        let mut dialer = node();
        let mut receiver = node();

        dialer.notify_foregrounded();
        receiver.notify_foregrounded();
        receiver
            .link_up(LinkId(1), None, Role::Receiver, 4096)
            .unwrap();

        // Only the dialer has anything to say at link-up: it opens the handshake.
        let opening = dialer
            .link_up(LinkId(1), Some(peripheral(1)), Role::Dialer, 4096)
            .unwrap();
        settle(&mut dialer, &mut receiver, opening);

        assert_eq!(dialer.sessions[&LinkId(1)].state, State::Syncing);
        assert_eq!(receiver.sessions[&LinkId(1)].state, State::Syncing);

        (dialer, receiver)
    }

    #[test]
    fn an_identity_transfer_asks_both_users_and_lands_on_the_target() {
        let (mut source, mut target) = attended_pair();
        let key = source.custody.identity().unwrap().to_hex();

        let offered = source.offer_identity(LinkId(1)).unwrap();
        let shown: Vec<Action> = settle(&mut source, &mut target, offered)
            .into_iter()
            .filter(|action| matches!(action, Action::ConfirmIdentityTransfer(..)))
            .collect();

        // The same number on both screens, each asked once.
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[0], shown[1]);

        let accepted = target.answer_identity_transfer(LinkId(1), true).unwrap();
        settle(&mut target, &mut source, accepted);

        let confirmed = source.answer_identity_transfer(LinkId(1), true).unwrap();

        assert!(
            settle(&mut source, &mut target, confirmed)
                .contains(&Action::IdentityTransfer(LinkId(1), Outcome::Received))
        );
        assert_eq!(
            target
                .take_transferred_identity(LinkId(1))
                .map(|key| key.to_hex()),
            Some(key)
        );
        assert!(target.take_transferred_identity(LinkId(1)).is_none());
    }

    #[test]
    fn a_transferred_key_survives_the_link_dropping_before_it_is_taken() {
        let (mut source, mut target) = attended_pair();

        let offered = source.offer_identity(LinkId(1)).unwrap();
        settle(&mut source, &mut target, offered);
        let accepted = target.answer_identity_transfer(LinkId(1), true).unwrap();
        settle(&mut target, &mut source, accepted);
        let confirmed = source.answer_identity_transfer(LinkId(1), true).unwrap();
        settle(&mut source, &mut target, confirmed);

        target.link_down(LinkId(1));

        assert!(target.take_transferred_identity(LinkId(1)).is_some());
    }

    #[test]
    fn backgrounding_ends_an_identity_transfer() {
        let (mut source, mut target) = attended_pair();

        let offered = source.offer_identity(LinkId(1)).unwrap();
        settle(&mut source, &mut target, offered);

        let backgrounded = source.notify_backgrounded();

        assert!(backgrounded.contains(&Action::IdentityTransfer(LinkId(1), Outcome::Refused)));
        assert!(
            settle(&mut source, &mut target, backgrounded)
                .contains(&Action::IdentityTransfer(LinkId(1), Outcome::Refused))
        );
    }

    #[test]
    fn an_identity_transfer_needs_a_link() {
        let mut node = node();

        assert!(node.offer_identity(LinkId(1)).is_err());
        assert!(node.answer_identity_transfer(LinkId(1), true).is_err());
        assert!(node.take_transferred_identity(LinkId(1)).is_none());
    }
}
