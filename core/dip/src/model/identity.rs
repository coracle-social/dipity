//! The identities this device is acting as.

use std::collections::BTreeSet;

use coracle_lib::keys::PublicKey;

/// The pubkeys this device authenticates as, and that both authorship
/// registers are measured from.
///
/// One key today, because custody is one key today. The wire does not require
/// that — NIP-42 lets either side prove a sequence, and [`Peer`] already holds
/// whatever the far side proved — so the asymmetry is a custody decision
/// rather than a protocol one, and this type is where it lives.
///
/// Should a device ever carry several, note what does and does not multiply.
/// Serving does not: the trust graph names people, so what a peer may be shown
/// is one question about one device, and the registers simply widen to "any
/// identity of ours". Attribution does: a recipient signature names one
/// recipient and a proof designates one verifier, so those are minted per pair.
///
/// [`Peer`]: crate::session::Peer
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalIdentity {
    /// Every pubkey this device is acting as. Never empty.
    pubkeys: BTreeSet<PublicKey>,
}

impl LocalIdentity {
    /// A device acting as one pubkey.
    #[must_use]
    pub fn new(pubkey: PublicKey) -> Self {
        Self {
            pubkeys: [pubkey].into_iter().collect(),
        }
    }

    /// Every pubkey this device is acting as.
    pub fn pubkeys(&self) -> impl Iterator<Item = &PublicKey> {
        self.pubkeys.iter()
    }

    /// The pubkey to sign an event with, and to name when a peer needs to be
    /// told who this is.
    ///
    /// Panics is not the failure mode here: the set is never empty, because
    /// there is nothing to construct one from but a key.
    #[must_use]
    pub fn signing(&self) -> PublicKey {
        *self
            .pubkeys
            .iter()
            .next()
            .expect("a local identity holds at least one pubkey")
    }

    /// Whether `pubkey` is one this device is acting as.
    #[must_use]
    pub fn owns(&self, pubkey: &PublicKey) -> bool {
        self.pubkeys.contains(pubkey)
    }
}

impl From<PublicKey> for LocalIdentity {
    fn from(pubkey: PublicKey) -> Self {
        Self::new(pubkey)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::author;

    #[test]
    fn an_identity_owns_the_key_it_was_built_from_and_no_other() {
        let identity = LocalIdentity::new(author(1));

        assert!(identity.owns(&author(1)));
        assert!(!identity.owns(&author(2)));
        assert_eq!(identity.signing(), author(1));
        assert_eq!(identity.pubkeys().count(), 1);
    }
}
