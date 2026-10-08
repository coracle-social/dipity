//! Change notifications for the `blob` table.
//!
//! The transfer layer listens for what to fetch next; the view listens for
//! progress and for the moment an image becomes displayable; the node listens
//! for [`BlobChange::Removed`] and deletes the bytes.

use tokio::sync::broadcast::{self, Receiver, Sender};

use crate::db::{Db, Tx};
use crate::model::{Blob, BlobHash};

/// How far a subscriber may fall behind. This is not the busy channel it looks
/// like, because progress is reported per group of chunks rather than per chunk.
const CAPACITY: usize = 256;

/// Something that happened to a blob.
#[derive(Debug, Clone)]
pub enum BlobChange {
    /// A blob was referenced by a stored event for the first time. This is what
    /// puts it on the want list. Boxed because every other variant is a handful
    /// of bytes and each subscriber gets its own clone.
    Recorded(Box<Blob>),
    /// More bytes landed, by hash and total held.
    Progressed(BlobHash, u64),
    /// Every byte is held and hashes to its address.
    Completed(BlobHash),
    /// The record went — evicted, or the last event referencing it was
    /// deleted. The bytes go with it, and a subscriber that misses this is why
    /// [`Node`](crate::node::Node) also sweeps.
    Removed(BlobHash),
}

/// This group's channel, one per store.
pub(crate) fn new() -> Sender<BlobChange> {
    broadcast::channel(CAPACITY).0
}

/// Listen for changes to blobs in `db`.
#[must_use]
pub fn subscribe(db: &Db) -> Receiver<BlobChange> {
    db.channels.blob.subscribe()
}

/// Announce a change once `tx` commits.
pub(crate) fn notify(tx: &Tx<'_>, change: BlobChange) {
    let sender = tx.channels.blob.clone();

    tx.after_commit(move || {
        let _ = sender.send(change);
    });
}
