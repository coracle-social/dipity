//! Noise XX over the link: Curve25519 / ChaCha20-Poly1305 / SHA-256.
//!
//! A wrapper over `snow`'s handshake and transport states. The static key is
//! generated per handshake. No device has a long-term Noise identity, so
//! nothing the handshake establishes outlives the session and a stranger
//! cannot collect a durable identifier by dialing. The nostr identity is
//! bound to this channel by mutual NIP-42 instead, for the life of one
//! session. `docs/transport.md#the-static-key-is-generated-per-session`.

use anyhow::{Context, Result, bail};
use snow::{Builder, HandshakeState, TransportState};

use crate::link::Role;

/// The pattern's parameters.
const PARAMS: &str = "Noise_XX_25519_ChaChaPoly_SHA256";

/// The Noise session for one link, either mid-handshake or in transport mode.
pub struct Noise {
    /// Which way the handshake is driven.
    role: Role,
    /// The local static public key, for the `noise://` authority in the AUTH
    /// event's relay tag. `docs/nips/p2p-auth.md`.
    local_static: [u8; 32],
    /// The peer's static public key, captured at completion. The peer's AUTH
    /// responses name this as the relay tag.
    remote_static: Option<[u8; 32]>,
    /// The handshake hash, taken at completion because snow gives access to it
    /// only on the handshake state.
    hash: Option<[u8; 32]>,
    /// The snow handshake state, until it completes.
    handshake: Option<HandshakeState>,
    /// The snow transport state, once it has.
    transport: Option<TransportState>,
}

impl Noise {
    /// The ChaCha20-Poly1305 tag every sealed payload carries, which the chunk
    /// capacity has to leave room for.
    pub const TAG: usize = 16;

    /// Begin a handshake, generating a fresh static key for it.
    ///
    /// The dialer is the Noise initiator. Everything here can fail on a device
    /// whose entropy source is not answering, and a handshake begins in a
    /// background wake with no one to see a panic, so it is reported instead.
    pub fn begin(role: Role) -> Result<Self> {
        let params = PARAMS
            .parse::<snow::params::NoiseParams>()
            .expect("a fixed, valid parameter set");
        let keypair = Builder::new(params.clone())
            .generate_keypair()
            .context("generating a static key for the handshake")?;
        let local_static: [u8; 32] = keypair
            .public
            .as_slice()
            .try_into()
            .context("a Curve25519 public key that is not 32 bytes")?;
        let builder = Builder::new(params)
            .local_private_key(&keypair.private)
            .context("building over the generated static key")?;

        let handshake = if role == Role::Dialer {
            builder.build_initiator()
        } else {
            builder.build_responder()
        }
        .context("building the handshake state")?;

        Ok(Self {
            role,
            local_static,
            remote_static: None,
            hash: None,
            handshake: Some(handshake),
            transport: None,
        })
    }

    /// The local static public key, for the `noise://` authority in the AUTH
    /// event's relay tag. `docs/nips/p2p-auth.md`.
    #[must_use]
    pub fn local_static_key(&self) -> [u8; 32] {
        self.local_static
    }

