//! This device answering a peer: the relay half.
//!
//! Everything it has to apply is expressible as a [`Query`]. A peer's `REQ`
//! becomes a filter, the user's Gossip scope and visibility become a
//! [`PubkeyPolicy`](crate::model::PubkeyPolicy), and the two authorship registers
//! that may travel become [`Registers::offerable`]. `db::query::list_events`
//! takes all three, so there is no second place where what a peer may see is
//! decided.

use anyhow::Result;
use coracle_lib::filters::Filter;

use crate::db::Db;
use crate::model::{Query, Registers};
use crate::session::Peer;
use crate::sync::{Message, SubscriptionId};

/// Handle a message this device's relay half received.
pub fn handle(_db: &Db, _peer: &Peer, _message: Message) -> Result<Vec<Message>> {
    todo!("REQ / CLOSE / NEG-OPEN / NEG-MSG / inbound EVENT")
}

/// The query answering `filter` for this peer.
///
/// The peer's filter is the only part of this that came off the wire. The
/// registers and the policy are the user's, and a peer cannot widen either.
#[must_use]
pub fn query_for(peer: &Peer, filter: Filter) -> Query {
    let query = Query::new()
        .with_filter(filter)
        .with_registers(Registers::offerable(peer.identity));

    // Blocked wins over every identity, so any session reaching here may be served.
    match peer.policies.first() {
        Some(policy) => query.with_policy(policy.clone()),
        None => query,
    }
}

/// The set the NIP-77 negentropy pass diffs, bounded by the same query as
/// everything else the relay half serves.
pub fn reconcilable(_db: &Db, _peer: &Peer, _filter: Filter) -> Result<coracle_lib::sync::SyncSet> {
    todo!("list_events under query_for, into a SyncSet")
}

/// Serve one subscription, paginated in reverse chronological order with
/// dynamic since/until windows.
pub fn serve(
    _db: &Db,
    _peer: &Peer,
    _subscription: &SubscriptionId,
    _filters: &[Filter],
) -> Result<Vec<Message>> {
    todo!("paginate list_events, then EOSE")
}
