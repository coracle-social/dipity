//! The radio loop, as the shell drives it.
//!
//! [`dip::Node`] is the whole gossip stack behind one object: radio events in,
//! [`Action`]s out, no I/O and no callbacks. This exports it, and the export is
//! deliberately one to one — every entry point the core declares is a method
//! here, and every action it emits is a variant, because a shell that cannot
//! answer an action is a subsystem that silently does nothing.
//!
//! # The lock
//!
//! The core takes `&mut self` and uniffi hands out `Arc`, so the mutex lives
//! here. Each method takes it, runs one entry point, and drops it before the
//! actions are converted. Two callbacks can still run under it: the logger, and
//! a read of the identity key when a session signs mid-call, which is one
//! Keychain or Keystore read. Anything slower, the backup's scrypt above all,
//! runs with the lock released.
//!
//! # The vocabulary
//!
//! [`LinkId`], [`PeripheralId`] and [`Role`] cross as themselves. They are
//! one-field records rather than a `u64` and a `String`, so Swift and Kotlin get
//! types a call site cannot swap — which matters most at
//! [`Node::link_up`](Node::link_up), where both appear.

use std::sync::{Arc, Mutex};

use coracle_lib::events::HashedEvent;
use dip::keys::KeyCustody as CoreCustody;
use dip::session::transfer::Outcome;
use dip::{
    Action as CoreAction, LinkId as CoreLinkId, Notification as CoreNotification,
    PeripheralId as CorePeripheralId,
};

use crate::keys::{Custody, KeyCustody};
use crate::store::Store;

/// One GATT connection, from the moment it is up until it is torn down.
///
/// The shell assigns it and the core keys sessions on it: a link exists before
/// anyone is identified, and a peer may authenticate as several pubkeys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct LinkId {
    /// The shell's number for this connection.
    pub value: u64,
}

/// A BLE peripheral, as the platform names it.
///
/// Opaque and per-app, and stable only until the peer's BLE address rotates.
/// Nothing durable may be derived from it — `docs/discovery.md`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PeripheralId {
    /// CoreBluetooth's identifier, or Android's device address.
    pub value: String,
}

/// Which side dialed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Role {
    /// This device connected to the peer.
    Dialer,
    /// The peer connected to this device.
    Receiver,
}

/// How an identity transfer ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TransferOutcome {
    /// The key arrived. Take it with [`Node::take_transferred_identity`].
    Received,
    /// The key left this device, so the peer holds this identity too.
    Sent,
    /// One of the two users said no, or the peer could not run the flow.
    Refused,
}

