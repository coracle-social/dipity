//! One entry in the quota ledger. `docs/sync.md#quotas`.

use coracle_lib::keys::PublicKey;

/// Which budget a charge is against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Meter {
    /// Events a peer wrote to this device.
    Event,
    /// Blob bytes this device took from a peer.
    Blob,
}

impl Meter {
    /// The meter as the `spending` table writes it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Blob => "blob",
        }
    }
}

/// Something one peer wrote to this device, counted against its budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Charge {
    /// Who it is charged to.
    pub pubkey: PublicKey,
    /// Which budget.
    pub meter: Meter,
    /// Whether it also counts against the pool every untrusted peer shares.
    pub pooled: bool,
    /// When, in seconds.
    pub at: i64,
    /// How many bytes.
    pub bytes: u64,
}
