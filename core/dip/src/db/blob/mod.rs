//! Blobs: the `blob` table.
//!
//! Metadata for the files events reference, keyed by SHA-256, along with how
//! much of each one this device holds. The bytes live outside the database in
//! the blob directory the shell provides. See `docs/sync.md`.

pub mod channel;
pub mod command;
pub mod query;
