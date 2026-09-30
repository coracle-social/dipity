//! Change notifications for the event tables.
//!
//! Every write to `event`, `event_tag`, `event_fts`, `event_seen` or
//! `event_shared` announces
//! itself here once its transaction commits. Two subscribers matter: the view,
//! which re-reads to stay live, and the session layer, which forwards a stored
//! event to whoever is currently connected — which is what makes gossip
//! immediate rather than waiting for the next reconciliation.

use tokio::sync::broadcast::{self, Receiver, Sender};

use coracle_lib::events::HashedEvent;

use crate::db::{Db, Tx};
use crate::model::Provenance;

/// How far a subscriber may fall behind before it starts missing changes.
///
/// Sized for a sync burst: a peer can hand over hundreds of events in a few
/// seconds, and a subscriber that lags is told it lagged, so it can recover by
/// re-reading rather than by being handed the backlog.
const CAPACITY: usize = 1024;

/// Something that happened to an event.
#[derive(Debug, Clone)]
pub enum EventChange {
    /// An event was stored for the first time. Carries the event, so a
    /// forwarder does not have to read it back to relay it. Boxed because the
    /// other variants are a handful of bytes and each subscriber gets a clone.
    Stored(Box<HashedEvent>),
    /// An event already stored was seen from another peer.
    Seen(Provenance),
    /// An event was handed to a peer it had not been handed to before. Carries
    /// its id, as lowercase hex.
    Shared(String),
    /// An event was removed — superseded, deleted by its author, or forgotten.
    /// Carries its id, as lowercase hex.
    Deleted(String),
}

/// This group's channel, one per store.
pub(crate) fn new() -> Sender<EventChange> {
    broadcast::channel(CAPACITY).0
}

/// Listen for changes to events in `db`.
///
/// A receiver that falls [`CAPACITY`] behind is told so and skipped forward; it
/// has missed changes and should re-read whatever it is tracking.
#[must_use]
pub fn subscribe(db: &Db) -> Receiver<EventChange> {
    db.channels.event.subscribe()
}

/// Announce a change once `tx` commits.
///
/// Deferred rather than sent, so nothing hears about a row a later error rolled
/// back, and so a subscriber that reads on the news finds it there.
pub(crate) fn notify(tx: &Tx<'_>, change: EventChange) {
    let sender = tx.channels.event.clone();

    tx.after_commit(move || {
        // Nobody listening is the normal state during a background wake.
        let _ = sender.send(change);
    });
}
