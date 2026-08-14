//! Change notifications for the `recipient_signature` table.
//!
//! Holding a signature is what lets an event travel its second hop, so the
//! forwarding side listens here: an event that could not be relayed to anyone
//! becomes relayable the moment its signature arrives.

use coracle_lib::events::EventId;
use coracle_lib::keys::PublicKey;
use tokio::sync::broadcast::{self, Receiver, Sender};

use crate::db::{Db, Tx};
use crate::model::RecipientSignature;

/// How far a subscriber may fall behind. Signatures arrive one per event per
/// recipient, so this tracks the event channel's traffic at a lower rate.
const CAPACITY: usize = 256;

/// Something that happened to a stored signature.
#[derive(Debug, Clone)]
pub enum RecipientSignatureChange {
    /// A signature was stored.
    Stored(Box<RecipientSignature>),
    /// A signature was removed, by event id and recipient.
    Removed(EventId, PublicKey),
}

/// This group's channel, one per store.
pub(crate) fn new() -> Sender<RecipientSignatureChange> {
    broadcast::channel(CAPACITY).0
}

/// Listen for changes to stored signatures in `db`.
#[must_use]
pub fn subscribe(db: &Db) -> Receiver<RecipientSignatureChange> {
    db.channels().recipient_signature.subscribe()
}

/// Announce a change once `tx` commits.
pub(crate) fn notify(tx: &Tx<'_>, change: RecipientSignatureChange) {
    let sender = tx.channels().recipient_signature.clone();

    tx.after_commit(move || {
        let _ = sender.send(change);
    });
}
