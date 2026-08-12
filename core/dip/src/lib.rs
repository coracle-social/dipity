//! Dip's core: everything other than UI and platform bindings.
//!
//! Platform bindings live in `dip-ffi`, kept separate so this crate
//! stays testable on the host with nothing but `cargo test`.
//!
//! # The store
//!
//! [`db`] owns one SQLite connection for the process and the migrations behind
//! it. [`domain`] holds the models, one module per model, each with its tables,
//! its reads, its writes and the channel its writes announce on.
//!
//! Nostr's own types are `coracle-lib`'s, not this crate's: events, keys, tags,
//! kinds, addresses, filters and NIP-77 items. They have to agree byte for byte
//! with a peer running the other platform's build, so there is one definition
//! of each and it is not here. `coracle_lib::util::now()` is the clock.
//!
//! [`db::query`] and [`db::command`] are the way in, and they are the only way
//! in: one function per question asked and per thing that happens, each opening
//! a transaction and threading it through however many domains the answer
//! takes. Nothing outside them holds a transaction, so a change that spans
//! models cannot half-happen and a caller cannot assemble an answer in the
//! wrong order.
//!
//! Which peer may be served what is not a second way in either: it rides on the
//! query, as an [`EventFilter`](domain::event::model::EventFilter) carrying the
//! [`Policy`](domain::pref::model::Policy) the user set and the authorship
//! registers `docs/proofs.md` requires.
//!
//! ```no_run
//! # fn main() -> anyhow::Result<()> {
//! # let incoming: coracle_lib::events::HashedEvent = todo!();
//! # let (peer, identity): (&coracle_lib::keys::PublicKey, &coracle_lib::keys::PublicKey) = todo!();
//! # let signature = None;
//! use coracle_lib::filters::Filter;
//! use dip::domain::event::model::{EventFilter, Order, Registers};
//!
//! dip::db::configure("/path/the/shell/provides")?;
//!
//! let stored = dip::db::command::receive_event(
//!     &incoming, peer, signature, identity, coracle_lib::util::now(),
//! )?;
//!
//! // The user's feed: by arrival, since a note handed over today is new to
//! // them whatever its author stamped it.
//! let feed = dip::db::query::with_details(dip::db::query::list_events(
//!     &EventFilter::new()
//!         .with_filter(Filter::new().add_kinds([1]).add_limit(50))
//!         .with_order(Order::SeenAt),
//! )?)?;
//!
//! // What that peer may be handed, which is the same query under the
//! // constraints they are owed.
//! let offerable = dip::db::query::list_events(
//!     &EventFilter::new()
//!         .with_registers(Registers::offerable(*identity))
//!         .with_policy(dip::db::query::policy(identity)?.for_peer(*peer)),
//! )?;
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

pub mod db;
pub mod domain;
pub mod util;

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