/// Something the shell does on the core's behalf.
///
/// Every entry point answers with a list of these and the core calls nothing
/// back, which is what keeps a lock off a foreign call. An action the shell
/// drops is a subsystem that stops working with no error anywhere.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum Action {
    /// Start or stop scanning for our service UUID.
    Scan {
        /// Whether to be scanning.
        on: bool,
    },
    /// Start or stop advertising it.
    Advertise {
        /// Whether to be advertising.
        on: bool,
    },
    /// Dial a peripheral the scheduler admitted.
    Connect {
        /// The peripheral to dial.
        peripheral: PeripheralId,
    },
    /// Tear a link down.
    Disconnect {
        /// The link to drop.
        link: LinkId,
    },
    /// Write one fragment, already sized to the link's MTU.
    ///
    /// Report the acknowledged ATT write with [`Node::write_complete`], which
    /// releases the next.
    Send {
        /// The link to write on.
        link: LinkId,
        /// The fragment, header included.
        fragment: Vec<u8>,
    },
    /// Ask the user whether an unadmitted stranger may connect.
    ///
    /// Answer with [`Node::approve`]; the link is held up to the drain cap
    /// meanwhile.
    RequestApproval {
        /// The link waiting on the answer.
        link: LinkId,
        /// What the two users compare before either of them answers.
        code: u32,
    },
    /// Who the peer on a link proved to be, once per pubkey it proves.
    ///
    /// The gate runs before either side names a pubkey, so a pet name the user
    /// typed there is for a person. This says which key that person holds.
    PeerIdentified {
        /// The link the peer proved itself over.
        link: LinkId,
        /// The pubkey it proved, hex.
        pubkey: String,
        /// The value both users compare before naming the peer.
        code: u32,
        /// Whether this device dialed the link.
        dialed: bool,
    },
    /// Present the share sheet over a key backup the core has written.
    ///
    /// The path is the shell's, not the view's: the view learns only that the
    /// file was shared or dismissed. `docs/keys.md#backup`.
    ShareKeyBackup {
        /// Where the file is.
        path: String,
    },
    /// Write one bulk fragment to this link's L2CAP channel, length-prefixed
    /// because the channel is a byte stream.
    ///
    /// Answered with [`Node::bulk_write_complete`], separately from GATT.
    SendBulk {
        /// The link whose L2CAP channel to write on.
        link: LinkId,
        /// The fragment, length prefix included.
        fragment: Vec<u8>,
    },
    /// Publish an L2CAP channel and report its PSM with
    /// [`Node::l2cap_published`], or [`Node::l2cap_unavailable`] if the platform
    /// will not.
    PublishL2cap {
        /// The link to publish for.
        link: LinkId,
    },
    /// Open the L2CAP channel the peer published at this PSM, and report the
    /// result with [`Node::l2cap_opened`] or [`Node::l2cap_unavailable`].
    OpenL2cap {
        /// The link to open on.
        link: LinkId,
        /// The PSM the peer published at.
        psm: u16,
    },
    /// Show the user this six-digit comparison value and ask whether the other
    /// device shows the same one.
    ///
    /// Both ends are asked and either may answer first. Answer with
    /// [`Node::answer_identity_transfer`]. `docs/keys.md#login-with-device`.
    ConfirmIdentityTransfer {
        /// The link the transfer is running on.
        link: LinkId,
        /// The six digits to put on screen.
        code: u32,
    },
    /// How an identity transfer ended.
    ///
    /// On [`TransferOutcome::Received`] take the key with
    /// [`Node::take_transferred_identity`], write it to secure storage, and
    /// reopen the node under it.
    IdentityTransfer {
        /// The link the transfer ran on.
        link: LinkId,
        /// What happened.
        outcome: TransferOutcome,
    },
    /// Post a notification, which the core raises only while the app is in
    /// the background and only for what the user switched on.
    Notify {
        /// What to tell the user.
        announcement: Announcement,
    },
    /// Call [`Node::tick`] at or after this unix second.
    ///
    /// Advisory: iOS runs no timer for a suspended app, so the heartbeat and the
    /// connection scheduler both recover on the next radio callback.
    WakeAt {
        /// The unix second to wake at.
        at: i64,
    },
}

/// Something worth interrupting a user who is not looking at the app.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum Announcement {
    /// Somebody the user has not named is in range and can be paired with.
    Pairing,
    /// New writing has arrived since the user last opened the app.
    Content {
        /// How many posts.
        count: u32,
        /// The user's name for whoever wrote the latest, if they named them.
        author: Option<String>,
        /// The start of the latest post, or its title.
        excerpt: String,
    },
}

impl From<CoreNotification> for Announcement {
    fn from(notification: CoreNotification) -> Self {
        match notification {
            CoreNotification::Pairing => Self::Pairing,
            CoreNotification::Content {
                count,
                author,
                excerpt,
            } => Self::Content {
                count,
                author,
                excerpt,
            },
        }
    }
}

/// Why an entry point could not finish.
///
/// Two cases, because they are two different things for the shell to do. A
/// `Link` failure is a link the shell disconnects — the core will not carry on
/// with it, and leaving it up holds one of six central slots against a session
/// that is going nowhere. A `Core` failure implicates no link.
#[derive(Debug, uniffi::Error)]
pub enum NodeError {
    /// This link cannot carry on. Disconnect it.
    Link {
        /// The link to drop.
        link: LinkId,
        /// What went wrong, for the log.
        reason: String,
    },
    /// The core could not do what was asked, and no link is implicated.
    Core {
        /// What went wrong, for the log.
        reason: String,
    },
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Link { link, reason } => write!(f, "link {} failed: {reason}", link.value),
            Self::Core { reason } => write!(f, "the core failed: {reason}"),
        }
    }
}

