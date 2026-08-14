//! Recipient signatures: the `recipient_signature` table.
//!
//! Each row is an author's signature over an event id and a recipient pubkey,
//! held by the peer it names. It is the witness an authorship proof is built
//! from, and it is never served to a peer. See `docs/proofs.md`.

pub mod command;
pub mod events;
pub mod query;
