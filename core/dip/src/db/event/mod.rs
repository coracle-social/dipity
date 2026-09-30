//! Events: the store's centre, and the five tables that make one queryable.
//!
//! `event` holds the event, `event_tag` and `event_fts` index it, `event_seen`
//! records every peer it has been seen from and `event_shared` every peer it
//! has been handed to. The last two are provenance, which never leaves the
//! device.
//!
//! The event type is `coracle-lib`'s
//! [`HashedEvent`](coracle_lib::events::HashedEvent) — see [`crate::model`] for
//! why that stage and not [`Event`](coracle_lib::events::Event).

pub mod channel;
pub mod command;
pub mod query;