impl std::error::Error for NodeError {}

impl NodeError {
    /// A failure the shell answers by dropping `link`.
    fn link(link: LinkId, error: &anyhow::Error) -> Self {
        Self::Link {
            link,
            reason: format!("{error:#}"),
        }
    }

    /// A failure with no link behind it.
    fn core(error: &anyhow::Error) -> Self {
        Self::Core {
            reason: format!("{error:#}"),
        }
    }

    /// The poisoned-mutex case, which is a previous call having panicked.
    fn poisoned() -> Self {
        Self::Core {
            reason: "the core panicked on an earlier call and cannot be used".to_owned(),
        }
    }
}

/// The core, as the shell holds it.
#[derive(uniffi::Object)]
pub struct Node {
    /// The gossip stack. Locked per call and never across one.
    inner: Mutex<dip::Node>,
}

/// Take the lock, run one entry point, and answer with the actions it returned.
///
/// The guard is dropped before the conversion, so nothing crossing back into
/// Swift or Kotlin happens under it.
macro_rules! drive {
    ($self:expr, |$node:ident| $call:expr) => {{
        let actions = {
            let mut guard = $self.inner.lock().map_err(|_| NodeError::poisoned())?;
            let $node = &mut *guard;

            $call
        };

        Ok(actions.into_iter().map(Action::from).collect())
    }};
}

#[uniffi::export]
impl Node {
    /// Open a node over a store the shell has already opened.
    ///
    /// `directory` is where blob bytes go, under `directory/blobs`, and is the
    /// shell's choice on both platforms. `custody` is the Keychain or Keystore
    /// the identity key is read out of.
    #[uniffi::constructor]
    pub fn open(
        store: Arc<Store>,
        custody: Arc<dyn KeyCustody>,
        directory: String,
    ) -> Result<Arc<Self>, NodeError> {
        let custody: Arc<dyn CoreCustody> = Arc::new(Custody::new(custody));
        let node = dip::Node::open(Arc::clone(&store.db), custody, directory)
            .map_err(|error| NodeError::core(&error))?;

        Ok(Arc::new(Self {
            inner: Mutex::new(node),
        }))
    }

    /// This device's pubkey, hex-encoded.
    pub fn identity(&self) -> Result<String, NodeError> {
        let guard = self.inner.lock().map_err(|_| NodeError::poisoned())?;

        Ok(guard.identity().to_hex())
    }

    // --------------------------------------------------------- Radio events

