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
//! its scalar on drop. Nothing holds key bytes between encounters — a
//! [`Node`](crate::Node) and a [`Session`](crate::session::Session) keep the
//! pubkey and this trait, and no more than that.
//!
//! Whoever implements it owns the wipe of whatever they read out of storage on
//! the way here.

use std::cell::OnceCell;

use anyhow::Result;
use coracle_lib::keys::{PublicKey, SecretKey};

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

/// The identity as serving a peer needs it: the pubkey always, and the key only
/// when there is something to sign.
///
/// A batch that only forwards never reads secure storage, because forwarding
/// signs nothing. `docs/proofs.md#hygiene`.
pub trait Signer {
    /// The identity's pubkey, which costs no read.
    fn pubkey(&self) -> PublicKey;

    /// The identity key, read at most once however often it is asked for.
    fn key(&self) -> Result<&SecretKey>;
}

/// One batch's signer over custody: the key is read the first time a signature
/// wants it, kept for the rest of the batch, and dropped with it.
pub struct Batch<'a> {
    /// The identity's pubkey, which the node already holds.
    pubkey: PublicKey,
    /// Where the key is read from, if it is needed.
    custody: &'a dyn KeyCustody,
    /// The key, once read.
    key: OnceCell<SecretKey>,
}

impl<'a> Batch<'a> {
    /// A batch acting as `pubkey`, reading its key out of `custody` on demand.
    #[must_use]
    pub fn new(pubkey: PublicKey, custody: &'a dyn KeyCustody) -> Self {
        Self {
            pubkey,
            custody,
            key: OnceCell::new(),
        }
    }
}

impl Signer for Batch<'_> {
    fn pubkey(&self) -> PublicKey {
        self.pubkey
    }

    fn key(&self) -> Result<&SecretKey> {
        if let Some(key) = self.key.get() {
            return Ok(key);
        }

        let key = self.custody.identity()?;

        Ok(self.key.get_or_init(|| key))
    }
}

/// A key in hand signs as itself.
impl Signer for SecretKey {
    fn pubkey(&self) -> PublicKey {
        self.public_key()
    }

    fn key(&self) -> Result<&SecretKey> {
        Ok(self)
    }
}

/// A key already in hand, which is what the host and `cargo test` have.
///
/// The key was handed over at construction and is answered unchanged, because
/// there is no shell, no Keychain and nothing to read out of.
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
