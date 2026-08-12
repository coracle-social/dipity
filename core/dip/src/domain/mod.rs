//! The domain models and the tables behind them.
//!
//! One module per model, each holding four files:
//!
//! | File | Holds |
//! | --- | --- |
//! | `model.rs` | The types, and what they mean |
//! | `query.rs` | Read-only free functions over the model's tables |
//! | `command.rs` | Free functions that modify them |
//! | `events.rs` | The change channel commands announce on |
//!
//! A model owns every table it touches and no table is touched from two models,
//! so the invariants between tables — an event and its indexes, a blob and the
//! event anchoring it — have exactly one place they can be broken.
//!
//! Every query and command takes a [`Tx`](crate::db::Tx) rather than reaching
//! for the database itself, so several of them compose into one atomic write.
//! [`crate::db::query`] and [`crate::db::command`] are where a transaction comes from.

pub mod blob;
pub mod event;
pub mod pref;
pub mod proof;
