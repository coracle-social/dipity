//! One stored preference, and the keys the core reads back.

use serde::{Deserialize, Serialize};

/// One stored preference.
///
/// Values are JSON documents, so a preference can grow from a scalar into a
/// structure without a migration, and so the view and the core agree on what a
/// value means without a per-key encoding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pref {
    /// The key, dotted by area.
    pub key: String,
    /// The value, as a JSON document.
    pub value: String,
    /// When it was last written.
    pub updated_at: i64,
}

/// The keys defined for user preferences.
pub mod keys {
    /// How long the device keeps accepting unknown peers after the app is backgrounded.
    pub const COOL_OFF_MINUTES: &str = "policy.cool_off_minutes";

    /// Times of day, in the device's local timezone, when the user is passively discoverable.
    pub const DISCOVERABLE_TIMES: &str = "policy.discoverable_times";

    /// How many new pubkeys the device will disclose to per discoverable window.
    pub const DISCLOSURE_BUDGET: &str = "policy.disclosure_budget";

    /// Who can see what the user publishes, as ordered rules and a default.
    pub const VISIBILITY: &str = "policy.visibility";

    /// Whose events the device stores from a peer. Default `lenient`.
    pub const ACCEPT: &str = "policy.accept";

    /// Whose events the device relays onward. Default `network`.
    pub const GOSSIP: &str = "policy.gossip";

    /// Which peers may be handed the signature that lets them forward the
    /// user's events one more hop.
    pub const FORWARD: &str = "policy.forward";

    /// How long a carried event outlives the last peer to hand it over. Default 30 days.
    pub const RETENTION_DAYS: &str = "policy.retention_days";
}
