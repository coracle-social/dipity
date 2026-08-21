//! One link's encrypted pipe: Noise over the fragmenting codec, queued by
//! channel priority.
//!
//! [`Wire`] is what a session talks through. It deals in plaintext
//! [`Frame`]s: encryption begins the moment the handshake completes, and the
//! handshake itself — whose messages travel unencrypted, decrypted by the
//! peer's handshake state — is driven through [`initiate`](Wire::initiate)
//! and [`read_handshake`](Wire::read_handshake) without the caller ever
//! seeing a raw Noise message.

use anyhow::Result;

use crate::link::Role;

use super::{Channel, Codec, Frame, Noise, Outbox};

/// The encrypted, fragmented, prioritized pipe over one link.
pub struct Wire {
    /// The Noise session encrypting it.
    noise: Noise,
    /// Fragmentation, sized to this link's negotiated MTU.
    codec: Codec,
    /// What is waiting to go out, most urgent channel first.
    outbox: Outbox,
}

impl Wire {
    /// A wire over a link whose negotiated MTU is `mtu` bytes, with a fresh
    /// Noise session for it.
    pub fn new(role: Role, mtu: usize) -> Result<Self> {
        Ok(Self {
            noise: Noise::begin(role),
            codec: Codec::new(mtu)?,
            outbox: Outbox::default(),
        })
    }

    /// Queue the dialer's opening handshake message.
    pub fn initiate(&mut self) -> Result<()> {
        let message = self.noise.first_handshake_message()?;

        self.send_plain(&message)
    }

    /// Take one handshake message, queue whatever reply the pattern calls
    /// for, and say whether the handshake has completed.
    pub fn read_handshake(&mut self, payload: &[u8]) -> Result<bool> {
        if let Some(reply) = self.noise.read_handshake(payload)? {
            self.send_plain(&reply)?;
        }

        Ok(self.noise.is_complete())
    }

    /// Whether the handshake has completed and traffic is encrypted.
    #[must_use]
    pub fn is_secured(&self) -> bool {
        self.noise.is_complete()
    }

    /// Queue a frame, encrypted once the handshake has completed and
    /// fragmented to the link's MTU.
    pub fn send(&mut self, frame: &Frame) -> Result<()> {
        let payload = if self.noise.is_complete() {
            self.noise.encrypt(&frame.payload)?
        } else {
            frame.payload.clone()
        };

        let fragments = self.codec.fragment(&Frame {
            channel: frame.channel,
            payload,
        });

        self.outbox.push(frame.channel, fragments);

        Ok(())
    }

    /// Take one write off the characteristic, returning a decrypted frame
    /// once its last fragment lands.
    pub fn receive(&mut self, write: &[u8]) -> Result<Option<Frame>> {
        let Some(frame) = self.codec.absorb(write)? else {
            return Ok(None);
        };

        if !self.noise.is_complete() {
            return Ok(Some(frame));
        }

        Ok(Some(Frame {
            channel: frame.channel,
            payload: self.noise.decrypt(&frame.payload)?,
        }))
    }

    /// The next fragment for the shell to write, if the last one has been
    /// acknowledged.
    pub fn next_write(&mut self) -> Option<Vec<u8>> {
        self.outbox.next_write()
    }

    /// Record that the fragment the shell was handed has been acknowledged.
    pub fn acknowledge_write(&mut self) {
        self.outbox.acknowledge();
    }

    /// Whether nothing is queued or in flight.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.outbox.is_idle()
    }

    /// The handshake hash, the transcript both sides agree on.
    pub fn handshake_hash(&self) -> Result<[u8; 32]> {
        self.noise.handshake_hash()
    }

    /// The local static public key, for the `noise://` authority in the AUTH
    /// event's relay tag.
    #[must_use]
    pub fn local_static_key(&self) -> [u8; 32] {
        self.noise.local_static_key()
    }

    /// The peer's static public key, once the handshake has completed.
    #[must_use]
    pub fn remote_static_key(&self) -> Option<[u8; 32]> {
        self.noise.remote_static_key()
    }

    /// Queue a handshake payload without encryption. Only the handshake path
    /// uses this: the peer's handshake state is what decrypts it.
    fn send_plain(&mut self, payload: &[u8]) -> Result<()> {
        let fragments = self.codec.fragment(&Frame {
            channel: Channel::Control,
            payload: payload.to_vec(),
        });

        self.outbox.push(Channel::Control, fragments);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pump every queued write from one wire into the other.
    fn pump(from: &mut Wire, into: &mut Wire) -> Option<Frame> {
        let mut received = None;

        while let Some(write) = from.next_write() {
            from.acknowledge_write();

            if let Some(frame) = into.receive(&write).unwrap() {
                if frame.channel == Channel::Control && !into.is_secured() {
                    into.read_handshake(&frame.payload).unwrap();
                } else {
                    received = Some(frame);
                }
            }
        }

        received
    }

    #[test]
    fn a_frame_crosses_encrypted_after_the_handshake() {
        let mut dialer = Wire::new(Role::Dialer, 4096).unwrap();
        let mut receiver = Wire::new(Role::Receiver, 4096).unwrap();

        dialer.initiate().unwrap();
        pump(&mut dialer, &mut receiver); // msg1
        pump(&mut receiver, &mut dialer); // reply
        pump(&mut dialer, &mut receiver); // msg3

        assert!(dialer.is_secured());
        assert!(receiver.is_secured());

        // A frame sent now is ciphertext on the wire and plaintext off it.
        let sent = Frame {
            channel: Channel::Sync,
            payload: b"the quick brown fox".to_vec(),
        };
        dialer.send(&sent).unwrap();

        let received = pump(&mut dialer, &mut receiver).expect("the frame to arrive");
        assert_eq!(received, sent);
    }

    #[test]
    fn both_sides_agree_on_the_handshake_hash() {
        let mut dialer = Wire::new(Role::Dialer, 4096).unwrap();
        let mut receiver = Wire::new(Role::Receiver, 4096).unwrap();

        dialer.initiate().unwrap();
        pump(&mut dialer, &mut receiver);
        pump(&mut receiver, &mut dialer);
        pump(&mut dialer, &mut receiver);

        assert_eq!(
            dialer.handshake_hash().unwrap(),
            receiver.handshake_hash().unwrap()
        );
    }
}
