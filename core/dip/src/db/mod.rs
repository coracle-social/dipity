//! The database: its tables, its migrations, and the transaction every query
//! and command takes.
//!
//! A [`Db`] is one open store: a SQLite connection, and the channels its writes
//! announce on. It is an instance rather than a global, and every query and
//! command takes one, so a process can hold two stores that share nothing. The
//! shell opens one over the directory it owns at startup ([`Db::open`]) and
//! hands it around; everything after that is core-side.
//!
//! [`query`] and [`command`] are the way in, and they are the only way in: one
//! function per question asked and per thing that happens, each taking the
//! store, opening a transaction on it with [`Db::read`] or [`Db::write`] and
//! threading that through however many tables the answer takes.
//!
//! Below them sits one module per group of tables — [`blob`], [`event`],
//! [`pref`], [`recipient_signature`] — each holding three files:
//!
//! | File | Holds |
//! | --- | --- |
//! | `query.rs` | Read-only free functions over the group's tables |
//! | `command.rs` | Free functions that modify them |
//! | `channel.rs` | The change channel commands announce on |
//!
//! A group owns every table it touches and no table is touched from two of
//! them, so the invariants between tables — an event and its indexes, a blob
//! and the event anchoring it — have exactly one place they can be broken.
//! Every function down here takes a [`Tx`] rather than reaching for the
//! database itself, so several of them compose into one atomic write.
//!
//! The types the answers are expressed in are [`crate::model`]'s, organized by
//! what they mean rather than by which table holds them.
//!
//! The schema itself is SQL, in `core/dip/migrations`, applied in order and
//! recorded in SQLite's `user_version`.

pub mod command;
pub mod query;

pub mod blob;
pub mod event;
pub mod pairing;
pub mod pref;
pub mod recipient_signature;
pub mod spending;

pub(crate) mod channels;
pub(crate) mod condition;
mod core;
pub(crate) mod sql;

pub use self::core::{Db, Tx, migrate};
