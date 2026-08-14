//! The event kinds the core reads meaning from.
//!
//! Every other kind is stored, queried and served without being interpreted, so
//! it needs no name here.

/// Kind 0: the author's profile.
pub const KIND_PROFILE: u16 = 0;

/// Kind 5: a request to delete the events its `e` and `a` tags name.
pub const KIND_DELETE: u16 = 5;

/// Kind 10000: NIP-51's mute list.
pub const KIND_MUTE: u16 = 10_000;
