//! Where an event came from, which never leaves the device.

use coracle_lib::keys::PublicKey;

/// One sighting: an event, a peer it was seen from, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// The event seen, as a lowercase hex id.
    pub event_id: String,
    /// The peer it came from.
    pub peer_pubkey: PublicKey,
    /// When it arrived, by the local clock.
    pub seen_at: i64,
}
