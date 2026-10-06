//! What moves between two identified peers. `docs/sync.md`.
//!
//! Every device is both halves of the relay protocol at once, and the two are
//! separate modules because they answer to different rules:
//!
//! | Module | Is | Governed by |
//! | --- | --- | --- |
//! | [`relay`] | what this device serves a peer | Sharing, and relaying only to contacts |
//! | [`client`] | what it takes from one | Accept scope and the authorship registers |
//!
//! Neither is a trait: one [`Relay`](relay::Relay) and one
//! [`Client`](client::Client) per session, each owning its half's state with
//! a single `handle` entry point over the same [`Db`](crate::db::Db).

pub mod blob;
pub mod client;
pub mod message;
pub mod relay;
pub mod spending;

pub use message::{Message, SubscriptionId};

/// Per-peer ceilings on what a session may write to this device.
///
/// Accepting gossip is an unbounded write from whoever is standing nearby, so
/// these apply independently of scope: a contact is
/// still metered, and what it has spent against these is [`spending`].
/// `docs/sync.md#quotas`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quota {
    /// Events this session may store.
    pub events: u32,
    /// Bytes of events it may store.
    pub bytes: u64,
    /// Bytes of blobs it may store.
    pub blob_bytes: u64,
}

impl Quota {
    /// What a peer who is not a contact gets.
    pub const STRANGER: Self = Self {
        events: 128,
        bytes: 256 * 1024,
        blob_bytes: 0,
    };

    /// What every stranger together may write in the window.
    ///
    /// The per-peer stranger budget bounds one pubkey, and a pubkey is free:
    /// content events carry no signature, so a fresh keypair per encounter
    /// costs an attacker nothing and resets their meter. This is the ceiling
    /// that does not reset, because it is not keyed on identity at all.
    /// `docs/sync.md#quotas`.
    ///
    /// Eight strangers' worth. A crowd passes; a beacon camped in one does not
    /// keep taking all day.
    pub const STRANGER_POOL: Self = Self {
        events: 8 * Self::STRANGER.events,
        bytes: 8 * Self::STRANGER.bytes,
        blob_bytes: 0,
    };

    /// What a contact gets.
    pub const CONTACT: Self = Self {
        events: 4096,
        bytes: 8 * 1024 * 1024,
        blob_bytes: 16 * 1024 * 1024,
    };
}