    /// A peripheral advertising our service UUID was seen.
    ///
    /// Whether to dial is the core's call; the shell keeps the radio.
    pub fn peripheral_seen(
        &self,
        peripheral: PeripheralId,
        rssi: i16,
    ) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node.peripheral_seen(&peripheral.into(), rssi))
    }

    /// A dial failed before a link came up. The core retries it shortly.
    pub fn dial_failed(&self, peripheral: PeripheralId) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node.dial_failed(&peripheral.into()))
    }

    /// A GATT connection came up, with the MTU the link negotiated.
    ///
    /// `peripheral` is the one this device dialed; a link the peer dialed has
    /// none.
    pub fn link_up(
        &self,
        link: LinkId,
        peripheral: Option<PeripheralId>,
        role: Role,
        mtu: u32,
    ) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node
            .link_up(
                link.into(),
                peripheral.map(Into::into),
                role.into(),
                mtu as usize,
            )
            .map_err(|error| NodeError::link(link, &error))?)
    }

    /// A link went away.
    pub fn link_down(&self, link: LinkId) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node.link_down(link.into()))
    }

    /// One write arrived off the characteristic.
    pub fn bytes_received(&self, link: LinkId, write: Vec<u8>) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node.bytes_received(link.into(), &write))
    }

    /// A read arrived off this link's L2CAP channel.
    pub fn bulk_received(&self, link: LinkId, read: Vec<u8>) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node.bulk_received(link.into(), &read))
    }

    /// The ATT write for the last [`Action::Send`] on this link was
    /// acknowledged, which releases the next fragment.
    pub fn write_complete(&self, link: LinkId) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node.write_complete(link.into()))
    }

    /// The last [`Action::SendBulk`] on this link went out.
    pub fn bulk_write_complete(&self, link: LinkId) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node.bulk_write_complete(link.into()))
    }

    /// This device published an L2CAP channel at `psm`, as
    /// [`Action::PublishL2cap`] asked.
    pub fn l2cap_published(&self, link: LinkId, psm: u16) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node
            .l2cap_published(link.into(), psm)
            .map_err(|error| NodeError::link(link, &error))?)
    }

    /// The channel [`Action::OpenL2cap`] named is open, at this MTU.
    pub fn l2cap_opened(&self, link: LinkId, mtu: u32) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node
            .l2cap_opened(link.into(), mtu as usize)
            .map_err(|error| NodeError::link(link, &error))?)
    }

    /// This end cannot do L2CAP on this link, so it stays on GATT.
    pub fn l2cap_unavailable(&self, link: LinkId) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node
            .l2cap_unavailable(link.into())
            .map_err(|error| NodeError::link(link, &error))?)
    }

    // -------------------------------------------------------------- The app

    /// Run whatever the clock has made due: heartbeats, drains, and the next
    /// dial the scheduler will allow.
    pub fn tick(&self) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node.tick())
    }

    /// The battery level, percent. Blob transfers are metered against it, so
    /// with no report the core is deciding on missing information.
    pub fn battery(&self, level: u8) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node.battery(level))
    }

    /// The app came to the front, which opens the gate's cool-off window.
    pub fn notify_foregrounded(&self) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node.notify_foregrounded())
    }

    /// The app went to the back.
    pub fn notify_backgrounded(&self) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node.notify_backgrounded())
    }

    /// The user answered [`Action::RequestApproval`].
    pub fn approve(&self, link: LinkId, approved: bool) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node
            .approve(link.into(), approved)
            .map_err(|error| NodeError::link(link, &error))?)
    }

    /// Everything the user has said about who gets what, as JSON, with the
    /// core's own defaults where nothing is written.
    ///
    /// The compiled policy every live session is bound to, so the settings
    /// screen edits what the gossip path obeys rather than a second reading of
    /// the same preference keys. `docs/policy.md`.
    pub fn policy(&self) -> Result<String, NodeError> {
        let guard = self.inner.lock().map_err(|_| NodeError::poisoned())?;

        serde_json::to_string(guard.policy()).map_err(|error| NodeError::Core {
            reason: format!("the policy could not be encoded: {error}"),
        })
    }

    /// The user's preferences changed, so every live session is rebound under
    /// the policy they compile to.
    pub fn policy_changed(&self) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node
            .policy_changed()
            .map_err(|error| NodeError::core(&error))?)
    }

    /// Delete everything in the trash: the user's own events are retracted,
    /// and anybody else's dropped from this device.
    pub fn empty_trash(&self) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node
            .empty_trash()
            .map_err(|error| NodeError::core(&error))?)
    }

    /// Store and offer an event this device authored, with the media it
    /// attaches.
    ///
    /// `event` is NIP-01 JSON with an id and no signature, built by the view —
    /// a tag has to be in the event before its id can compute, so composing one
    /// lives above the core.
    pub fn publish(&self, event: String, media: Vec<Vec<u8>>) -> Result<Vec<Action>, NodeError> {
        let event: HashedEvent = serde_json::from_str(&event).map_err(|error| NodeError::Core {
            reason: format!("that is not a hashed event: {error}"),
        })?;
        let media: Vec<&[u8]> = media.iter().map(Vec::as_slice).collect();

        drive!(self, |node| node
            .publish(&event, &media)
            .map_err(|error| NodeError::core(&error))?)
    }

    // ------------------------------------------------------------- Identity

    /// Offer this device's identity to the peer on `link`.
    ///
    /// `docs/keys.md#login-with-device`. Both users confirm before anything
    /// moves.
    pub fn offer_identity(&self, link: LinkId) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node
            .offer_identity(link.into())
            .map_err(|error| NodeError::link(link, &error))?)
    }

    /// The user answered [`Action::ConfirmIdentityTransfer`].
    pub fn answer_identity_transfer(
        &self,
        link: LinkId,
        confirmed: bool,
    ) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node
            .answer_identity_transfer(link.into(), confirmed)
            .map_err(|error| NodeError::link(link, &error))?)
    }

    /// Take the identity [`TransferOutcome::Received`] announced, once.
    ///
    /// Answers its 32 secret bytes for the shell to write to secure storage,
    /// after which the node is reopened under the new key.
    pub fn take_transferred_identity(&self, link: LinkId) -> Result<Option<Vec<u8>>, NodeError> {
        let mut guard = self.inner.lock().map_err(|_| NodeError::poisoned())?;
        let key = guard.take_transferred_identity(link.into());

        Ok(key.as_ref().map(crate::keys::secret_bytes))
    }

    /// Write a key backup into `cache` and ask for the share sheet over it.
    ///
    /// `password` encrypts it as a NIP-49 `ncryptsec`; without one the file
    /// carries a plain `nsec`. Either way the key is encoded in the core and
    /// the caller is handed no part of it.
    ///
    /// The key is read and the file written without the lock: scrypt takes
    /// long enough that holding it would stall every radio event behind it.
    pub fn export_key(
        &self,
        cache: String,
        password: Option<String>,
    ) -> Result<Vec<Action>, NodeError> {
        let custody = self
            .inner
            .lock()
            .map_err(|_| NodeError::poisoned())?
            .custody();
        let identity = custody
            .identity()
            .map_err(|error| NodeError::core(&error))?;
        let path = dip::backup::write(&identity, std::path::Path::new(&cache), password.as_deref())
            .map_err(|error| NodeError::core(&error))?;

        drive!(self, |node| node.key_backup_written(path))
    }

    /// The share sheet closed, shared or dismissed, so the file goes.
    pub fn key_export_finished(&self) -> Result<Vec<Action>, NodeError> {
        drive!(self, |node| node
            .key_export_finished()
            .map_err(|error| NodeError::core(&error))?)
    }
}

