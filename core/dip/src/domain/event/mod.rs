//! Events: the store's centre, and the four tables that make one queryable.
//!
//! `event` holds the event, `event_tag` and `event_fts` index it, and
//! `event_seen` records every peer it has been seen from. The last of those is
//! provenance, which never leaves the device.
//!
//! The event type is `coracle-lib`'s
//! [`HashedEvent`](coracle_lib::events::HashedEvent) — see [`model`] for why
//! that stage and not [`Event`](coracle_lib::events::Event).

pub mod command;
pub mod events;
pub mod model;
pub mod query;

#[cfg(test)]
pub(crate) mod fixtures;
