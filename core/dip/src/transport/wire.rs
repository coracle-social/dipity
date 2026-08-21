//! One link's encrypted pipe: Noise over the fragmenting codec, queued by
//! channel priority.
//!
//! [`Wire`] is what a session talks through. It deals in plaintext
//! [`Frame`]s: encryption begins the moment the handshake completes, and the
//! handshake itself — whose messages travel unencrypted, decrypted by the
//! peer's handshake state — is driven through [`initiate`](Wire::initiate)
//! and [`read_handshake`](Wire::read_handshake) without the caller ever
//! seeing a raw Noise message.
//!
//! A payload is sealed as it leaves the outbox, not as it is queued. The
//! transport cipher steps a nonce per call and keeps no window, while the
//! scheduler lets a control frame overtake queued bulk fragments, so sealing
//! at enqueue would hand the peer ciphertext in an order it cannot open.

use anyhow::{Result, bail};

use crate::link::Role;

use super::frame::HEADER;
use super::{Channel, Codec, Fragment, Frame, Noise, Outbox, Secrecy};

/// The encrypted, fragmented, prioritized pipe over one link.
pub struct Wire {
    /// The Noise session encrypting it.
    noise: Noise,
    /// Fragmentation and reassembly, both over plaintext.
    codec: Codec,
    /// What is waiting to go out, most urgent channel first.
    outbox: Outbox,
    /// The link's negotiated MTU: the ceiling on one write, header and tag
    /// included.
    mtu: usize,
}

