//! Change notifications for the `proof` table.
//!
//! Holding a signature is what lets an event travel its second hop, so the
//! forwarding side listens here: an event that could not be relayed to anyone
//! becomes relayable the moment its signature arrives.

use std::sync::LazyLock;

use coracle_lib::keys::PublicKey;
use tokio::sync::broadcast::{self, Receiver, Sender};

use crate::db::Tx;
use crate::model::Proof;

/// How far a subscriber may fall behind. Signatures arrive one per event per
/// recipient, so this tracks the event channel's traffic at a lower rate.
const CAPACITY: usize = 256;

/// Something that happened to a stored signature.
#[derive(Debug, Clone)]
pub enum ProofChange {
    /// A signature was stored.
    Stored(Box<Proof>),
    /// A signature was removed, by event id and recipient.
    Removed(String, PublicKey),
}

/// The channel.
static CHANGES: LazyLock<Sender<ProofChange>> = LazyLock::new(|| broadcast::channel(CAPACITY).0);

/// Listen for changes to stored signatures.
#[must_use]
pub fn subscribe() -> Receiver<ProofChange> {
    CHANGES.subscribe()
}

/// Announce a change once `tx` commits.
pub(crate) fn notify(tx: &Tx<'_>, change: ProofChange) {
    tx.after_commit(move || {
        let _ = CHANGES.send(change);
    });
}
