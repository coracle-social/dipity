//! Pairing: the `pair_secret` and `disclosure` tables.
//!
//! A pair secret is derived from a completed session's handshake hash and
//! stored against the peer, so that a later encounter is recognized from its
//! tags before either side names a pubkey. The disclosure table is the budget: how
//! many times this device has handed its identity to an unrecognized peer, per
//! window.
//! `docs/discovery.md#recognition`, `docs/policy.md#discoverability`.

pub mod command;
pub mod query;
