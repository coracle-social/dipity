//! The comparison value two users read off their screens to prove that nothing
//! sits between them.
//!
//! Noise XX authenticates nobody on its own, so both of dip's rituals end in two
//! people comparing something: the consent gate before either side has named a
//! pubkey (`docs/discovery.md#the-consent-gate`), and login with device before
//! the key moves (`docs/keys.md#login-with-device`). A machine in the middle
//! completes two handshakes and holds two transcripts, so the two devices show
//! different values.
//!
//! The label is what stops one ritual's value standing in for the other's, and
//! `space` is how many values the screen it lands on can actually draw.

use sha2::{Digest, Sha256};

/// Domain separation for login with device, whose screen shows six digits.
pub const LOGIN_LABEL: &[u8] = b"dip/login-with-device/sas";

/// How many values the login prompt can show: six decimal digits, as Bluetooth
/// numeric comparison uses.
pub const LOGIN_SPACE: u32 = 1_000_000;

/// Domain separation for the consent gate.
pub const PAIRING_LABEL: &[u8] = b"dip/pairing/sas";

/// How many values the pairing gate can show: five shapes drawn from an
/// alphabet of eight in three tints.
///
/// The screen draws the whole space rather than a prefix of it, so a wider one
/// here shows the user nothing and a narrower one throws entropy away.
pub const PAIRING_SPACE: u32 = 24 * 24 * 24 * 24 * 24;

/// The value both devices display, derived from the transcript they share.
#[must_use]
pub fn sas(label: &[u8], handshake_hash: &[u8; 32], space: u32) -> u32 {
    let mut digest = Sha256::new();
    digest.update(label);
    digest.update(handshake_hash);

    let bytes: [u8; 4] = digest.finalize()[..4].try_into().expect("four bytes");

    u32::from_be_bytes(bytes) % space
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comparison_value_fits_the_screen_that_draws_it() {
        for seed in 0..64u8 {
            assert!(sas(LOGIN_LABEL, &[seed; 32], LOGIN_SPACE) < LOGIN_SPACE);
            assert!(sas(PAIRING_LABEL, &[seed; 32], PAIRING_SPACE) < PAIRING_SPACE);
        }
    }

    #[test]
    fn two_transcripts_disagree() {
        assert_ne!(
            sas(LOGIN_LABEL, &[1; 32], LOGIN_SPACE),
            sas(LOGIN_LABEL, &[2; 32], LOGIN_SPACE)
        );
    }

    #[test]
    fn two_rituals_over_one_transcript_disagree() {
        assert_ne!(
            sas(LOGIN_LABEL, &[1; 32], LOGIN_SPACE),
            sas(PAIRING_LABEL, &[1; 32], LOGIN_SPACE)
        );
    }
}