impl Wire {
    /// A wire over a link whose negotiated MTU is `mtu` bytes, with a fresh
    /// Noise session for it.
    pub fn new(role: Role, mtu: usize) -> Result<Self> {
        // Sealed writes are the tighter of the two modes, so an MTU that cannot
        // carry a sealed payload byte cannot carry the session either.
        if mtu <= HEADER + Noise::TAG {
            bail!("an MTU of {mtu} leaves no room for a sealed payload byte");
        }

        Ok(Self {
            noise: Noise::begin(role)?,
            codec: Codec::default(),
            outbox: Outbox::default(),
            mtu,
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

    /// Queue a frame, fragmented to the link's MTU and sealed on its way out.
    ///
    /// An error before the handshake completes: the handshake has its own path,
    /// and nothing else may reach a peer in the clear.
    pub fn send(&mut self, frame: &Frame) -> Result<()> {
        if !self.noise.is_complete() {
            bail!("a frame was queued before the handshake completed");
        }

        let fragments = self.codec.fragment(frame, self.capacity(Secrecy::Sealed))?;

        self.outbox.push(fragments, Secrecy::Sealed);

        Ok(())
    }

    /// Take one write off the characteristic, returning a decrypted frame
    /// once its last fragment lands.
    pub fn receive(&mut self, write: &[u8]) -> Result<Option<Frame>> {
        let mut fragment = Fragment::decode(write)?;

        // Everything after the handshake is sealed, so one that will not open
        // is not a fragment this peer sent.
        if self.noise.is_complete() {
            fragment.payload = self.noise.decrypt(&fragment.payload)?;
        }

        self.codec.absorb(fragment)
    }

    /// The next fragment for the shell to write, if the last one has been
    /// acknowledged.
    ///
    /// This is where a payload is sealed, so ciphertext is produced in the
    /// order it goes on the wire whatever the scheduler did to the queue.
    pub fn next_write(&mut self) -> Result<Option<Vec<u8>>> {
        let Some((mut fragment, secrecy)) = self.outbox.next_write() else {
            return Ok(None);
        };

        if secrecy == Secrecy::Sealed {
            fragment.payload = self.noise.encrypt(&fragment.payload)?;
        }

        Ok(Some(fragment.encode()))
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
        let frame = Frame {
            channel: Channel::Control,
            payload: payload.to_vec(),
        };
        let fragments = self.codec.fragment(&frame, self.capacity(Secrecy::Clear))?;

        self.outbox.push(fragments, Secrecy::Clear);

        Ok(())
    }

    /// The payload bytes one write can carry, which the AEAD tag eats into
    /// once a fragment is sealed.
    fn capacity(&self, secrecy: Secrecy) -> usize {
        match secrecy {
            Secrecy::Sealed => self.mtu - HEADER - Noise::TAG,
            Secrecy::Clear => self.mtu - HEADER,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pump every queued write from one wire into the other, collecting the
    /// frames that arrive and feeding the handshake whatever belongs to it.
    fn pump(from: &mut Wire, into: &mut Wire) -> Vec<Frame> {
        let mut received = Vec::new();

        while let Some(write) = from.next_write().unwrap() {
            from.acknowledge_write();

            assert!(
                write.len() <= from.mtu,
                "a write of {} bytes overran the MTU",
                write.len()
            );

            if let Some(frame) = into.receive(&write).unwrap() {
                if frame.channel == Channel::Control && !into.is_secured() {
                    into.read_handshake(&frame.payload).unwrap();
                } else {
                    received.push(frame);
                }
            }
        }

        received
    }

    /// A dialer and a receiver whose handshake has completed.
    fn secured_pair(mtu: usize) -> (Wire, Wire) {
        let mut dialer = Wire::new(Role::Dialer, mtu).unwrap();
        let mut receiver = Wire::new(Role::Receiver, mtu).unwrap();

        dialer.initiate().unwrap();

        while !(dialer.is_secured() && receiver.is_secured()) {
            pump(&mut dialer, &mut receiver);
            pump(&mut receiver, &mut dialer);
        }

        (dialer, receiver)
    }

    #[test]
    fn a_frame_crosses_encrypted_after_the_handshake() {
        let (mut dialer, mut receiver) = secured_pair(4096);

        // A frame sent now is ciphertext on the wire and plaintext off it.
        let sent = Frame {
            channel: Channel::Sync,
            payload: b"the quick brown fox".to_vec(),
        };
        dialer.send(&sent).unwrap();

        assert_eq!(pump(&mut dialer, &mut receiver), vec![sent]);
    }

    #[test]
    fn both_sides_agree_on_the_handshake_hash() {
        let (dialer, receiver) = secured_pair(4096);

        assert_eq!(
            dialer.handshake_hash().unwrap(),
            receiver.handshake_hash().unwrap()
        );
    }

    #[test]
    fn a_control_frame_overtakes_queued_bulk_without_losing_the_cipher() {
        // The MTU is small enough that the sync frame is several fragments, so
        // the control frame lands in the middle of them.
        let (mut dialer, mut receiver) = secured_pair(HEADER + Noise::TAG + 8);

        let bulk = Frame {
            channel: Channel::Sync,
            payload: b"the quick brown fox jumps over the lazy dog".to_vec(),
        };
        let beat = Frame {
            channel: Channel::Control,
            payload: b"ping".to_vec(),
        };

        dialer.send(&bulk).unwrap();
        dialer.send(&beat).unwrap();

        // Sealing at enqueue would have handed the peer these writes out of
        // nonce order, and the first sync fragment would fail to open.
        assert_eq!(pump(&mut dialer, &mut receiver), vec![beat, bulk]);
    }

    #[test]
    fn a_frame_past_the_chunk_capacity_round_trips() {
        let mtu = HEADER + Noise::TAG + 16;
        let (mut dialer, mut receiver) = secured_pair(mtu);

        // Several fragments' worth, and not a multiple of the capacity.
        let sent = Frame {
            channel: Channel::Blob,
            payload: (0..=200u8).collect(),
        };
        dialer.send(&sent).unwrap();

        assert_eq!(pump(&mut dialer, &mut receiver), vec![sent]);
    }

    #[test]
    fn an_empty_frame_is_still_a_frame() {
        let (mut dialer, mut receiver) = secured_pair(4096);

        let sent = Frame {
            channel: Channel::Control,
            payload: Vec::new(),
        };
        dialer.send(&sent).unwrap();

        assert_eq!(pump(&mut dialer, &mut receiver), vec![sent]);
    }

    #[test]
    fn nothing_is_sent_before_the_handshake_completes() {
        let mut dialer = Wire::new(Role::Dialer, 4096).unwrap();

        assert!(
            dialer
                .send(&Frame {
                    channel: Channel::Sync,
                    payload: b"early".to_vec(),
                })
                .is_err()
        );
    }

    #[test]
    fn a_frame_that_will_not_open_after_the_handshake_is_an_error() {
        let (mut dialer, mut receiver) = secured_pair(4096);

        dialer
            .send(&Frame {
                channel: Channel::Sync,
                payload: b"the quick brown fox".to_vec(),
            })
            .unwrap();

        let mut write = dialer.next_write().unwrap().unwrap();
        let last = write.len() - 1;
        write[last] ^= 0xff;

        assert!(receiver.receive(&write).is_err());
    }

    #[test]
    fn an_unusable_mtu_is_an_error() {
        assert!(Wire::new(Role::Dialer, 0).is_err());
        assert!(Wire::new(Role::Dialer, HEADER + Noise::TAG).is_err());
        assert!(Wire::new(Role::Dialer, HEADER + Noise::TAG + 1).is_ok());
    }
}
