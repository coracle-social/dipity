//! Every change channel a store carries, one per group of tables.
//!
//! Held by the [`Db`](super::Db) rather than by statics, so a subscriber hears
//! the writes of the store it subscribed to and no other's.
//!
//! Each group owns its change type and how far a subscriber may fall behind.

use tokio::sync::broadcast::Sender;

use crate::db::blob::channel::BlobChange;
use crate::db::event::channel::EventChange;
use crate::db::pref::channel::PrefChange;
use crate::db::recipient_signature::channel::RecipientSignatureChange;

/// The senders, one per group of tables.
pub(crate) struct Channels {
    /// Changes to the `blob` table.
    pub(crate) blob: Sender<BlobChange>,
    /// Changes to the event tables.
    pub(crate) event: Sender<EventChange>,
    /// Changes to the `pref` table.
    pub(crate) pref: Sender<PrefChange>,
    /// Changes to the `recipient_signature` table.
    pub(crate) recipient_signature: Sender<RecipientSignatureChange>,
}

impl Default for Channels {
    fn default() -> Self {
        Self {
            blob: crate::db::blob::channel::new(),
            event: crate::db::event::channel::new(),
            pref: crate::db::pref::channel::new(),
            recipient_signature: crate::db::recipient_signature::channel::new(),
        }
    }
}
