//! Preferences: the `pref` table.
//!
//! A key/value store for app policy and UI settings. Policy lives here because
//! it is read during a background wake, with no user to prompt and no view
//! running to compute anything. See `docs/policy.md`.

pub mod channel;
pub mod command;
pub mod query;