/// The primary service, which is the whole BLE advertisement.
///
/// Read rather than hard-coded in Swift and Kotlin: an iPhone and an Android
/// phone have to name the same one, and two copies is one place to disagree.
#[uniffi::export]
#[must_use]
pub fn service_uuid() -> String {
    dip::transport::SERVICE_UUID.to_owned()
}

/// The one characteristic on that service.
#[uniffi::export]
#[must_use]
pub fn characteristic_uuid() -> String {
    dip::transport::CHARACTERISTIC_UUID.to_owned()
}

/// The name the view knows a transfer's ending by.
///
/// Read rather than spelled in Swift and Kotlin, the same way
/// [`change_name`](crate::store::change_name) is: the three endings mean three
/// different screens and the view switches on this string.
#[uniffi::export]
#[must_use]
pub fn outcome_name(outcome: TransferOutcome) -> String {
    match outcome {
        TransferOutcome::Received => "received",
        TransferOutcome::Sent => "sent",
        TransferOutcome::Refused => "refused",
    }
    .to_owned()
}

/// The `imeta` entries an event has to carry for a peer to fetch `bytes` and
/// check what it gets.
///
/// A free function because it reads nothing: the view composes the tag and the
/// event, and the core is handed the result.
#[uniffi::export]
#[must_use]
pub fn media_tags(bytes: Vec<u8>) -> Vec<String> {
    dip::Node::media_tags(&bytes)
}

impl From<LinkId> for CoreLinkId {
    fn from(link: LinkId) -> Self {
        Self(link.value)
    }
}

