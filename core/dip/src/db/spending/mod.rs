//! Spending: the `spending` table, which the rolling 24 h quota sums.
//!
//! One row per event accepted from a peer, keyed by the pubkey it was seen
//! from, so a reconnect cannot refill the budget the per-session counter
//! already spent. `docs/sync.md#quotas`.

pub mod command;
pub mod query;
