//! A pair secret and the pubkey it is held against. `docs/discovery.md#recognition`.

use coracle_lib::keys::PublicKey;

/// A pair secret this device holds, and the pubkey it is held against: one
/// row of `pair_secret`.
///
/// A peer that proved several pubkeys has a row for each, all under one
/// secret, which is how recognition resolves a device to every identity it
/// proved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairSecret {
    /// A pubkey the peer proved in a session that synced.
    pub pubkey: PublicKey,
    /// The secret both devices hold for it.
    pub secret: [u8; 32],
}
