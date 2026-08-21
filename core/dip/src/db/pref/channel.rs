//! Change notifications for the `pref` table.
//!
//! Policy is read at encounter time, with nobody watching, so the session layer
//! listens here rather than caching a value it might have read before the user
//! changed it.

use tokio::sync::broadcast::{self, Receiver, Sender};

use crate::db::{Db, Tx};
use crate::model::Pref;

/// How far a subscriber may fall behind. Preferences change at human speed.
const CAPACITY: usize = 64;

/// Something that happened to a preference.
#[derive(Debug, Clone)]
pub enum PrefChange {
    /// A preference was written.
    Set(Pref),
    /// A preference was removed, so its default applies again.
    Removed(String),
}

/// This group's channel, one per store.
pub(crate) fn new() -> Sender<PrefChange> {
    broadcast::channel(CAPACITY).0
}

/// Listen for changes to preferences in `db`.
#[must_use]
pub fn subscribe(db: &Db) -> Receiver<PrefChange> {
    db.channels.pref.subscribe()
}

/// Announce a change once `tx` commits.
pub(crate) fn notify(tx: &Tx<'_>, change: PrefChange) {
    let sender = tx.channels.pref.clone();

    tx.after_commit(move || {
        let _ = sender.send(change);
    });
}
