//! The names the core and the shell use for a link.

/// A BLE peripheral, as the platform names it.
///
/// Opaque and per-app: CoreBluetooth hands out its own identifier rather than a
/// MAC address, and it is stable only until the peer's BLE address rotates.
/// Nothing durable may be derived from it — see `docs/discovery.md`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeripheralId(pub String);

/// One GATT connection, from the moment it is up until it is torn down.
///
/// The core keys sessions on this rather than on a pubkey, because a link
/// exists before anyone is identified and a peer may authenticate as several
/// pubkeys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinkId(pub u64);

/// Which side dialed.
///
/// It decides who speaks first at every step of `docs/discovery.md`: the dialer
/// sends its recognition tags first, and identifies itself first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// This device connected to the peer.
    Dialer,
    /// The peer connected to this device.
    Receiver,
}