    /// Whether the handshake has completed and traffic is encrypted.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.transport.is_some()
    }

    /// Take a handshake message from the peer, and produce the reply the
    /// pattern calls for.
    ///
    /// `Ok(None)` means the handshake completed on this message and there is
    /// nothing further to send.
    pub fn read_handshake(&mut self, message: &[u8]) -> Result<Option<Vec<u8>>> {
        let Some(handshake) = &mut self.handshake else {
            bail!("the handshake is over");
        };

        let mut payload = vec![0u8; message.len()];
        handshake.read_message(message, &mut payload)?;

        let reply = if handshake.is_handshake_finished() {
            None
        } else {
            let mut out = vec![0u8; u16::MAX as usize + 1];
            let written = handshake.write_message(&[], &mut out)?;

            Some(out[..written].to_vec())
        };

        // On the initiator's reply the post-write check is what fires; on the
        // responder's final read the post-read check already did.
        if handshake.is_handshake_finished() {
            self.finish()?;
        }

        Ok(reply)
    }

    /// The first handshake message, which the dialer sends on connect.
    pub fn first_handshake_message(&mut self) -> Result<Vec<u8>> {
        if self.role != Role::Dialer {
            bail!("the receiver speaks in response");
        }

        let Some(handshake) = &mut self.handshake else {
            bail!("the handshake is over");
        };

        let mut out = vec![0u8; u16::MAX as usize + 1];
        let written = handshake.write_message(&[], &mut out)?;

        if handshake.is_handshake_finished() {
            self.finish()?;
        }

        Ok(out[..written].to_vec())
    }

    /// Move the completed handshake into transport mode, capturing the hash
    /// before the handshake state moves.
    fn finish(&mut self) -> Result<()> {
        let Some(handshake) = self.handshake.take() else {
            return Ok(());
        };

        self.hash = Some(
            handshake
                .get_handshake_hash()
                .try_into()
                .expect("a 32-byte hash"),
        );
        self.remote_static = handshake.get_remote_static().map(|bytes| {
            let mut key = [0u8; 32];
            key.copy_from_slice(bytes);

            key
        });
        self.transport = Some(handshake.into_transport_mode()?);

        Ok(())
    }

    /// The peer's static public key, once the handshake has completed.
    #[must_use]
    pub fn remote_static_key(&self) -> Option<[u8; 32]> {
        self.remote_static
    }

    /// The handshake hash: a transcript binding of everything both sides sent.
    ///
    /// Recognition tags are MACs over this, so a tag is worthless outside the
    /// session it was minted in. `docs/discovery.md#recognition`.
    pub fn handshake_hash(&self) -> Result<[u8; 32]> {
        if let Some(hash) = self.hash {
            return Ok(hash);
        }

        bail!("the hash exists only once the handshake has completed")
    }

    /// Encrypt one fragment's payload.
    ///
    /// The cipher is a strict sequence — snow steps a nonce per call and keeps
    /// no window — so calls have to happen in the order the writes go out.
    pub fn encrypt(&mut self, payload: &[u8]) -> Result<Vec<u8>> {
        let Some(transport) = &mut self.transport else {
            bail!("traffic encrypts only once the handshake has completed");
        };

        // One AEAD tag over the payload.
        let mut out = vec![0u8; payload.len() + Self::TAG];
        let written = transport.write_message(payload, &mut out)?;

        Ok(out[..written].to_vec())
    }

    /// Decrypt one fragment's payload, in the order the peer sealed it.
    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let Some(transport) = &mut self.transport else {
            bail!("traffic decrypts only once the handshake has completed");
        };

        let mut out = vec![0u8; ciphertext.len()];
        let written = transport.read_message(ciphertext, &mut out)?;

        Ok(out[..written].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run a full XX exchange between a dialer and a receiver.
    fn complete(dialer: &mut Noise, receiver: &mut Noise) {
        let message = dialer.first_handshake_message().unwrap();
        let reply = receiver.read_handshake(&message).unwrap().unwrap();
        let reply = dialer.read_handshake(&reply).unwrap().unwrap();
        receiver.read_handshake(&reply).unwrap();

        assert!(dialer.is_complete());
        assert!(receiver.is_complete());
    }

    #[test]
    fn the_handshake_hash_is_a_transcript_binding_both_sides_see() {
        let mut dialer = Noise::begin(Role::Dialer).unwrap();
        let mut receiver = Noise::begin(Role::Receiver).unwrap();

        complete(&mut dialer, &mut receiver);

        assert_eq!(
            dialer.handshake_hash().unwrap(),
            receiver.handshake_hash().unwrap()
        );
    }

    #[test]
    fn both_directions_encrypt_and_decrypt() {
        let mut dialer = Noise::begin(Role::Dialer).unwrap();
        let mut receiver = Noise::begin(Role::Receiver).unwrap();

        complete(&mut dialer, &mut receiver);

        let forward = dialer.encrypt(b"toward the receiver").unwrap();
        let backward = receiver.encrypt(b"toward the dialer").unwrap();

        assert_eq!(receiver.decrypt(&forward).unwrap(), b"toward the receiver");
        assert_eq!(dialer.decrypt(&backward).unwrap(), b"toward the dialer");
    }

    #[test]
    fn only_the_dialer_speaks_first() {
        let mut receiver = Noise::begin(Role::Receiver).unwrap();

        assert!(receiver.first_handshake_message().is_err());
    }

    #[test]
    fn nothing_encrypts_before_the_handshake_completes() {
        let mut dialer = Noise::begin(Role::Dialer).unwrap();

        assert!(dialer.encrypt(b"early").is_err());
        assert!(dialer.handshake_hash().is_err());
    }

    #[test]
    fn a_reply_after_completion_is_an_error() {
        let mut dialer = Noise::begin(Role::Dialer).unwrap();
        let mut receiver = Noise::begin(Role::Receiver).unwrap();

        complete(&mut dialer, &mut receiver);

        assert!(receiver.read_handshake(&[1, 2, 3]).is_err());
    }
}
