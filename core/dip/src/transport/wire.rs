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
use super::{Channel, Codec, Fragment, Frame, Noise, Outbox, Pipe, Secrecy};

/// The length prefix a bulk write carries.
///
/// L2CAP gives a byte stream on both platforms, so the fragment boundaries GATT
/// gets from the ATT write have to be written down. Two bytes, big-endian,
/// ahead of the fragment. `docs/transport.md#the-l2cap-bandwidth-upgrade`.
const BULK_PREFIX: usize = 2;

/// The largest bulk write, which is what the length prefix can count.
pub const MAX_BULK_MTU: usize = u16::MAX as usize;

/// The encrypted, fragmented, prioritized pipe over one link.
pub struct Wire {
    /// The Noise session encrypting it.
    pub noise: Noise,
    /// Fragmentation and reassembly, both over plaintext.
    codec: Codec,
    /// What is waiting to go out, most urgent channel first.
    pub outbox: Outbox,
    /// The link's negotiated MTU: the ceiling on one write, header and tag
    /// included.
    mtu: usize,
    /// The open L2CAP channel's MTU, once the shell reports one. Bulk is the
    /// only thing that rides it.
    bulk_mtu: Option<usize>,
    /// What has arrived off the bulk stream and does not yet make a fragment.
    bulk_inbox: Vec<u8>,
}

impl Wire {
    /// A wire over a link whose negotiated MTU is `mtu` bytes, with a fresh
    /// Noise session for it.
    pub fn new(role: Role, mtu: usize) -> Result<Self> {
        // Sealed writes are the tighter mode, so an MTU too small for one carries no session.
        if mtu <= HEADER + Noise::TAG {
            bail!("an MTU of {mtu} leaves no room for a sealed payload byte");
        }

        Ok(Self {
            noise: Noise::begin(role)?,
            codec: Codec::default(),
            outbox: Outbox::default(),
            mtu,
            bulk_mtu: None,
            bulk_inbox: Vec::new(),
        })
    }

    /// An L2CAP channel came up: bulk moves onto it, at its MTU.
    pub fn open_bulk(&mut self, mtu: usize) -> Result<()> {
        if mtu <= HEADER + Noise::TAG {
            bail!("a bulk MTU of {mtu} leaves no room for a sealed payload byte");
        }

        self.bulk_mtu = Some(mtu.min(MAX_BULK_MTU));
        self.outbox.set_bulk(true);

        Ok(())
    }

    /// The L2CAP channel went away, so bulk goes back on GATT.
    ///
    /// Whatever was queued for it is still queued: a fragment sized for the
    /// bulk MTU would overrun an ATT write, so the partial frame it belongs to
    /// is dropped and the transfer resumes from what the store already holds.
    pub fn close_bulk(&mut self) {
        self.bulk_mtu = None;
        self.bulk_inbox.clear();
        self.outbox.set_bulk(false);
        self.outbox.discard(Channel::Blob);
        self.codec.forget(Channel::Blob);
    }

