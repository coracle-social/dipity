//! The database, its migrations, and the transaction every query and command
//! takes.
//!
//! One SQLite connection for the process, behind a lazy lock that opens the
//! file and runs migrations on first use. The shell hands over the directory at
//! startup ([`configure`]); everything after that is core-side.
//!
//! Transaction scope lives here too, with the connection it locks: [`read`] and
//! [`write`] are the only things that take the lock, and the use cases in
//! [`query`] and [`command`] are the only things that call them. Those two are
//! the way in — one function per question asked and per thing that happens,
//! each threading a transaction through however many [domains](crate::domain)
//! the answer takes.
//!
//! The schema itself is SQL, in `core/dip/migrations`, applied in order and
//! recorded in SQLite's `user_version`.

pub mod command;
pub mod query;

pub(crate) mod condition;
mod core;
pub(crate) mod sql;

pub use self::core::{Tx, configure, migrate, open_in_memory};
pub(crate) use self::core::{read, write};
