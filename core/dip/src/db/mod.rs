//! The database: its tables, its migrations, and the transaction every query
//! and command takes.
//!
//! One SQLite connection for the process, behind a lazy lock that opens the
//! file and runs migrations on first use. The shell hands over the directory at
//! startup ([`configure`]); everything after that is core-side.
//!
//! [`query`] and [`command`] are the way in, and they are the only way in: one
//! function per question asked and per thing that happens, each opening a
//! transaction with [`read`] or [`write`] and threading it through however many
//! tables the answer takes.
//!
//! Below them sits one module per group of tables — [`blob`], [`event`],
//! [`pref`], [`proof`] — each holding three files:
//!
//! | File | Holds |
//! | --- | --- |
//! | `query.rs` | Read-only free functions over the group's tables |
//! | `command.rs` | Free functions that modify them |
//! | `events.rs` | The change channel commands announce on |
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
pub mod pref;
pub mod proof;

pub(crate) mod condition;
mod core;
pub(crate) mod sql;

pub use self::core::{Tx, configure, migrate, open_in_memory};
pub(crate) use self::core::{read, write};
