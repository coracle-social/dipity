//! The core as the shell sees it: bytes and radio events in, actions out.
//!
//! One object owning every live session, the store, and the identity key.
//! It is sans-io, performing no I/O and reading no clock, which lets the whole
//! stack above the radio run in `cargo test` with two nodes handing each other
//! bytes.
//!
//! # The call shape
//!
//! Every entry point returns [`Action`]s. The one thing that calls the shell
//! back is [`KeyCustody`], for the key a signature needs, and it acts on
//! nothing; everything the shell does for the core it does from an action, so
//! it can answer one synchronously without deadlocking the core against itself.
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

mod notify;
pub use notify::Notification;
use notify::Notifier;
mod scheduler;

pub use scheduler::{MAX_LINKS, RSSI_FLOOR};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, bail};
use coracle_kinds::delete;
use coracle_lib::addresses::Address;
use coracle_lib::events::{EventContent, EventId, HashedEvent};
use coracle_lib::filters::Filter;
use coracle_lib::keys::{PublicKey, SecretKey};
use coracle_lib::tags::Tags;
use tokio::sync::broadcast::{self, error::TryRecvError};

use crate::backup;
use crate::blobs::{BlobStore, FileBlobStore, verified};
use crate::clock;
use crate::db::blob::channel as blob_channel;
use crate::db::blob::channel::BlobChange;
use crate::db::command as db_command;
use crate::db::event::channel::{self, EventChange};
use crate::db::pref::channel::{self as pref_channel, PrefChange};
use crate::db::recipient_signature::channel::{
    self as signature_channel, RecipientSignatureChange,
};
use crate::db::{Db, query};
use crate::keys::KeyCustody;
use crate::link::{LinkId, PeripheralId, Role};
use crate::model::{BLOCK, Blob, BlobHash, CONTACT, MUTE, NotificationPrefs, Policy, Query};
use crate::session::gate::Presence;
use crate::session::l2cap::Step;
use crate::session::transfer::Outcome;
use crate::session::{Ending, Session, State};
use crate::transport::{Channel, Pipe};
use scheduler::Scheduler;

/// Minimum gap between retention sweeps, against a window measured in days.
const SWEEP_INTERVAL_SECONDS: i64 = 60 * 60;

/// How long something stays in the trash before the sweep empties it.
/// `docs/storage.md#the-trash`.
pub const TRASH_SECONDS: i64 = 7 * 86_400;

