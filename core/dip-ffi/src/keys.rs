//! The Keychain and the Keystore, as the core reaches them.
//!
//! [`dip::keys`] says why the identity key is a trait the shell implements
//! rather than a constructor argument. This is that trait as a uniffi callback
//! interface, plus the adapter between the two: the shell answers 32 bytes it
//! has just read out of secure storage, and [`Custody`] turns them into a
//! `SecretKey` and wipes everything it touched on the way.
//!
//! The bytes are not a hex string, because a `String` handed over from Swift or
//! Kotlin is a copy nothing on this side can find to wipe. What crosses is a
//! byte array the shell can zero as soon as the call returns.

use std::fmt;
use std::sync::Arc;

use coracle_lib::keys::SecretKey;
use zeroize::Zeroize;

/// Why the identity could not be read.
///
/// Two cases, because they are two different things for the shell to do:
/// there is no identity yet, which is first run, or there is one and it did not
/// come back, which is a device that cannot take part until it does.
#[derive(Debug, uniffi::Error)]
pub enum KeyError {
    /// Secure storage holds no identity yet.
    Missing,
    /// Secure storage refused, or what it held was not a key.
    Unreadable {
        /// What the platform said, for the log.
        reason: String,
    },
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("secure storage holds no identity"),
            Self::Unreadable { reason } => write!(f, "the identity could not be read: {reason}"),
        }
    }
}

impl std::error::Error for KeyError {}

/// The platform's secure storage, as the core reads it.
///
/// Implemented in Swift over the Keychain and in Kotlin over a Keystore-wrapped
/// key. `docs/keys.md#signing-happens-in-the-background` fixes the accessibility
/// class on each: readable while the device is locked, and never behind a
/// biometric gate, because the calls below happen during a background wake with
/// nobody in front of the phone.
#[uniffi::export(with_foreign)]
pub trait KeyCustody: Send + Sync {
    /// Read the identity out of secure storage, as its 32 secret bytes.
    fn secret_key(&self) -> Result<Vec<u8>, KeyError>;
}

/// The exported callback, as [`dip::keys::KeyCustody`].
pub struct Custody(Arc<dyn KeyCustody>);

impl Custody {
    /// Wrap what the shell registered.
    pub fn new(custody: Arc<dyn KeyCustody>) -> Self {
        Self(custody)
    }
}

impl dip::keys::KeyCustody for Custody {
    fn identity(&self) -> anyhow::Result<SecretKey> {
        let mut bytes = self.0.secret_key()?;
        let mut hex = hex::encode(&bytes);
        let key = SecretKey::from_hex(&hex);

        bytes.zeroize();
        hex.zeroize();

        Ok(key?)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use coracle_lib::keys::SecretKey;
    use dip::keys::KeyCustody as _;

    use super::{Custody, KeyCustody, KeyError};

    struct Stored(Vec<u8>);

    impl KeyCustody for Stored {
        fn secret_key(&self) -> Result<Vec<u8>, KeyError> {
            Ok(self.0.clone())
        }
    }

    struct Empty;

    impl KeyCustody for Empty {
        fn secret_key(&self) -> Result<Vec<u8>, KeyError> {
            Err(KeyError::Missing)
        }
    }

    #[test]
    fn the_shells_bytes_become_the_identity_the_core_signs_with() {
        let key = SecretKey::generate();
        let custody = Custody::new(Arc::new(Stored(hex::decode(key.to_hex()).unwrap())));

        assert_eq!(custody.identity().unwrap().to_hex(), key.to_hex());
    }

    #[test]
    fn a_first_run_reads_as_an_error_rather_than_as_a_key() {
        assert!(Custody::new(Arc::new(Empty)).identity().is_err());
    }

    #[test]
    fn bytes_that_are_not_a_key_are_refused() {
        let custody = Custody::new(Arc::new(Stored(vec![0; 16])));

        assert!(custody.identity().is_err());
    }
}
