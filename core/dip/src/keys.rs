//! Where the identity key lives, which is never here.
//!
//! `docs/keys.md#key-custody` puts the nostr key in platform secure storage —
//! Keychain on iOS, a Keystore-wrapped key on Android — and has the core read
//! it out when a signature is needed. So the core declares what it needs and
//! the shell implements it, like [`BlobStore`](crate::blobs::BlobStore) and
//! for the same reason: a platform capability is a trait pointing down, never
//! an import pointing up.
//!
//! # Read for one use
//!
//! [`KeyCustody::identity`] is called at the moment a signature is wanted, and
//! what it answers is dropped at the end of that expression. `SecretKey` zeroes
//! its scalar on drop, so nothing holds key bytes between encounters — a
//! [`Node`](crate::Node) and a [`Session`](crate::session::Session) keep the
//! pubkey and this trait, and no more than that.
//!
//! Whoever implements it owns the wipe of whatever they read out of storage on
//! the way here.

use anyhow::Result;
use coracle_lib::keys::SecretKey;

/// The identity key, read out of wherever the platform keeps it.
///
/// One method, because reading is the only thing the core does with it.
/// Generating, importing and deleting an identity are the shell's, and happen
/// with a user in front of the phone rather than during a background wake.
pub trait KeyCustody: Send + Sync {
    /// Read the identity out for one use.
    ///
    /// An error is a device that cannot sign, which is a device that cannot
    /// authenticate — so it fails the link rather than degrading it to an
    /// unauthenticated one.
    fn identity(&self) -> Result<SecretKey>;
}

/// A key already in hand, which is what the host and `cargo test` have.
///
/// No shell, no Keychain and nothing to read out of, so the key was handed over
/// at construction and is answered unchanged.
impl KeyCustody for SecretKey {
    fn identity(&self) -> Result<SecretKey> {
        Ok(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::KeyCustody;
    use coracle_lib::keys::SecretKey;

    #[test]
    fn a_key_in_hand_is_its_own_custody() {
        let key = SecretKey::generate();
        let custody: Arc<dyn KeyCustody> = Arc::new(key.clone());

        assert_eq!(custody.identity().unwrap().to_hex(), key.to_hex());
    }
}
