//! What moves between two identified peers. `docs/sync.md`.
//!
//! Every device is both halves of the relay protocol at once, and the two are
//! separate modules because they answer to different rules:
//!
//! | Module | Is | Governed by |
//! | --- | --- | --- |
//! | [`relay`] | what this device serves a peer | Gossip scope and visibility |
//! | [`client`] | what it takes from one | Accept scope and the authorship registers |
//!
//! Neither is a trait: one implementation of each, as free functions over a
//! [`Session`](crate::session::Session) and a [`Db`](crate::db::Db), the same
//! shape as [`crate::db::query`] and [`crate::db::command`].

pub mod blob;
pub mod client;
pub mod message;
pub mod relay;

pub use message::{Message, SubscriptionId};

use crate::db::Db;

/// The app-wide dependencies a sync half needs, bundled so a handler stays
/// under the argument limit: the store and the rolling spending ledger.
pub struct Ctx<'a> {
    /// The store events are read from and written to.
    pub db: &'a Db,
    /// The rolling spending ledger, metered across sessions.
    pub spending: &'a crate::spending::SpendingLedger,
}

/// Per-peer ceilings on what a session may write to this device.
///
/// Accepting gossip is an unbounded write from whoever is standing nearby, so
/// these apply independently of scope: a peer inside the user's trust graph is
/// still metered. `docs/sync.md#quotas`.
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
    /// What a peer the user has never trusted gets.
    pub const STRANGER: Self = Self {
        events: 128,
        bytes: 256 * 1024,
        blob_bytes: 0,
    };

    /// What a peer in the user's trust graph gets.
    pub const TRUSTED: Self = Self {
        events: 4096,
        bytes: 8 * 1024 * 1024,
        blob_bytes: 16 * 1024 * 1024,
    };
}