/// Something the shell does on the core's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Start or stop scanning for our service UUID. Asked again while
    /// scanning, the scan restarts, which makes a platform that reports each
    /// peripheral once in the background report them afresh.
    Scan(bool),
    /// Start or stop advertising it.
    Advertise(bool),
    /// Dial a peripheral the scheduler admitted.
    Connect(PeripheralId),
    /// Wait for a peer that walked away to come back, and connect when it does.
    ///
    /// A standing connect that does not time out and survives the app being
    /// suspended, which is what reaches a peer when no scan or tick would.
    /// Answered like a [`Connect`](Self::Connect), with
    /// [`Node::link_up`] naming the peripheral. A later `Connect` to the same
    /// one supersedes it. `docs/discovery.md#connection-scheduling`.
    WaitFor(PeripheralId),
    /// Give up waiting for a peripheral, whose address has rotated.
    StopWaiting(PeripheralId),
    /// Tear a link down.
    Disconnect(LinkId),
    /// Write one fragment, already sized to the link's MTU.
    ///
    /// The shell reports back with [`Node::write_complete`] once the ATT write
    /// is acknowledged, which releases the next.
    Send(LinkId, Vec<u8>),
    /// Ask the user whether an unadmitted stranger may connect. Answer with
    /// [`Node::approve`]; the link is held up to the gate hold meanwhile.
    ///
    /// The value is what the two users compare, and the gate authenticates
    /// nobody without it. `docs/discovery.md#the-consent-gate`.
    RequestApproval(LinkId, u32),
    /// Who the peer on a link proved to be, once per pubkey it proves, and the
    /// value the two users compare to be sure of it.
    ///
    /// A pet name the user typed at the gate is for a person rather than for a
    /// key, because the gate runs before either side names a pubkey. This is what says
    /// which key that person turned out to hold. The value is the one a held
    /// gate shows, which lets a peer the gate let through be named over the
    /// same comparison. `docs/discovery.md#meeting-somebody`.
    ///
    /// A peer the user named but this device did not recognize has forgotten
    /// them, or holds a pair secret this one does not. It is asked about again
    /// rather than passed over. `docs/discovery.md#meeting-somebody`.
    PeerIdentified {
        /// The link the peer proved itself over.
        link: LinkId,
        /// The pubkey it proved.
        pubkey: PublicKey,
        /// The value both users compare.
        code: u32,
        /// Whether this device dialed the link. Two links to one person
        /// resolve to the one the lower pubkey dialed. A view showing one of
        /// them before that happens can pick the same one the core will keep.
        dialed: bool,
        /// Whether this device recognized the pubkey by its pair secret.
        recognized: bool,
    },
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
    /// separately from GATT, which keeps a slow ATT write from stalling the
    /// transfer.
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
    /// Post a notification. Raised only while the app is in the background and
    /// only for what the user switched on. `docs/storage.md#notifications`.
    Notify(Notification),
    /// Call [`Node::tick`] at or after this unix second.
    ///
    /// This is advisory. The heartbeat and the connection scheduler both
    /// recover on the next radio callback, because iOS runs no timer for a
    /// suspended app.
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
    /// Where the app is, which an identity transfer needs. `None` until the
    /// shell first reports it.
    presence: Option<Presence>,
    /// This store's event channel, through which a stored own event is offered
    /// to every connected peer without the writer knowing.
    events: broadcast::Receiver<EventChange>,
    /// This store's blob channel, through which bytes are reclaimed when their
    /// record goes without whoever removed it holding the blob store.
    blob_changes: broadcast::Receiver<BlobChange>,
    /// This store's preference channel, through which a written preference
    /// rebinds every live session without the writer knowing a node exists.
    pref_changes: broadcast::Receiver<PrefChange>,
    /// This store's signature channel, through which an event becomes
    /// forwardable to every connected peer the moment its author's signature
    /// lands.
    signature_changes: broadcast::Receiver<RecipientSignatureChange>,
    /// When the retention sweep last ran, which keeps frequent ticks from
    /// sweeping often.
    events_swept_at: Option<i64>,
    /// The backup file waiting on a share sheet, kept so that the core can
    /// delete it whichever way the sheet ends.
    key_backup: Option<PathBuf>,
    /// Whether the shell has been told to scan and advertise, which the first
    /// tick does once. `docs/discovery.md#session-lifecycle`.
    radio_started: bool,
    /// Keys an identity transfer delivered, kept here rather than on the
    /// session so a link dropping before the shell takes one does not lose it.
    transferred: BTreeMap<LinkId, SecretKey>,
    /// Links closed because another link to the same person carries the
    /// session. Their peripheral is not redialed while it does.
    duplicates: std::collections::BTreeSet<LinkId>,
    /// Who each peripheral this device dialed turned out to be, which keeps a
    /// person already connected over a link they dialed from being dialed a
    /// second time.
    dialed_as: BTreeMap<PeripheralId, PublicKey>,
    /// What has arrived since the user last looked, and what was announced.
    notifier: Notifier,
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

        let spending = Arc::new(crate::sync::spending::SpendingLedger::open(Arc::clone(&db)));

        let mut node = Self {
            events: channel::subscribe(&db),
            blob_changes: blob_channel::subscribe(&db),
            pref_changes: pref_channel::subscribe(&db),
            signature_changes: signature_channel::subscribe(&db),
            db,
            custody,
            identity,
            policy,
            blobs,
            battery: None,
            sessions: BTreeMap::new(),
            link_peripheral: BTreeMap::new(),
            scheduler: Scheduler::default(),
            spending,
            presence: None,
            events_swept_at: None,
            key_backup: None,
            radio_started: false,
            transferred: BTreeMap::new(),
            duplicates: std::collections::BTreeSet::new(),
            dialed_as: BTreeMap::new(),
            notifier: Notifier::default(),
        };

        node.notifier.prefs = query::notification_prefs(&node.db).unwrap_or_else(|error| {
            log::error!("reading the notification preferences failed: {error:#}");
            NotificationPrefs::default()
        });

        // Whatever was removed while no node was listening is still on disk.
        if let Err(error) = node.sweep_blobs() {
            log::error!("sweeping the blob store at open failed: {error:#}");
        }

        // Opening sweeps, because a device that meets nobody never ticks.
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
        let blobs = Arc::new(FileBlobStore::within(directory)?);

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

        match self.scheduler.next_dial(self.central_links(), &self.busy()) {
            Some(peripheral) => vec![Action::Connect(peripheral)],
            None => Vec::new(),
        }
    }

    /// A dial the shell started failed before a link came up: the peer is
    /// gone, or both phones dialed each other at once. It is retried shortly.
    pub fn dial_failed(&mut self, peripheral: &PeripheralId) -> Vec<Action> {
        self.scheduler.dial_failed(peripheral);

        self.collect()
    }

    /// A dial reached a device that does not serve our service, which matching
    /// Apple's overflow area does now and then. It is left alone until its
    /// address rotates.
    pub fn not_ours(&mut self, peripheral: &PeripheralId) -> Vec<Action> {
        self.scheduler.not_ours(peripheral);

        self.collect()
    }

    /// A GATT connection came up, with the MTU the link negotiated.
    ///
    /// `peripheral` is the one this device dialed, and the scheduler grades the
    /// link's outcome by it; a link the peer dialed has none.
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
            self.scheduler.connected(&peripheral);
            self.link_peripheral.insert(link, peripheral);
        }

        self.sessions.insert(link, session);

        Ok(self.collect())
    }

    /// A link went away. The shell only reports a disconnect that fired, which is
    /// reliable. The session closes rather than waiting out a drain that cannot
    /// complete, because the link is gone and nothing on it will ever finish.
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
    /// A read may hold several fragments, part of one, or both, because the
    /// channel is a byte stream; the wire keeps what does not complete one.
    pub fn bulk_received(&mut self, link: LinkId, read: &[u8]) -> Vec<Action> {
        let received = self.receive_bulk(link, read);

        self.after_receiving(link, received)
    }

    /// End the link if reading it, or answering on it, failed, and collect
    /// either way.
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
    ///
    /// A fetch the channel was carrying starts again on GATT.
    pub fn l2cap_unavailable(&mut self, link: LinkId) -> Result<Vec<Action>> {
        let db = Arc::clone(&self.db);

        self.on_session(link, move |session| {
            session.l2cap_unavailable()?;
            session.maybe_fetch_blob(&db)
        })
    }

    /// Carry a shell answer to the session it belongs to, and collect.
    ///
    /// The link may already be down — a channel the shell was opening comes
    /// back after a disconnect as readily as before one — and an answer with no
    /// session left to hear it is dropped rather than being an error. One the
    /// session cannot take ends it, the way a frame it cannot read does. The
    /// shell is told to disconnect, and the session does not hold its slot.
    fn on_session(
        &mut self,
        link: LinkId,
        answer: impl FnOnce(&mut Session) -> Result<()>,
    ) -> Result<Vec<Action>> {
        let answered = match self.sessions.get_mut(&link) {
            Some(session) => answer(session),
            None => Ok(()),
        };

        Ok(self.after_receiving(link, answered))
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

        // Who a peripheral turned out to be is kept only while it is heard, because its id rotates.
        let scheduler = &self.scheduler;
        self.dialed_as
            .retain(|peripheral, _| scheduler.knows(peripheral));

        let mut actions = Vec::new();

        if !self.radio_started {
            self.radio_started = true;
            actions.extend([Action::Scan(true), Action::Advertise(true)]);
        }

        actions.extend(
            self.scheduler
                .lapsed_waits()
                .into_iter()
                .map(Action::StopWaiting),
        );

        // A queued candidate may now be past its rate limit, its backoff, or the link cap.
        if let Some(peripheral) = self.scheduler.next_dial(self.central_links(), &self.busy()) {
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

    /// The app went to the background.
    pub fn notify_backgrounded(&mut self) -> Vec<Action> {
        self.set_presence(Presence::Background)
    }

    /// Record where the app is and rebind it on every live session.
    ///
    /// Leaving the foreground ends any identity transfer, which is the one
    /// flow that does not run with nobody in front of the screen.
    fn set_presence(&mut self, presence: Presence) -> Vec<Action> {
        self.presence = Some(presence);

        if presence == Presence::Foreground {
            self.notifier.seen();
        }

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

    /// Re-read the policy and rebind it on every live session after the user
    /// changed a preference.
    ///
    /// Every entry point does this on its own once the store announces a
    /// preference, a contact card, or a block or mute list, because a session that cached
    /// a policy would keep serving a peer the user has just blocked. A session
    /// whose peer the new policy blocks closes here.
    pub fn policy_changed(&mut self) -> Result<Vec<Action>> {
        self.rebind_policy()?;

        Ok(self.collect())
    }

    /// Recompile the policy from the store and bind it on every live session.
    ///
    /// A change to what moves between peers forgets the old policy's refusals,
    /// evicts what the new Accept scope no longer admits, and reconciles every
    /// live session again. `docs/sync.md#resyncing`.
    fn rebind_policy(&mut self) -> Result<()> {
        let before = Arc::clone(&self.policy);
        self.policy = Arc::new(query::policy(&self.db, &self.identity)?);

        for session in self.sessions.values_mut() {
            session.set_policy(Arc::clone(&self.policy));
        }

        if !self.policy.moves_sync(&before) {
            return Ok(());
        }

        db_command::forget_policy_refusals(&self.db)?;

        if self.policy.changes_accept(&before) {
            db_command::evict_out_of_scope(&self.db, &self.policy, clock::now())?;
        }

        for session in self.sessions.values_mut() {
            if let Err(error) = session.resync(&self.db) {
                log::error!("resyncing link {:?} failed: {error:#}", session.link);
            }
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
        // Only the newest file is ever on disk, because a retry writes over the last attempt's.
        let path = backup::write(&self.custody.identity()?, cache.as_ref(), password)?;

        Ok(self.key_backup_written(path))
    }

    /// A backup was written at `path`, by [`export_key`](Self::export_key) or
    /// by a caller that wrote it without holding the node: keep it until the
    /// share sheet closes, and ask for the sheet.
    pub fn key_backup_written(&mut self, path: PathBuf) -> Vec<Action> {
        self.key_backup = Some(path.clone());

        let mut actions = self.collect();
        actions.push(Action::ShareKeyBackup(path));

        actions
    }

    /// Where the identity key is read from, for work that reads it without
    /// holding the node, the backup's scrypt above all.
    #[must_use]
    pub fn custody(&self) -> Arc<dyn KeyCustody> {
        Arc::clone(&self.custody)
    }

    /// Delete the file once the share sheet closes, shared or dismissed.
    ///
    /// Dismissing counts as not downloaded rather than an error, and the user
    /// can export again.
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
                db_command::complete_blob(&self.db, &blob.sha256, size)?;
            }
        }

        Ok(())
    }

    // ------------------------------------ Draining what the sessions produced

    /// How many links this device dialed.
    ///
    /// A peer dialing us must not spend one of our slots, because
    /// `docs/discovery.md` caps *central* links. Six inbound connections would
    /// otherwise stop this device dialing anyone, which in a crowd is the
    /// moment it most wants to.
    fn central_links(&self) -> usize {
        self.link_peripheral.len()
    }

    /// Peripherals whose person is connected already, over any link, which a
    /// second dial would only duplicate.
    fn busy(&self) -> std::collections::BTreeSet<PeripheralId> {
        let connected: std::collections::BTreeSet<PublicKey> = self
            .sessions
            .values()
            .filter_map(|session| session.peer.as_ref())
            .flat_map(|peer| peer.pubkeys.iter().copied())
            .collect();

        self.dialed_as
            .iter()
            .filter(|(_, pubkey)| connected.contains(pubkey))
            .map(|(peripheral, _)| peripheral.clone())
            .collect()
    }

    /// Sweep every session for work: fragments to write, links to tear down,
    /// and the earliest deadline worth waking for.
    ///
    /// This is the one place actions are produced, which keeps an entry point
    /// from forgetting to flush a session it advanced.
    fn collect(&mut self) -> Vec<Action> {
        self.close_duplicates();

        let preferences_moved = self.drain_preferences();
        let graph_moved = self.offer_saved_events();

        if (preferences_moved || graph_moved)
            && let Err(error) = self.rebind_policy()
        {
            log::error!("recompiling the policy failed: {error:#}");
        }

        if preferences_moved {
            match query::notification_prefs(&self.db) {
                Ok(prefs) => self.notifier.prefs = prefs,
                Err(error) => log::error!("reading the notification preferences failed: {error:#}"),
            }
        }

        let (wanted_more, referenced) = self.drain_blob_changes();

        // Heartbeats go out before writes are drained, which keeps a quiet session proving alive.
        for session in self.sessions.values_mut() {
            if let Err(error) = session.maybe_heartbeat() {
                log::error!(
                    "sending a heartbeat on link {:?} failed: {error:#}",
                    session.link
                );
            }

            for hash in &referenced {
                session.reconsider_blob(hash);
            }

            if wanted_more && let Err(error) = session.maybe_fetch_blob(&self.db) {
                log::error!(
                    "asking for a wanted blob on link {:?} failed: {error:#}",
                    session.link
                );
            }
        }

        let mut actions = Vec::new();
        let mut asking = Vec::new();

        // Whatever each session wants of the shell, asked once apiece.
        for session in self.sessions.values_mut() {
            // A held gate, and the value the two users compare over it.
            if let Some(code) = session.request_approval() {
                actions.push(Action::RequestApproval(session.link, code));
                asking.push((session.link, None));
            }

            // Identifying the link lands a name typed at the gate, or lets one be given now.
            let code = session.pairing_code();
            let identified = session.take_identified();

            if let (Some(peripheral), Some(pubkey)) = (
                self.link_peripheral.get(&session.link),
                identified.iter().min(),
            ) {
                self.dialed_as.insert(peripheral.clone(), *pubkey);
            }

            for pubkey in identified {
                if let Some(code) = code {
                    let recognized = session.recognized(&pubkey);

                    actions.push(Action::PeerIdentified {
                        link: session.link,
                        pubkey,
                        code,
                        dialed: session.role == Role::Dialer,
                        recognized,
                    });
                    asking.push((session.link, Some((pubkey, recognized))));
                }
            }

            // Only a phone that has published nothing as itself may take another identity.
            if session.transfer.invited()
                && published_as(&self.db, &self.identity)
                && let Err(error) = session.turn_away_transfer()
            {
                log::error!(
                    "turning away an identity transfer on link {:?} failed: {error:#}",
                    session.link
                );
            }

            // An identity transfer asks each user once, and says how it ended.
            if let Some(sas) = session.take_transfer_prompt() {
                actions.push(Action::ConfirmIdentityTransfer(session.link, sas));
            }

            if let Some(outcome) = session.transfer.take_outcome() {
                actions.push(Action::IdentityTransfer(session.link, outcome));
            }

            // Taken out before the session can close, so that a link dropping now keeps the key.
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

        actions.extend(self.notifications(asking));

        for session in self.sessions.values_mut() {
            actions.extend(writes(session));
        }

        let closed: Vec<(LinkId, Ending, bool, bool)> = self
            .sessions
            .iter()
            .filter(|(_, session)| session.state == State::Closed)
            .map(|(link, session)| {
                (
                    *link,
                    session.ending,
                    session.was_harvested(),
                    session.synced(),
                )
            })
            .collect();

        let mut rescan = false;

        // Grading is one decision because every teardown passes through here.
        for (link, ending, harvested, synced) in closed {
            self.sessions.remove(&link);

            let duplicate = self.duplicates.remove(&link);

            if let Some(peripheral) = self.link_peripheral.remove(&link) {
                match ending {
                    _ if duplicate => self.scheduler.duplicate(&peripheral),
                    _ if harvested => self.scheduler.harvested(&peripheral),
                    Ending::WalkedAway => {
                        self.scheduler.walked_away(&peripheral);

                        // Only a peer that synced is worth a standing connect: one that refused us would loop.
                        if synced {
                            self.scheduler.awaiting(&peripheral);
                            actions.push(Action::WaitFor(peripheral));
                        }

                        rescan |= self.presence != Some(Presence::Foreground);
                    }
                    Ending::Refused => self.scheduler.refused(&peripheral),
                }
            }

            actions.push(Action::Disconnect(link));
        }

        // A fresh scan brings the peer back to a suspended phone, which has no tick to redial from.
        if rescan && self.radio_started {
            actions.push(Action::Scan(true));
        }

        if let Some(deadline) = self.sessions.values().map(Session::deadline).min() {
            actions.push(Action::WakeAt(deadline));
        }

        actions
    }

    /// Close all but one of several syncing links to the same person, which is
    /// what two phones dialing each other at once leaves behind.
    ///
    /// Both ends keep the link the lower of the two pubkeys dialed, which lets
    /// both close the same one without a word passing between them. Two links
    /// dialed by the same side cannot be told apart that way, and both stay.
    fn close_duplicates(&mut self) {
        let identity = self.identity;
        let mut kept: BTreeMap<PublicKey, (PublicKey, LinkId)> = BTreeMap::new();
        let mut closing = Vec::new();

        for (link, session) in &self.sessions {
            if session.state != State::Syncing {
                continue;
            }

            let Some(person) = session
                .peer
                .as_ref()
                .and_then(|peer| peer.pubkeys.iter().min().copied())
            else {
                continue;
            };
            let dialer = if session.role == Role::Dialer {
                identity
            } else {
                person
            };

            match kept.get(&person).copied() {
                None => {
                    kept.insert(person, (dialer, *link));
                }
                Some((their_dialer, their_link)) if their_dialer != dialer => {
                    if dialer < their_dialer {
                        closing.push(their_link);
                        kept.insert(person, (dialer, *link));
                    } else {
                        closing.push(*link);
                    }
                }
                Some(_) => {}
            }
        }

        for link in closing {
            if let Some(session) = self.sessions.get_mut(&link) {
                session.close(Ending::Refused);
                self.duplicates.insert(link);
            }
        }
    }

    /// Offer events the store just saved to every connected peer.
    ///
    /// Drains the store's event channel, which anything that stores an event
    /// announces on — a publish, or an ingest — so the writer never has to know a
    /// peer is attached. Each session decides what it may be offered, by the
    /// same query a fresh `REQ` would run, and an event held for this device
    /// alone goes nowhere.
    ///
    /// Answers whether a contact card, block or mute list arrived, which the policy is
    /// compiled from, or whether changes were missed and one might have.
    fn offer_saved_events(&mut self) -> bool {
        let mut graph_moved = false;
        let mut saved = Vec::new();

        loop {
            match self.events.try_recv() {
                Ok(EventChange::Stored(event)) => {
                    graph_moved |= [CONTACT, BLOCK, MUTE].contains(&event.kind);
                    // What arrives on screen is seen as it arrives.
                    if self.presence != Some(Presence::Foreground) {
                        self.notifier
                            .stored(&event, &self.identity, &self.policy.graph);
                    }
                    saved.push(*event);
                }
                // Seen events are not newly stored, and Deleted are gone. Both are the view's.
                Ok(_) => {}
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                // The next reconciliation covers missed offers; the policy is re-read in case.
                Err(TryRecvError::Lagged(_)) => graph_moved = true,
            }
        }

        // An event someone else wrote becomes forwardable when its signature to this device lands.
        loop {
            match self.signature_changes.try_recv() {
                Ok(RecipientSignatureChange::Stored(signature))
                    if signature.recipient_pubkey == self.identity =>
                {
                    match query::get_event(&self.db, &signature.event_id) {
                        Ok(Some(event)) => saved.push(event),
                        Ok(None) => {}
                        Err(error) => log::error!("reading a newly signed event failed: {error:#}"),
                    }
                }
                Ok(_) | Err(TryRecvError::Lagged(_)) => {}
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }

        for event in &saved {
            self.offer(event);
        }

        graph_moved
    }

    /// Offer one event to every connected peer that may have it.
    fn offer(&mut self, event: &HashedEvent) {
        for session in self.sessions.values_mut() {
            if let Err(error) = session.offer_event(&self.db, event) {
                log::error!(
                    "offering a saved event on link {:?} failed: {error:#}",
                    session.link
                );
            }
        }
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
    /// Drains the store's blob channel, which event deletion announces on. A
    /// store holds bytes for exactly as long as the `blob` table says to,
    /// because the record is what references the file — and whoever removed the
    /// row does not have to hold the blob store to say so.
    ///
    /// Answers whether the want list grew, or might have, which every syncing
    /// session has to hear before it asks for anything more, and which hashes
    /// gained a reference a peer may now be able to serve.
    fn drain_blob_changes(&mut self) -> (bool, Vec<BlobHash>) {
        let mut wanted_more = false;
        let mut referenced = Vec::new();

        loop {
            match self.blob_changes.try_recv() {
                Ok(BlobChange::Removed(sha256)) => {
                    if let Err(error) = self.blobs.delete(&sha256) {
                        log::error!("deleting the bytes of blob {sha256} failed: {error:#}");
                    }
                }
                Ok(BlobChange::Recorded(_)) => wanted_more = true,
                Ok(BlobChange::Referenced(sha256)) => {
                    wanted_more = true;
                    referenced.push(sha256);
                }
                // Progress and completion are the transfer layer's and the view's.
                Ok(_) => {}
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                // The sweep reclaims what a dropped removal would orphan for good.
                Err(TryRecvError::Lagged(_)) => {
                    wanted_more = true;

                    if let Err(error) = self.sweep_blobs() {
                        log::error!("sweeping the blob store after a lag failed: {error:#}");
                    }
                }
            }
        }

        (wanted_more, referenced)
    }

    /// Delete every byte held for a hash the `blob` table has no record of.
    ///
    /// The channel is the prompt path and it is lossy: a subscriber that falls
    /// behind misses removals, and nothing is listening at all while the app is
    /// closed. The table is the record either way, and it settles which files
    /// may stay.
    fn sweep_blobs(&self) -> Result<()> {
        let recorded = query::recorded_blob_hashes(&self.db)?;

        for sha256 in self.blobs.hashes()? {
            if !recorded.contains(&sha256) {
                self.blobs.delete(&sha256)?;
            }
        }

        Ok(())
    }

    /// What to tell a user who is not looking: somebody asking to pair on one
    /// of `asking`, and new writing. A held gate names nobody, and a peer
    /// already named is asking only if this device did not recognize them.
    fn notifications(&mut self, asking: Vec<(LinkId, Option<(PublicKey, bool)>)>) -> Vec<Action> {
        let background = self.presence != Some(Presence::Foreground);
        let mut notifications = Vec::new();

        for (link, identified) in asking {
            let asks = identified.is_none_or(|(pubkey, recognized)| {
                !recognized || !self.policy.graph.contacts.contains(&pubkey)
            });
            let pubkey = identified.map(|(pubkey, _)| pubkey);

            if asks && let Some(notification) = self.notifier.pairing(link, pubkey, background) {
                notifications.push(Action::Notify(notification));
            }
        }

        if let Some((count, latest)) = self.notifier.content(clock::now(), background) {
            let author =
                query::name_for(&self.db, &self.identity, &latest.pubkey).unwrap_or_else(|error| {
                    log::error!("reading a name for a notification failed: {error:#}");
                    None
                });

            notifications.push(Action::Notify(Notification::Content {
                count,
                author,
                excerpt: notify::excerpt(&latest),
            }));
        }

        notifications
    }

    /// Put an event in the trash, retracting it at once if the user wrote it.
    /// `docs/storage.md#the-trash`.
    pub fn trash(&mut self, id: &EventId) -> Result<Vec<Action>> {
        let now = clock::now();

        if !db_command::set_trashed(&self.db, id, true, now)? {
            return Ok(self.collect());
        }

        if let Some(event) = query::get_event(&self.db, id)? {
            self.retract(&event, now)?;
        }

        Ok(self.collect())
    }

    /// Take an event back out of the trash. One the user wrote is restored for
    /// peers too, by retracting the kind 5 that retracted it; one its author
    /// retracted stays where it is.
    /// `docs/storage.md#the-trash`.
    pub fn restore(&mut self, id: &EventId) -> Result<Vec<Action>> {
        let now = clock::now();
        let event = query::get_event(&self.db, id)?;

        if let Some(event) = &event
            && event.pubkey != self.identity
            && query::is_deleted(&self.db, event)?
        {
            bail!("{id} was retracted by its author, so it cannot be put back");
        }

        db_command::set_trashed(&self.db, id, false, now)?;

        if let Some(event) = event
            && event.pubkey == self.identity
        {
            for deletion in query::deletions(&self.db, &event)? {
                db_command::publish_event(
                    &self.db,
                    &retraction(&deletion, now),
                    &self.identity,
                    now,
                )?;
            }
        }

        Ok(self.collect())
    }

    /// Delete everything in the trash, from this device. `docs/storage.md#the-trash`.
    pub fn empty_trash(&mut self) -> Result<Vec<Action>> {
        self.delete_trashed(i64::MAX)?;

        Ok(self.collect())
    }

    /// Delete what went into the trash before `before`, retracting anything of
    /// the user's that went in before trashing did.
    fn delete_trashed(&mut self, before: i64) -> Result<usize> {
        let mut deleted = 0;

        for (id, trashed_at) in query::trashed(&self.db)? {
            if trashed_at >= before {
                continue;
            }

            if let Some(event) = query::get_event(&self.db, &id)? {
                self.retract(&event, clock::now())?;
            }

            if db_command::forget_event(&self.db, &id)? {
                deleted += 1;
            }
        }

        Ok(deleted)
    }

    /// Publish a kind 5 for `event` if the user wrote it and none covers it yet.
    fn retract(&self, event: &HashedEvent, at: i64) -> Result<()> {
        if event.pubkey == self.identity && !query::is_deleted(&self.db, event)? {
            db_command::publish_event(&self.db, &retraction(event, at), &self.identity, at)?;
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

            match db_command::forget_events_seen_before(&self.db, &self.identity(), cutoff, now) {
                Ok(forgotten) => log::debug!("the retention sweep forgot {forgotten} events"),
                Err(error) => log::error!("the retention sweep failed: {error:#}"),
            }

            match self.delete_trashed(now - TRASH_SECONDS) {
                Ok(deleted) => log::debug!("the trash sweep deleted {deleted} events"),
                Err(error) => log::error!("the trash sweep failed: {error:#}"),
            }
        }
    }
}

/// A NIP-09 request that the user's `event` be forgotten, naming its id, its
/// kind, and its address when it has one.
fn retraction(event: &HashedEvent, at: i64) -> HashedEvent {
    let mut tags = Tags::new()
        .add("e", [event.id.to_hex()])
        .add("k", [event.kind.to_string()]);

    if let Some(address) = Address::from_event(event) {
        tags = tags.add("a", [address.to_string()]);
    }

    EventContent::new()
        .with_tags(tags)
        .with_kind(delete::KIND)
        .with_created_at(at)
        .with_pubkey(event.pubkey)
        .with_id()
}

/// Everything one session has ready to write, on each of its [`Pipe`]s.
///
/// Sealing fails only on a wire that can no longer carry the session. A
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

/// Whether the store holds anything `identity` wrote, which is what makes a
/// phone's own key worth keeping. A store that cannot be read counts as one
/// that does, which keeps a failure from costing the user what they published.
/// `docs/keys.md#login-with-device`.
fn published_as(db: &Db, identity: &PublicKey) -> bool {
    let mine = Query::new().with_filter(Filter::new().add_authors([*identity]).add_limit(1));

    query::list_events(db, &mine).map_or(true, |events| !events.is_empty())
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
    use crate::model::{BlobHash, Policy, Query, keys};
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

    /// The device's own media has to be in the table, because the sweep at open
    /// takes every file the table does not know.
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

        db_command::forget_events_seen_before(&node.db, &node.identity(), 2_000, 2_000).unwrap();

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
    fn dials_still_on_their_way_count_against_the_link_cap() {
        let mut node = node();

        for index in 0..MAX_LINKS {
            clock::at(1_000 + index as i64, || {
                assert_eq!(
                    node.peripheral_seen(&peripheral(index as u8), -80),
                    vec![Action::Connect(peripheral(index as u8))]
                );
            });
        }

        // None of the six has come up yet, and a seventh would exceed the cap when they do.
        clock::at(1_000 + MAX_LINKS as i64, || {
            assert!(node.peripheral_seen(&peripheral(200), -80).is_empty());
        });
    }

    #[test]
    fn a_weaker_reading_sorts_a_candidate_behind_a_nearer_one() {
        let mut node = node();

        // The first sighting takes the rate limit and the rest queue.
        clock::at(1_000, || {
            node.peripheral_seen(&peripheral(1), -60);
            node.peripheral_seen(&peripheral(2), -70);
            node.peripheral_seen(&peripheral(3), -75);
            node.peripheral_seen(&peripheral(2), -85);
        });

        // The second walked away while the third walked up.
        clock::at(1_001, || {
            assert_eq!(
                node.scheduler.next_dial(0, &Default::default()),
                Some(peripheral(3))
            );
        });
    }

    #[test]
    fn a_candidate_unheard_for_a_rotation_is_forgotten() {
        let mut node = node();

        // The first sighting takes the rate limit and the second queues.
        clock::at(1_000, || {
            node.peripheral_seen(&peripheral(9), -80);
            node.peripheral_seen(&peripheral(1), -80);
        });

        // Long after its id rotated, dialing it would dial a ghost.
        clock::at(1_000 + scheduler::CANDIDATE_TTL_SECONDS, || {
            assert!(node.scheduler.next_dial(0, &Default::default()).is_none());
        });
    }

    #[test]
    fn a_failed_dial_is_retried_within_seconds() {
        let mut node = node();

        clock::at(1_000, || {
            assert_eq!(
                node.peripheral_seen(&peripheral(1), -60),
                vec![Action::Connect(peripheral(1))]
            );
            node.dial_failed(&peripheral(1));
        });

        // Not a never-answered minute: the retry comes inside the jittered window.
        let retried = clock::at(
            1_000 + scheduler::DIAL_FAILED_BACKOFF_SECONDS + scheduler::DIAL_FAILED_JITTER_SECONDS,
            || node.tick(),
        );

        assert!(retried.contains(&Action::Connect(peripheral(1))));
    }

    #[test]
    fn a_harvester_is_not_redialed_until_its_address_rotates() {
        let mut node = node();

        clock::at(1_000, || {
            node.peripheral_seen(&peripheral(1), -60);
            node.scheduler.harvested(&peripheral(1));
        });

        let soon = clock::at(1_000 + scheduler::WALKED_AWAY_BACKOFF_SECONDS + 1, || {
            node.tick()
        });
        assert!(!soon.contains(&Action::Connect(peripheral(1))));

        let rotated = clock::at(1_000 + scheduler::HARVESTED_BACKOFF_SECONDS, || {
            node.peripheral_seen(&peripheral(1), -60)
        });
        assert!(rotated.contains(&Action::Connect(peripheral(1))));
    }

    #[test]
    fn a_linked_peripheral_is_not_dialed_again_while_its_link_is_up() {
        let mut node = node();

        clock::at(1_000, || {
            node.peripheral_seen(&peripheral(1), -60);
            node.link_up(LinkId(1), Some(peripheral(1)), Role::Dialer, 4096)
                .unwrap();
        });

        // Still advertising, and still in range, but already linked.
        clock::at(1_100, || {
            assert!(
                !node
                    .peripheral_seen(&peripheral(1), -60)
                    .contains(&Action::Connect(peripheral(1)))
            );
        });
    }

    #[test]
    fn a_peer_whose_link_dropped_is_redialed_without_being_seen_again() {
        let mut node = node();

        clock::at(1_000, || {
            node.peripheral_seen(&peripheral(1), -60);
            node.link_up(LinkId(1), Some(peripheral(1)), Role::Dialer, 4096)
                .unwrap();
            node.link_down(LinkId(1));
        });

        // A radio reporting each device once never reports this one again; the queue remembers it.
        let redialed = clock::at(1_000 + scheduler::WALKED_AWAY_BACKOFF_SECONDS, || {
            node.tick()
        });

        assert!(redialed.contains(&Action::Connect(peripheral(1))));
    }

    #[test]
    fn a_peer_that_synced_and_walked_away_in_the_background_is_waited_for() {
        let mut one = node();
        let mut two = node();

        one.notify_foregrounded();
        two.notify_foregrounded();

        clock::at(1_000, || {
            one.tick();
            one.peripheral_seen(&peripheral(1), -60);
            two.link_up(LinkId(1), None, Role::Receiver, 4096).unwrap();
            let opening = one
                .link_up(LinkId(1), Some(peripheral(1)), Role::Dialer, 4096)
                .unwrap();
            settle(&mut one, &mut two, opening);
            one.notify_backgrounded();
        });

        let gone = clock::at(1_100, || one.link_down(LinkId(1)));

        assert!(gone.contains(&Action::WaitFor(peripheral(1))));
        assert!(gone.contains(&Action::Scan(true)));

        // Once its address has rotated, the peer cannot come back under it.
        let later = clock::at(1_100 + scheduler::AWAIT_SECONDS, || one.tick());

        assert!(later.contains(&Action::StopWaiting(peripheral(1))));
    }

    #[test]
    fn a_link_that_never_synced_is_not_waited_for() {
        let mut node = node();

        clock::at(1_000, || {
            node.peripheral_seen(&peripheral(1), -60);
            node.link_up(LinkId(1), Some(peripheral(1)), Role::Dialer, 4096)
                .unwrap();

            assert!(
                !node
                    .link_down(LinkId(1))
                    .contains(&Action::WaitFor(peripheral(1)))
            );
        });
    }

    #[test]
    fn a_device_that_is_not_ours_is_left_alone_until_its_address_rotates() {
        let mut node = node();

        clock::at(1_000, || {
            node.peripheral_seen(&peripheral(1), -60);
            node.not_ours(&peripheral(1));
        });

        let soon = clock::at(1_000 + scheduler::NEVER_ANSWERED_BACKOFF_SECONDS, || {
            node.peripheral_seen(&peripheral(1), -60)
        });

        assert!(!soon.contains(&Action::Connect(peripheral(1))));
    }

    #[test]
    fn a_peer_that_forgot_the_user_is_not_recognized_by_them_again() {
        let mut one = node();
        let mut two = node();

        one.notify_foregrounded();
        two.notify_foregrounded();

        let meet = |one: &mut Node, two: &mut Node, link: u64| {
            two.link_up(LinkId(link), None, Role::Receiver, 4096)
                .unwrap();
            let opening = one
                .link_up(LinkId(link), Some(peripheral(1)), Role::Dialer, 4096)
                .unwrap();
            let actions = settle(one, two, opening);
            one.link_down(LinkId(link));
            two.link_down(LinkId(link));
            actions
        };

        // Recognized by whom, as each side announced the other.
        let recognized = |actions: &[Action], pubkey: PublicKey| {
            actions.iter().find_map(|action| match action {
                Action::PeerIdentified {
                    pubkey: proved,
                    recognized,
                    ..
                } if *proved == pubkey => Some(*recognized),
                _ => None,
            })
        };

        clock::at(1_000, || meet(&mut one, &mut two, 1));
        let again = clock::at(2_000, || meet(&mut one, &mut two, 2));

        assert_eq!(recognized(&again, two.identity()), Some(true));
        assert_eq!(recognized(&again, one.identity()), Some(true));

        db_command::forget_pairing(&one.db, &two.identity()).unwrap();
        let after = clock::at(3_000, || meet(&mut one, &mut two, 3));

        // Two still holds the old secret, but one no longer offers a tag for it.
        assert_eq!(recognized(&after, two.identity()), Some(false));
        assert_eq!(recognized(&after, one.identity()), Some(false));
    }

    #[test]
    fn two_phones_that_dialed_each_other_keep_the_same_one_link() {
        let mut one = node();
        let mut two = node();

        one.notify_foregrounded();
        two.notify_foregrounded();

        // One dials two on link 1, and two dials one on link 2.
        two.link_up(LinkId(1), None, Role::Receiver, 4096).unwrap();
        let opening = one
            .link_up(LinkId(1), Some(peripheral(1)), Role::Dialer, 4096)
            .unwrap();
        settle(&mut one, &mut two, opening);

        one.link_up(LinkId(2), None, Role::Receiver, 4096).unwrap();
        let opening = two
            .link_up(LinkId(2), Some(peripheral(2)), Role::Dialer, 4096)
            .unwrap();
        settle(&mut two, &mut one, opening);

        let kept = |node: &Node| node.sessions.keys().copied().collect::<Vec<_>>();

        assert_eq!(kept(&one).len(), 1, "one link to the same person");
        assert_eq!(kept(&one), kept(&two), "both ends kept the same link");
    }

    #[test]
    fn a_person_connected_over_their_own_dial_is_not_dialed_again() {
        let mut one = node();
        let mut two = node();

        one.notify_foregrounded();
        two.notify_foregrounded();

        // One dialed two once and knows whose that peripheral is.
        clock::at(1_000, || {
            one.peripheral_seen(&peripheral(1), -60);
            two.link_up(LinkId(1), None, Role::Receiver, 4096).unwrap();
            let opening = one
                .link_up(LinkId(1), Some(peripheral(1)), Role::Dialer, 4096)
                .unwrap();
            settle(&mut one, &mut two, opening);
            one.link_down(LinkId(1));
            two.link_down(LinkId(1));
        });

        // Now two dials one, and the session is up the other way round.
        clock::at(1_010, || {
            one.link_up(LinkId(2), None, Role::Receiver, 4096).unwrap();
            let opening = two
                .link_up(LinkId(2), Some(peripheral(2)), Role::Dialer, 4096)
                .unwrap();
            settle(&mut two, &mut one, opening);
        });

        // Two's advertisement keeps arriving, but dialing it would only make a duplicate.
        let actions = clock::at(1_000 + scheduler::WALKED_AWAY_BACKOFF_SECONDS + 5, || {
            one.peripheral_seen(&peripheral(1), -60)
        });

        assert!(!actions.contains(&Action::Connect(peripheral(1))));
    }

    #[test]
    fn a_link_the_core_closed_frees_its_slot_without_a_disconnect_report() {
        // The slot returns when the session goes, because the shell may never report back.
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
    fn naming_somebody_recompiles_the_policy() {
        let mut node = Node::new(
            db(),
            custody(secret(1)),
            Arc::new(crate::blobs::MemoryBlobStore::default()),
        )
        .unwrap();
        let card = crate::fixtures::event(
            author(1),
            CONTACT,
            100,
            "Ben",
            Tags::new().add("d", [author(2).to_hex()]),
        );

        node.publish(&card, &[]).unwrap();

        assert!(node.policy().graph.contacts.contains(&author(2)));
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

    fn trash_node(db: &Arc<Db>) -> Node {
        Node::new(
            Arc::clone(db),
            custody(secret(1)),
            Arc::new(crate::blobs::MemoryBlobStore::default()),
        )
        .unwrap()
    }

    #[test]
    fn emptying_the_trash_retracts_the_users_own_and_drops_the_rest() {
        let db = db();
        let mut node = trash_node(&db);
        let mine = note(author(1), 100, "mine", Tags::new());
        let theirs = note(author(3), 100, "theirs", Tags::new());
        let kept = note(author(3), 100, "not trashed", Tags::new());

        db_command::publish_event(&db, &mine, &author(1), 100).unwrap();
        db_command::receive_event(&db, &theirs, &[author(3)], 100).unwrap();
        db_command::receive_event(&db, &kept, &[author(3)], 100).unwrap();
        db_command::set_trashed(&db, &mine.id, true, 200).unwrap();
        db_command::set_trashed(&db, &theirs.id, true, 200).unwrap();

        node.empty_trash().unwrap();

        assert!(query::get_event(&db, &mine.id).unwrap().is_none());
        assert!(query::get_event(&db, &theirs.id).unwrap().is_none());
        assert!(query::get_event(&db, &kept.id).unwrap().is_some());
        assert!(query::trashed(&db).unwrap().is_empty());

        let retractions = query::list_events(
            &db,
            &Query::new().with_filter(Filter::new().add_kinds([delete::KIND])),
        )
        .unwrap();
        assert_eq!(retractions.len(), 1);
        assert_eq!(retractions[0].pubkey, author(1));
    }

    #[test]
    fn trashing_the_users_own_retracts_it_at_once_and_keeps_it_in_the_trash() {
        let db = db();
        let mut node = trash_node(&db);
        let mine = note(author(1), 100, "mine", Tags::new());
        let theirs = note(author(3), 100, "theirs", Tags::new());
        let retractions = || {
            query::list_events(
                &db,
                &Query::new().with_filter(Filter::new().add_kinds([delete::KIND])),
            )
            .unwrap()
        };

        db_command::publish_event(&db, &mine, &author(1), 100).unwrap();
        db_command::receive_event(&db, &theirs, &[author(3)], 100).unwrap();

        clock::at(200, || node.trash(&mine.id)).unwrap();
        clock::at(200, || node.trash(&theirs.id)).unwrap();

        assert_eq!(retractions().len(), 1);
        assert!(query::get_event(&db, &mine.id).unwrap().is_some());
        assert_eq!(query::trashed(&db).unwrap().len(), 2);

        node.empty_trash().unwrap();

        assert_eq!(retractions().len(), 1);
        assert!(query::get_event(&db, &mine.id).unwrap().is_none());
    }

    #[test]
    fn restoring_the_users_own_retracts_its_retraction() {
        let db = db();
        let mut node = trash_node(&db);
        let mine = note(author(1), 100, "mine", Tags::new());

        db_command::publish_event(&db, &mine, &author(1), 100).unwrap();
        clock::at(200, || node.trash(&mine.id)).unwrap();
        assert!(query::is_deleted(&db, &mine).unwrap());

        clock::at(300, || node.restore(&mine.id)).unwrap();

        assert!(!query::is_deleted(&db, &mine).unwrap());
        assert!(query::trashed_writing(&db).unwrap().is_empty());
        assert_eq!(query::trashed(&db).unwrap().len(), 1);
    }

    #[test]
    fn what_somebody_else_retracted_cannot_be_put_back() {
        let db = db();
        let mut node = trash_node(&db);
        let theirs = note(author(3), 100, "theirs", Tags::new());
        let deletion = crate::fixtures::event(
            author(3),
            delete::KIND,
            200,
            "",
            Tags::new().add("e", [theirs.id.to_hex()]),
        );

        db_command::receive_event(&db, &theirs, &[author(3)], 100).unwrap();
        db_command::receive_event(&db, &deletion, &[author(3)], 200).unwrap();

        assert_eq!(
            query::trashed_writing(&db).unwrap(),
            vec![(theirs.id, 200, true)]
        );
        assert!(node.restore(&theirs.id).is_err());
        assert_eq!(query::trashed(&db).unwrap().len(), 1);
    }

    #[test]
    fn the_sweep_empties_only_what_has_been_in_the_trash_a_week() {
        let db = db();
        let mut node = clock::at(1_000, || trash_node(&db));
        let old = note(author(3), 100, "a week in", Tags::new());
        let fresh = note(author(3), 100, "just trashed", Tags::new());

        db_command::receive_event(&db, &old, &[author(3)], 1_000).unwrap();
        db_command::receive_event(&db, &fresh, &[author(3)], 1_000).unwrap();
        db_command::set_trashed(&db, &old.id, true, 1_000).unwrap();
        db_command::set_trashed(&db, &fresh.id, true, 1_000 + TRASH_SECONDS).unwrap();

        clock::at(1_000 + TRASH_SECONDS + 1, || node.sweep_events());

        assert!(query::get_event(&db, &old.id).unwrap().is_none());
        assert!(query::get_event(&db, &fresh.id).unwrap().is_some());
    }

    #[test]
    fn restoring_takes_a_thing_back_out_of_the_trash() {
        let db = db();
        let theirs = note(author(3), 100, "changed my mind", Tags::new());

        db_command::receive_event(&db, &theirs, &[author(3)], 100).unwrap();
        assert!(db_command::set_trashed(&db, &theirs.id, true, 200).unwrap());
        assert!(db_command::set_trashed(&db, &theirs.id, false, 300).unwrap());
        assert!(query::trashed(&db).unwrap().is_empty());
    }

    #[test]
    fn writing_that_arrives_in_the_background_is_announced_once_switched_on() {
        let db = db();
        let mut node = trash_node(&db);
        let arrived = |n: u8| note(author(3), 100 + i64::from(n), "on the board", Tags::new());

        db_command::set_preference(&db, keys::NOTIFY_CONTENT, "true", 10).unwrap();
        let card = crate::fixtures::event(
            author(1),
            crate::model::CONTACT,
            50,
            "Ben",
            Tags::new().add("d", [author(3).to_hex()]),
        );
        db_command::publish_event(&db, &card, &author(1), 50).unwrap();
        node.notify_foregrounded();

        db_command::receive_event(&db, &arrived(1), &[author(3)], 100).unwrap();
        let looking = node.tick();
        assert!(
            !looking
                .iter()
                .any(|action| matches!(action, Action::Notify(_)))
        );

        node.notify_backgrounded();
        db_command::receive_event(&db, &arrived(2), &[author(3)], 100).unwrap();
        let pocketed = node.tick();

        // Counted since the user last looked, which was before the second arrived.
        assert!(pocketed.contains(&Action::Notify(Notification::Content {
            count: 1,
            author: Some("Ben".into()),
            excerpt: "on the board".into(),
        })));
    }

    #[test]
    fn blocking_somebody_evicts_what_they_wrote() {
        let db = db();
        let mut node = Node::new(
            Arc::clone(&db),
            custody(secret(1)),
            Arc::new(crate::blobs::MemoryBlobStore::default()),
        )
        .unwrap();
        let theirs = note(author(3), 100, "not welcome", Tags::new());
        db_command::receive_event(&db, &theirs, &[author(3)], 100).unwrap();

        let blocks = crate::fixtures::event(
            author(1),
            crate::model::BLOCK,
            200,
            "",
            Tags::new().add("p", [author(3).to_hex()]),
        );
        node.publish(&blocks, &[]).unwrap();

        assert!(
            crate::db::query::get_event(&db, &theirs.id)
                .unwrap()
                .is_none()
        );
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

        // Reading a session's writes back takes its peer, because they are sealed.
        let policy = Arc::new(Policy::new(author(1)));
        let mut session = Session::open(
            LinkId(2),
            Role::Receiver,
            4096,
            policy,
            custody(secret(1)),
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

        // The peer sends something the wire cannot carry, and this device drops the link.
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

    /// Two contacts' nodes over one link, synchronizing.
    fn contacts_pair() -> (Node, Node) {
        contacts_pair_after(|_, _| {})
    }

    /// Two contacts' nodes over one link, having done `before` to the dialer and the receiver first.
    fn contacts_pair_after(before: impl FnOnce(&mut Node, &mut Node)) -> (Node, Node) {
        let mut dialer = Node::new(
            db(),
            custody(secret(1)),
            Arc::new(crate::blobs::MemoryBlobStore::default()),
        )
        .unwrap();
        let mut receiver = Node::new(
            db(),
            custody(secret(2)),
            Arc::new(crate::blobs::MemoryBlobStore::default()),
        )
        .unwrap();

        for (node, by, named) in [(&mut dialer, 1, 2), (&mut receiver, 2, 1)] {
            let card = crate::fixtures::event(
                author(by),
                CONTACT,
                100,
                "neighbour",
                Tags::new().add("d", [author(named).to_hex()]),
            );

            node.publish(&card, &[]).unwrap();
        }

        before(&mut dialer, &mut receiver);

        receiver
            .link_up(LinkId(1), None, Role::Receiver, 4096)
            .unwrap();

        let opening = dialer
            .link_up(LinkId(1), Some(peripheral(1)), Role::Dialer, 4096)
            .unwrap();
        settle(&mut dialer, &mut receiver, opening);

        assert_eq!(dialer.sessions[&LinkId(1)].state, State::Syncing);
        assert_eq!(receiver.sessions[&LinkId(1)].state, State::Syncing);

        (dialer, receiver)
    }

    /// The image of a post that arrives on a link already syncing is fetched without the link coming up again.
    #[test]
    fn a_picture_posted_mid_session_is_fetched_on_the_same_link() {
        let (mut dialer, mut receiver) = contacts_pair();
        let bytes = b"a picture posted while both phones were already in range";
        let hash = BlobHash::digest(bytes);
        // Stamped now, since the live subscription asks for what is written from here on.
        let post = note(
            author(1),
            clock::now(),
            "look at this",
            Tags::new().add("imeta", Node::media_tags(bytes)),
        );

        let published = dialer.publish(&post, &[bytes]).unwrap();
        settle(&mut dialer, &mut receiver, published);

        assert!(query::get_event(&receiver.db, &post.id).unwrap().is_some());
        assert!(
            query::get_blob(&receiver.db, &hash)
                .unwrap()
                .unwrap()
                .complete
        );
    }

    /// The image of a post written before the two met arrives with it.
    #[test]
    fn a_picture_posted_before_the_link_is_fetched_once_it_syncs() {
        let bytes = b"a picture posted before the other phone came into range";
        let hash = BlobHash::digest(bytes);
        let post = note(
            author(1),
            clock::now() - 3_600,
            "earlier",
            Tags::new().add("imeta", Node::media_tags(bytes)),
        );
        let (_, receiver) = contacts_pair_after(|dialer, _| {
            dialer.publish(&post, &[bytes]).unwrap();
        });

        assert!(query::get_event(&receiver.db, &post.id).unwrap().is_some());
        assert!(
            query::get_blob(&receiver.db, &hash)
                .unwrap()
                .unwrap()
                .complete
        );
    }

    /// A hash the peer could not serve is asked for again once a post it can serve names it.
    #[test]
    fn media_the_peer_could_not_serve_is_fetched_once_it_posts_it() {
        let bytes = b"a picture somebody else named before its author posted it";
        let tags = || Tags::new().add("imeta", Node::media_tags(bytes));
        let hash = BlobHash::digest(bytes);
        let elsewhere = note(author(3), clock::now() - 60, "seen elsewhere", tags());

        let (mut dialer, mut receiver) = contacts_pair_after(|_, receiver| {
            db_command::receive_event(&receiver.db, &elsewhere, &[author(3)], clock::now())
                .unwrap();
        });

        // The dialer has no record of the hash yet, so the first ask was a 404.
        assert!(
            !query::get_blob(&receiver.db, &hash)
                .unwrap()
                .unwrap()
                .complete
        );

        let post = note(author(1), clock::now(), "mine after all", tags());
        let published = dialer.publish(&post, &[bytes]).unwrap();
        settle(&mut dialer, &mut receiver, published);

        assert!(
            query::get_blob(&receiver.db, &hash)
                .unwrap()
                .unwrap()
                .complete
        );
    }

    /// A phone whose clock runs a little behind still has what it writes in range gossiped live.
    #[test]
    fn a_post_from_a_clock_running_behind_still_arrives_live() {
        let (mut dialer, mut receiver) = contacts_pair();
        let post = note(
            author(1),
            clock::now() - 20,
            "written a moment ago",
            Tags::new(),
        );

        let published = dialer.publish(&post, &[]).unwrap();
        settle(&mut dialer, &mut receiver, published);

        assert!(query::get_event(&receiver.db, &post.id).unwrap().is_some());
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
    fn a_phone_that_has_published_turns_an_identity_away_without_asking() {
        let (mut source, mut target) = attended_pair();
        let mine = note(target.identity, 100, "already here", Tags::new());
        target.publish(&mine, &[]).unwrap();

        let offered = source.offer_identity(LinkId(1)).unwrap();
        let actions = settle(&mut source, &mut target, offered);

        // Only the source put its digits up, and it learns the offer went nowhere.
        let prompts = actions
            .iter()
            .filter(|action| matches!(action, Action::ConfirmIdentityTransfer(..)));
        assert_eq!(prompts.count(), 1);
        assert!(actions.contains(&Action::IdentityTransfer(LinkId(1), Outcome::Refused)));
        assert!(!target.sessions[&LinkId(1)].transfer.running());
        assert!(!source.sessions[&LinkId(1)].transfer.running());
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
    fn an_unusable_l2cap_channel_leaves_the_link_on_gatt() {
        let (mut dialer, _receiver) = attended_pair();

        let actions = dialer.l2cap_opened(LinkId(1), 1).unwrap();

        assert!(!actions.contains(&Action::Disconnect(LinkId(1))));
        assert_eq!(dialer.sessions[&LinkId(1)].state, State::Syncing);
    }

    #[test]
    fn an_identity_transfer_needs_a_link() {
        let mut node = node();

        assert!(node.offer_identity(LinkId(1)).is_err());
        assert!(node.answer_identity_transfer(LinkId(1), true).is_err());
        assert!(node.take_transferred_identity(LinkId(1)).is_none());
    }
}
