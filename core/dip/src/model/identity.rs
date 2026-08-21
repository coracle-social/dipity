//! The pubkeys a device is acting as.

use std::collections::BTreeSet;

use coracle_lib::keys::PublicKey;

/// The pubkeys a device is acting as. Aliased here for clarity - each peer
/// may authenticate as multiple pubkeys simultaneously.
pub type Identity = BTreeSet<PublicKey>;
