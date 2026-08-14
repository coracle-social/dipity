//! Noise XX over the link: Curve25519 / ChaCha20-Poly1305 / SHA-256.
//!
//! The static key is generated per handshake. No device has a long-term Noise
//! identity, so nothing the handshake establishes outlives the session and a
//! stranger cannot collect a durable identifier by dialing. The nostr identity
//! is bound to this channel by mutual NIP-42 instead, for the life of one
//! session. `docs/transport.md#the-static-key-is-generated-per-session`.

use anyhow::Result;

use crate::link::Role;

/// The Noise session for one link.
pub struct Noise {
    /// Whether the handshake has completed and traffic is encrypted.
    complete: bool,
}

impl Noise {
    /// Begin a handshake, generating a fresh static key for it.
    ///
    /// The dialer is the Noise initiator.
    #[must_use]
    pub fn begin(_role: Role) -> Self {
        Self { complete: false }
    }

    /// Whether the handshake has completed.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Take a handshake message from the peer, and produce the reply the
    /// pattern calls for.
    ///
    /// `Ok(None)` means the handshake completed on this message and there is
    /// nothing further to send.
    pub fn read_handshake(&mut self, _message: &[u8]) -> Result<Option<Vec<u8>>> {
        todo!("snow handshake state")
    }

    /// The first handshake message, which the dialer sends on connect.
    pub fn first_handshake_message(&mut self) -> Result<Vec<u8>> {
        todo!("snow handshake state")
    }

    /// The handshake hash: a transcript binding of everything both sides sent.
    ///
    /// Recognition tags are MACs over this, so a tag is worthless outside the
    /// session it was minted in. `docs/discovery.md#recognition`.
    pub fn handshake_hash(&self) -> Result<[u8; 32]> {
        todo!("snow handshake state")
    }

    /// Encrypt one frame's payload.
    pub fn encrypt(&mut self, _payload: &[u8]) -> Result<Vec<u8>> {
        todo!("snow transport state")
    }

    /// Decrypt one frame's payload.
    pub fn decrypt(&mut self, _ciphertext: &[u8]) -> Result<Vec<u8>> {
        todo!("snow transport state")
    }
}
