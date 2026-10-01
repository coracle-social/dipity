//! The quota ledger: the `spending` table.
//!
//! What each peer wrote to this device inside the rolling window, so the meters
//! survive a relaunch. [`crate::sync::spending`] reads it once when it opens
//! and writes every charge through. `docs/sync.md#quotas`.

pub mod command;
pub mod query;
