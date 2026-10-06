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

/// Which notifications the user switched on, each off until they say so.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NotificationPrefs {
    /// Somebody nearby wants to pair.
    pub pairing: bool,
    /// New writing arrived.
    pub content: bool,
}

/// The keys defined for user preferences.
pub mod keys {
    /// Whether to notify, in the background, that somebody nearby wants to pair.
    pub const NOTIFY_PAIRING: &str = "notifications.pairing";

    /// Whether to notify, in the background, that new writing arrived.
    pub const NOTIFY_CONTENT: &str = "notifications.content";

    /// Times of day, in the device's local timezone, when no stranger is told who the user is.
    pub const QUIET_TIMES: &str = "policy.quiet_times";

    /// Whether a stranger is told who the user is while the app is not in front. Default on.
    pub const DISCOVER_IN_BACKGROUND: &str = "policy.discover_in_background";

    /// Who is handed the user's own activity, and who may carry it further. Default `anyone`.
    pub const SHARING: &str = "policy.sharing";

    /// Whose events the device stores from a peer. Default `lenient`.
    pub const ACCEPT: &str = "policy.accept";

    /// How long a carried event stays after it first reaches this device. Default 90 days.
    pub const RETENTION_DAYS: &str = "policy.retention_days";
}