impl From<CoreLinkId> for LinkId {
    fn from(link: CoreLinkId) -> Self {
        Self { value: link.0 }
    }
}

impl From<PeripheralId> for CorePeripheralId {
    fn from(peripheral: PeripheralId) -> Self {
        Self(peripheral.value)
    }
}

impl From<CorePeripheralId> for PeripheralId {
    fn from(peripheral: CorePeripheralId) -> Self {
        Self {
            value: peripheral.0,
        }
    }
}

impl From<Role> for dip::Role {
    fn from(role: Role) -> Self {
        match role {
            Role::Dialer => Self::Dialer,
            Role::Receiver => Self::Receiver,
        }
    }
}

impl From<Outcome> for TransferOutcome {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Received => Self::Received,
            Outcome::Sent => Self::Sent,
            Outcome::Refused => Self::Refused,
        }
    }
}

impl From<CoreAction> for Action {
    fn from(action: CoreAction) -> Self {
        match action {
            CoreAction::Scan(on) => Self::Scan { on },
            CoreAction::Advertise(on) => Self::Advertise { on },
            CoreAction::Connect(peripheral) => Self::Connect {
                peripheral: peripheral.into(),
            },
            CoreAction::Disconnect(link) => Self::Disconnect { link: link.into() },
            CoreAction::Send(link, fragment) => Self::Send {
                link: link.into(),
                fragment,
            },
            CoreAction::RequestApproval(link, code) => Self::RequestApproval {
                link: link.into(),
                code,
            },
            CoreAction::PeerIdentified(link, pubkey, code, dialed) => Self::PeerIdentified {
                link: link.into(),
                pubkey: pubkey.to_hex(),
                code,
                dialed,
            },
            CoreAction::ShareKeyBackup(path) => Self::ShareKeyBackup {
                path: path.to_string_lossy().into_owned(),
            },
            CoreAction::SendBulk(link, fragment) => Self::SendBulk {
                link: link.into(),
                fragment,
            },
            CoreAction::PublishL2cap(link) => Self::PublishL2cap { link: link.into() },
            CoreAction::OpenL2cap(link, psm) => Self::OpenL2cap {
                link: link.into(),
                psm,
            },
            CoreAction::ConfirmIdentityTransfer(link, code) => Self::ConfirmIdentityTransfer {
                link: link.into(),
                code,
            },
            CoreAction::IdentityTransfer(link, outcome) => Self::IdentityTransfer {
                link: link.into(),
                outcome: outcome.into(),
            },
            CoreAction::Notify(notification) => Self::Notify {
                announcement: notification.into(),
            },
            CoreAction::WakeAt(at) => Self::WakeAt { at },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use crate::keys::KeyError;

    /// A directory that goes when the test does.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("dip-ffi-{name}-{}", std::process::id()));

            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();

            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A key the shell is holding, as the Keychain would.
    struct Held(coracle_lib::keys::SecretKey);

    impl KeyCustody for Held {
        fn secret_key(&self) -> Result<Vec<u8>, KeyError> {
            Ok(crate::keys::secret_bytes(&self.0))
        }
    }

    fn node(dir: &TempDir) -> Arc<Node> {
        let store = Store::open(dir.0.to_string_lossy().into_owned()).unwrap();
        let custody = Arc::new(Held(coracle_lib::keys::SecretKey::generate()));

        Node::open(store, custody, dir.0.to_string_lossy().into_owned()).unwrap()
    }

    #[test]
    fn a_node_opens_over_a_store_the_shell_already_has() {
        let dir = TempDir::new("open");
        let node = node(&dir);

        assert_eq!(node.identity().unwrap().len(), 64);
    }

    #[test]
    fn the_policy_crosses_with_its_defaults_already_filled_in() {
        let dir = TempDir::new("policy");
        let node = node(&dir);

        let crossed: serde_json::Value = serde_json::from_str(&node.policy().unwrap()).unwrap();

        // Nothing is written, so this is the core's own defaults and not an empty document.
        assert_eq!(crossed["accept"], "lenient");
        assert_eq!(crossed["retention_days"], 30);
        assert_eq!(crossed["visibility"]["rules"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_peripheral_the_scheduler_admits_comes_back_as_the_same_peripheral() {
        let dir = TempDir::new("seen");
        let node = node(&dir);
        let peripheral = PeripheralId {
            value: "CB-1".to_owned(),
        };

        let actions = node.peripheral_seen(peripheral.clone(), -40).unwrap();

        assert_eq!(actions, vec![Action::Connect { peripheral }]);
    }

    #[test]
    fn a_link_that_cannot_carry_on_names_itself_in_the_error() {
        let dir = TempDir::new("link");
        let node = node(&dir);
        let link = LinkId { value: 7 };

        node.link_up(link, None, Role::Receiver, 512).unwrap();

        // A link the shell reports up twice has nowhere for the second to go.
        let Err(NodeError::Link { link: named, .. }) =
            node.link_up(link, None, Role::Receiver, 512)
        else {
            panic!("expected the link to be named");
        };

        assert_eq!(named, link);
    }

    #[test]
    fn an_event_that_is_not_one_is_refused_without_a_link_behind_it() {
        let dir = TempDir::new("publish");
        let node = node(&dir);

        assert!(matches!(
            node.publish("not an event".to_owned(), Vec::new()),
            Err(NodeError::Core { .. })
        ));
    }

    #[test]
    fn every_ending_a_transfer_has_is_named_apart_from_the_others() {
        let named = [
            TransferOutcome::Received,
            TransferOutcome::Sent,
            TransferOutcome::Refused,
        ]
        .map(outcome_name);

        assert_eq!(named, ["received", "sent", "refused"]);
        assert_eq!(
            named
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            named.len()
        );
    }

    #[test]
    fn every_action_the_core_emits_crosses_as_itself() {
        let link = dip::LinkId(3);
        let peripheral = dip::PeripheralId("CB-2".to_owned());
        let proved = coracle_lib::keys::SecretKey::generate().public_key();

        let crossed: Vec<Action> = vec![
            CoreAction::Scan(true),
            CoreAction::Advertise(false),
            CoreAction::Connect(peripheral.clone()),
            CoreAction::Disconnect(link),
            CoreAction::Send(link, vec![1, 2]),
            CoreAction::RequestApproval(link, 7_654_321),
            CoreAction::PeerIdentified(link, proved, 1_234, true),
            CoreAction::ShareKeyBackup(PathBuf::from("/cache/dip-key.txt")),
            CoreAction::SendBulk(link, vec![3]),
            CoreAction::PublishL2cap(link),
            CoreAction::OpenL2cap(link, 129),
            CoreAction::ConfirmIdentityTransfer(link, 123_456),
            CoreAction::IdentityTransfer(link, Outcome::Received),
            CoreAction::WakeAt(1_700_000_000),
        ]
        .into_iter()
        .map(Action::from)
        .collect();

        let link = LinkId { value: 3 };
        let peripheral = PeripheralId {
            value: "CB-2".to_owned(),
        };

        assert_eq!(
            crossed,
            vec![
                Action::Scan { on: true },
                Action::Advertise { on: false },
                Action::Connect { peripheral },
                Action::Disconnect { link },
                Action::Send {
                    link,
                    fragment: vec![1, 2]
                },
                Action::RequestApproval {
                    link,
                    code: 7_654_321
                },
                Action::PeerIdentified {
                    link,
                    pubkey: proved.to_hex(),
                    code: 1_234,
                    dialed: true
                },
                Action::ShareKeyBackup {
                    path: "/cache/dip-key.txt".to_owned()
                },
                Action::SendBulk {
                    link,
                    fragment: vec![3]
                },
                Action::PublishL2cap { link },
                Action::OpenL2cap { link, psm: 129 },
                Action::ConfirmIdentityTransfer {
                    link,
                    code: 123_456
                },
                Action::IdentityTransfer {
                    link,
                    outcome: TransferOutcome::Received
                },
                Action::WakeAt { at: 1_700_000_000 },
            ]
        );
    }
}
