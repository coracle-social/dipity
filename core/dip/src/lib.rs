//! Dip's core: everything other than UI and platform bindings.
//!
//! Platform bindings live in `dip-ffi`, kept separate so this crate
//! stays testable on the host with nothing but `cargo test`.
//!
//! # The store
//!
//! A [`Db`](db::Db) is one open store: its SQLite connection, the migrations
//! behind it, and the channels its writes announce on, and every query and
//! command takes one. [`db`] holds one module per group of tables, each
//! carrying that group's reads and its writes, and [`model`] holds the types
//! those answers are expressed in, organized by what they mean rather than by
//! which table holds them.
//!
//! Nostr's own types are `coracle-lib`'s, not this crate's: events, keys, tags,
//! kinds, addresses, filters and NIP-77 items. They have to agree byte for byte
//! with a peer running the other platform's build, so there is one definition
//! of each and it is not here.
//!
//! [`db::query`] and [`db::command`] are the way in, and they are the only way
//! in: one function per question asked and per thing that happens, each opening
//! a transaction and threading it through however many tables the answer takes.
//! Nothing outside them holds a transaction, so a change that spans tables
//! cannot half-happen and a caller cannot assemble an answer in the wrong
//! order.
//!
//! Which peer may be served what is not a second way in either: it rides on the
//! query, as a [`Query`](model::Query) carrying the [`Policy`](model::Policy)
//! the user set and the authorship registers `docs/proofs.md` requires.
//!
//! ```no_run
//! # fn main() -> anyhow::Result<()> {
//! # let incoming: coracle_lib::events::HashedEvent = todo!();
//! # let (peer, identity): (&coracle_lib::keys::PublicKey, &coracle_lib::keys::PublicKey) = todo!();
//! # let signature = None;
//! use coracle_lib::filters::Filter;
//! use dip::db::Db;
//! use dip::model::{Query, Order, Registers};
//!
//! let db = Db::open("/path/the/shell/provides")?;
//!
//! let stored = dip::db::command::receive_event(
//!     &db, &incoming, &[*peer], signature, identity, coracle_lib::util::now(),
//! )?;
//!
//! // The user's feed: by arrival, since a note handed over today is new to
//! // them whatever its author stamped it.
//! let feed = dip::db::query::with_details(&db, dip::db::query::list_events(
//!     &db,
//!     &Query::new()
//!         .with_filter(Filter::new().add_kinds([1]).add_limit(50))
//!         .with_order(Order::SeenAt),
//! )?)?;
//!
//! // What that peer may be handed, which is the same query under the
//! // constraints they are owed.
//! let offerable = dip::db::query::list_events(
//!     &db,
//!     &Query::new()
//!         .with_registers(Registers::offerable(*identity))
//!         .with_policy(dip::db::query::policy(&db, identity)?.for_pubkey(*peer)),
//! )?;
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

pub mod clock;
pub mod db;
pub mod link;
pub mod model;
pub mod node;
pub mod session;
pub mod sync;
pub mod transport;
pub mod util;

pub use link::{LinkId, PeripheralId, Role};
pub use node::{Action, Node};

#[cfg(test)]
pub(crate) mod fixtures;

/// The core's version, as compiled.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_reported() {
        assert!(!super::version().is_empty());
    }
}
