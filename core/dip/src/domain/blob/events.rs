//! Change notifications for the `blob` table.
//!
//! The transfer layer listens for what to fetch next; the view listens for
//! progress and for the moment an image becomes displayable.

use std::sync::LazyLock;

use tokio::sync::broadcast::{self, Receiver, Sender};

use crate::db::Tx;

use super::model::Blob;

/// How far a subscriber may fall behind. Progress is reported per group of
/// chunks rather than per chunk, so this is not the busy channel it looks like.
const CAPACITY: usize = 256;

/// Something that happened to a blob.
#[derive(Debug, Clone)]
pub enum BlobChange {
    /// A blob was referenced by a stored event for the first time. This is what
    /// puts it on the want list. Boxed because every other variant is a handful
    /// of bytes and each subscriber gets its own clone.
    Recorded(Box<Blob>),
    /// More verified bytes landed, by hash and total held.
    Progressed(String, i64),
    /// Every byte is held and verified.
    Completed(String),
    /// The record went — evicted, or its anchoring event was deleted.
    Removed(String),
}

/// The channel.
static CHANGES: LazyLock<Sender<BlobChange>> = LazyLock::new(|| broadcast::channel(CAPACITY).0);

/// Listen for changes to blobs.
#[must_use]
pub fn subscribe() -> Receiver<BlobChange> {
    CHANGES.subscribe()
}

/// Announce a change once `tx` commits.
pub(crate) fn notify(tx: &Tx<'_>, change: BlobChange) {
    tx.after_commit(move || {
        let _ = CHANGES.send(change);
    });
}