    /// Whether bulk is riding an L2CAP channel.
    #[must_use]
    pub fn bulk_is_open(&self) -> bool {
        self.bulk_mtu.is_some()
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

    /// Queue a frame, fragmented to the link's MTU and sealed on its way out.
    ///
    /// An error before the handshake completes: the handshake has its own path,
    /// and nothing else may reach a peer in the clear.
    pub fn send(&mut self, frame: &Frame) -> Result<()> {
        if !self.noise.is_complete() {
            bail!("a frame was queued before the handshake completed");
        }

        let fragments = self
            .codec
            .fragment(frame, self.capacity(frame.channel, Secrecy::Sealed))?;

        self.outbox.push(fragments, Secrecy::Sealed);

        Ok(())
    }

    /// Take one write off the characteristic, returning a decrypted frame
    /// once its last fragment lands.
    pub fn receive(&mut self, write: &[u8]) -> Result<Option<Frame>> {
        let mut fragment = Fragment::decode(write)?;

        // Everything after the handshake is sealed, so one that will not open is not theirs.
        if self.noise.is_complete() {
            fragment.payload = self.noise.decrypt(&fragment.payload)?;
        }

        self.codec.absorb(fragment)
    }

    /// The next fragment for the shell to write on `pipe`, if the last one
    /// there has been acknowledged.
    ///
    /// This is where a payload is sealed, so ciphertext is produced in the
    /// order it goes on the wire whatever the scheduler did to the queue. A
    /// bulk write carries its length ahead of it, since L2CAP is a stream and
    /// keeps no boundaries of its own.
    pub fn next_write(&mut self, pipe: Pipe) -> Result<Option<Vec<u8>>> {
        let Some((mut fragment, secrecy)) = self.outbox.next_write(pipe) else {
            return Ok(None);
        };

        if secrecy == Secrecy::Sealed {
            fragment.payload = self.noise.encrypt(&fragment.payload)?;
        }

        let write = fragment.encode();

        Ok(Some(match pipe {
            Pipe::Gatt => write,
            Pipe::Bulk => length_prefixed(&write),
        }))
    }

    /// Take a slice off the bulk stream, returning every frame it completes.
    ///
    /// A read is whatever the socket handed the shell, so it may hold several
    /// fragments, part of one, or both.
    pub fn receive_bulk(&mut self, read: &[u8]) -> Result<Vec<Frame>> {
        self.bulk_inbox.extend_from_slice(read);

        let mut frames = Vec::new();

        while let Some(write) = self.take_bulk_write()? {
            let fragment = Fragment::decode(&write)?;

            // The bulk pipe carries one channel; anything else on it is a peer misbehaving.
            if fragment.channel != Channel::Blob {
                bail!("{:?} arrived on the bulk pipe", fragment.channel);
            }

            if let Some(frame) = self.receive(&write)? {
                frames.push(frame);
            }
        }

        Ok(frames)
    }

    /// Record that the fragment the shell was handed on `pipe` has been
    /// acknowledged.
    pub fn acknowledge_write(&mut self, pipe: Pipe) {
        self.outbox.acknowledge(pipe);
    }

    /// Cut one length-prefixed write off the front of the bulk stream.
    fn take_bulk_write(&mut self) -> Result<Option<Vec<u8>>> {
        let Some(&[high, low]) = self.bulk_inbox.get(..BULK_PREFIX) else {
            return Ok(None);
        };

        let length = usize::from(u16::from_be_bytes([high, low]));

        if length < HEADER {
            bail!("a bulk write claims {length} bytes, too few to be a fragment");
        }

        if self.bulk_inbox.len() < BULK_PREFIX + length {
            return Ok(None);
        }

        Ok(Some(
            self.bulk_inbox
                .drain(..BULK_PREFIX + length)
                .skip(BULK_PREFIX)
                .collect(),
        ))
    }

    /// Queue a handshake payload without encryption. Only the handshake path
    /// uses this: the peer's handshake state is what decrypts it.
    fn send_plain(&mut self, payload: &[u8]) -> Result<()> {
        let frame = Frame {
            channel: Channel::Control,
            payload: payload.to_vec(),
        };
        let fragments = self
            .codec
            .fragment(&frame, self.capacity(Channel::Control, Secrecy::Clear))?;

        self.outbox.push(fragments, Secrecy::Clear);

        Ok(())
    }

    /// The payload bytes one write on this channel can carry, which the AEAD
    /// tag eats into once a fragment is sealed.
    fn capacity(&self, channel: Channel, secrecy: Secrecy) -> usize {
        let mtu = match (channel, self.bulk_mtu) {
            (Channel::Blob, Some(bulk)) => bulk - BULK_PREFIX,
            _ => self.mtu,
        };

        match secrecy {
            Secrecy::Sealed => mtu - HEADER - Noise::TAG,
            Secrecy::Clear => mtu - HEADER,
        }
    }
}

/// One bulk write: its length, then the fragment.
fn length_prefixed(write: &[u8]) -> Vec<u8> {
    let mut prefixed = Vec::with_capacity(BULK_PREFIX + write.len());

    prefixed.extend_from_slice(&(write.len() as u16).to_be_bytes());
    prefixed.extend_from_slice(write);

    prefixed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pump every queued write from one wire into the other, collecting the
    /// frames that arrive and feeding the handshake whatever belongs to it.
    fn pump(from: &mut Wire, into: &mut Wire) -> Vec<Frame> {
        let mut received = Vec::new();

        while let Some(write) = from.next_write(Pipe::Gatt).unwrap() {
            from.acknowledge_write(Pipe::Gatt);

            assert!(
                write.len() <= from.mtu,
                "a write of {} bytes overran the MTU",
                write.len()
            );

            if let Some(frame) = into.receive(&write).unwrap() {
                if frame.channel == Channel::Control && !into.noise.is_complete() {
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

        while !(dialer.noise.is_complete() && receiver.noise.is_complete()) {
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
            dialer.noise.handshake_hash().unwrap(),
            receiver.noise.handshake_hash().unwrap()
        );
    }

    #[test]
    fn a_control_frame_overtakes_queued_bulk_without_losing_the_cipher() {
        // The MTU is small enough that the sync frame is several fragments.
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

        // Sealing at enqueue would hand the peer these writes out of nonce order.
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

        let mut write = dialer.next_write(Pipe::Gatt).unwrap().unwrap();
        let last = write.len() - 1;
        write[last] ^= 0xff;

        assert!(receiver.receive(&write).is_err());
    }

    #[test]
    fn a_bulk_frame_round_trips_over_a_stream_that_keeps_no_boundaries() {
        let mtu = HEADER + Noise::TAG + 8;
        let (mut dialer, mut receiver) = secured_pair(mtu);

        dialer.open_bulk(mtu).unwrap();
        receiver.open_bulk(mtu).unwrap();

        let sent = Frame {
            channel: Channel::Blob,
            payload: (0..=100u8).collect(),
        };
        dialer.send(&sent).unwrap();

        let mut stream = Vec::new();
        while let Some(write) = dialer.next_write(Pipe::Bulk).unwrap() {
            dialer.acknowledge_write(Pipe::Bulk);
            stream.extend_from_slice(&write);
        }

        // A byte at a time is the worst a socket can do, and the length prefix survives it.
        let received: Vec<Frame> = stream
            .into_iter()
            .flat_map(|byte| receiver.receive_bulk(&[byte]).unwrap())
            .collect();

        assert_eq!(received, vec![sent]);
    }

    #[test]
    fn bulk_does_not_wait_on_an_unacknowledged_gatt_write() {
        let (mut dialer, _receiver) = secured_pair(4096);

        dialer.open_bulk(4096).unwrap();
        dialer
            .send(&Frame {
                channel: Channel::Sync,
                payload: b"events".to_vec(),
            })
            .unwrap();
        dialer
            .send(&Frame {
                channel: Channel::Blob,
                payload: b"bytes".to_vec(),
            })
            .unwrap();

        // One write out on each pipe at once, which is what the second pipe is for.
        assert!(dialer.next_write(Pipe::Gatt).unwrap().is_some());
        assert!(dialer.next_write(Pipe::Gatt).unwrap().is_none());
        assert!(dialer.next_write(Pipe::Bulk).unwrap().is_some());
    }

    #[test]
    fn only_the_blob_channel_rides_the_bulk_pipe() {
        let (mut dialer, mut receiver) = secured_pair(4096);

        dialer.open_bulk(4096).unwrap();
        receiver.open_bulk(4096).unwrap();
        dialer
            .send(&Frame {
                channel: Channel::Sync,
                payload: b"events".to_vec(),
            })
            .unwrap();

        let write = dialer.next_write(Pipe::Gatt).unwrap().unwrap();

        assert!(receiver.receive_bulk(&length_prefixed(&write)).is_err());
    }

    #[test]
    fn closing_the_channel_drops_what_was_cut_for_it() {
        let (mut dialer, _receiver) = secured_pair(4096);

        dialer.open_bulk(4096).unwrap();
        dialer
            .send(&Frame {
                channel: Channel::Blob,
                payload: b"bytes".to_vec(),
            })
            .unwrap();
        dialer.close_bulk();

        // A fragment cut to the bulk MTU would overrun an ATT write, so it goes.
        assert!(!dialer.bulk_is_open());
        assert!(dialer.next_write(Pipe::Bulk).unwrap().is_none());
        assert!(dialer.next_write(Pipe::Gatt).unwrap().is_none());
    }

    #[test]
    fn an_unusable_bulk_mtu_is_an_error() {
        let (mut dialer, _receiver) = secured_pair(4096);

        assert!(dialer.open_bulk(HEADER + Noise::TAG).is_err());
        assert!(dialer.open_bulk(HEADER + Noise::TAG + 1).is_ok());
    }

    #[test]
    fn an_unusable_mtu_is_an_error() {
        assert!(Wire::new(Role::Dialer, 0).is_err());
        assert!(Wire::new(Role::Dialer, HEADER + Noise::TAG).is_err());
        assert!(Wire::new(Role::Dialer, HEADER + Noise::TAG + 1).is_ok());
    }
}
