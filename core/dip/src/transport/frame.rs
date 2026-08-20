//! The codec: one characteristic carrying three channels, fragmented to the
//! MTU and scheduled by priority.
//!
//! The ATT queue is per connection, so separate characteristics would not give
//! QoS isolation. The sender interleaves instead, so the heartbeat stays alive
//! through a media transfer.
//!
//! Header: one byte of channel, one byte of flags. `docs/transport.md#framing`.

use std::collections::{BTreeMap, VecDeque};

use anyhow::{Result, bail};

/// Which channel a frame belongs to, and the order they are served in.
///
/// Lower is more urgent: [`Control`](Channel::Control) pre-empts everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Channel {
    /// Handshake, recognition, `AUTH`, heartbeat. Never yields.
    Control = 0,
    /// The nostr relay protocol: `REQ`, `EVENT`, `NEG-*`.
    Sync = 1,
    /// Blob fragments, which yield to both of the above and move to L2CAP
    /// wherever one opens.
    Blob = 2,
}

impl Channel {
    /// Every channel, most urgent first.
    const ALL: [Channel; 3] = [Channel::Control, Channel::Sync, Channel::Blob];

    /// The channel a header byte names.
    fn from_byte(byte: u8) -> Result<Self> {
        match byte {
            0 => Ok(Channel::Control),
            1 => Ok(Channel::Sync),
            2 => Ok(Channel::Blob),
            other => bail!("frame names channel {other}, which does not exist"),
        }
    }
}

/// Set on every fragment but the last of a frame.
const FLAG_MORE: u8 = 0b0000_0001;

/// The header, which every fragment carries.
const HEADER: usize = 2;

/// The largest reassembled frame the codec accepts.
///
/// A message envelope around the largest stored event fits comfortably in this,
/// so a peer that exceeds it is misbehaving: the link that let it through is
/// dropped rather than fed. `docs/sync.md` caps events at 64 KB; this is the
/// reassembly ceiling for whatever carries them.
const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// One message on one channel, before fragmentation or after reassembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Which channel it belongs to.
    pub channel: Channel,
    /// The message. Ciphertext on the wire; plaintext either side of
    /// [`Noise`](super::Noise).
    pub payload: Vec<u8>,
}

/// Fragments frames down to the MTU, and reassembles what arrives.
///
/// One per link, because the MTU is negotiated per connection and the shell
/// reports it on connect.
pub struct Codec {
    /// Usable payload per write: the MTU the shell reported, less the header.
    capacity: usize,
    /// Partial frames, one per channel, since channels interleave.
    partial: BTreeMap<Channel, Vec<u8>>,
}

impl Codec {
    /// A codec for a link whose negotiated MTU is `mtu` bytes.
    pub fn new(mtu: usize) -> Result<Self> {
        let Some(capacity) = mtu.checked_sub(HEADER).filter(|room| *room > 0) else {
            bail!("an MTU of {mtu} leaves no room for a payload");
        };

        Ok(Self {
            capacity,
            partial: BTreeMap::new(),
        })
    }

    /// Cut `frame` into writes, each at most one MTU.
    #[must_use]
    pub fn fragment(&self, frame: &Frame) -> Vec<Vec<u8>> {
        let mut chunks = frame.payload.chunks(self.capacity).peekable();
        let mut writes = Vec::new();

        // An empty payload is still one write: the frame itself is the signal.
        if frame.payload.is_empty() {
            return vec![vec![frame.channel as u8, 0]];
        }

        while let Some(chunk) = chunks.next() {
            let flags = if chunks.peek().is_some() {
                FLAG_MORE
            } else {
                0
            };
            let mut write = Vec::with_capacity(HEADER + chunk.len());

            write.push(frame.channel as u8);
            write.push(flags);
            write.extend_from_slice(chunk);

            writes.push(write);
        }

        writes
    }

    /// Take one write off the characteristic, returning a frame once its last
    /// fragment lands.
    pub fn absorb(&mut self, write: &[u8]) -> Result<Option<Frame>> {
        let Some((&channel, rest)) = write.split_first() else {
            bail!("an empty write is not a fragment");
        };
        let Some((&flags, payload)) = rest.split_first() else {
            bail!("a one-byte write is not a fragment");
        };

        let channel = Channel::from_byte(channel)?;
        let buffered = self.partial.entry(channel).or_default();

        // Fragments beyond the frame cap are an attack on unbounded memory, not
        // a frame: the partial buffer clears and the error ends the link.
        if buffered.len() + payload.len() > MAX_FRAME_BYTES {
            self.partial.remove(&channel);
            bail!("a frame on {channel:?} exceeds the reassembly cap");
        }

        buffered.extend_from_slice(payload);

        if flags & FLAG_MORE != 0 {
            return Ok(None);
        }

        Ok(Some(Frame {
            channel,
            payload: self.partial.remove(&channel).unwrap_or_default(),
        }))
    }
}

/// What is waiting to go out on one link, and the order it goes.
///
/// Holds fragments rather than frames, so a bulk transfer already cut up does
/// not have to be re-fragmented when the heartbeat jumps the queue.
#[derive(Debug, Default)]
pub struct Outbox {
    /// One queue per channel, drained most urgent first.
    queues: BTreeMap<Channel, VecDeque<Vec<u8>>>,
    /// Whether a write is out and unacknowledged. The ATT queue is one deep as
    /// far as the core is concerned: the shell acknowledges each write, which
    /// releases the next.
    in_flight: bool,
}

impl Outbox {
    /// Queue every fragment of a frame, behind whatever its channel already
    /// holds.
    pub fn push(&mut self, channel: Channel, fragments: Vec<Vec<u8>>) {
        self.queues.entry(channel).or_default().extend(fragments);
    }

    /// The next fragment to write, or `None` if a write is in flight or there
    /// is nothing queued.
    ///
    /// Marks the returned fragment in flight; [`acknowledge`](Self::acknowledge)
    /// releases the next.
    pub fn next_write(&mut self) -> Option<Vec<u8>> {
        if self.in_flight {
            return None;
        }

        let fragment = Channel::ALL
            .iter()
            .find_map(|channel| self.queues.get_mut(channel)?.pop_front())?;

        self.in_flight = true;

        Some(fragment)
    }

    /// Record that the write the shell was handed has been acknowledged.
    pub fn acknowledge(&mut self) {
        self.in_flight = false;
    }

    /// Whether anything is queued or in flight.
    /// [`Draining`](crate::session::State::Draining) waits on it.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        !self.in_flight && self.queues.values().all(VecDeque::is_empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(channel: Channel, payload: &[u8]) -> Frame {
        Frame {
            channel,
            payload: payload.to_vec(),
        }
    }

    #[test]
    fn a_frame_round_trips_through_fragmentation() {
        let codec = Codec::new(HEADER + 4).unwrap();
        let subject = frame(Channel::Sync, b"the quick brown fox");
        let writes = codec.fragment(&subject);

        assert!(writes.len() > 1, "a long payload was not fragmented");
        assert!(writes.iter().all(|write| write.len() <= HEADER + 4));

        let mut reassembling = Codec::new(HEADER + 4).unwrap();
        let mut reassembled = None;

        for write in writes {
            reassembled = reassembling.absorb(&write).unwrap();
        }

        assert_eq!(reassembled, Some(subject));
    }

    #[test]
    fn channels_reassemble_independently() {
        let codec = Codec::new(HEADER + 4).unwrap();
        let bulk = codec.fragment(&frame(Channel::Blob, b"aaaaaaaa"));
        let beat = codec.fragment(&frame(Channel::Control, b"ping"));

        let mut reassembling = Codec::new(HEADER + 4).unwrap();

        // The heartbeat lands in the middle of the transfer.
        assert!(reassembling.absorb(&bulk[0]).unwrap().is_none());
        assert_eq!(
            reassembling.absorb(&beat[0]).unwrap(),
            Some(frame(Channel::Control, b"ping"))
        );
        assert_eq!(
            reassembling.absorb(&bulk[1]).unwrap(),
            Some(frame(Channel::Blob, b"aaaaaaaa"))
        );
    }

    #[test]
    fn control_pre_empts_bulk() {
        let codec = Codec::new(HEADER + 64).unwrap();
        let mut outbox = Outbox::default();

        outbox.push(
            Channel::Blob,
            codec.fragment(&frame(Channel::Blob, b"bulk")),
        );
        outbox.push(
            Channel::Control,
            codec.fragment(&frame(Channel::Control, b"ping")),
        );

        let first = outbox.next_write().unwrap();
        assert_eq!(Channel::from_byte(first[0]).unwrap(), Channel::Control);

        // Nothing else goes out until the shell acknowledges the write.
        assert!(outbox.next_write().is_none());
        outbox.acknowledge();

        let second = outbox.next_write().unwrap();
        assert_eq!(Channel::from_byte(second[0]).unwrap(), Channel::Blob);
    }

    #[test]
    fn an_unusable_mtu_is_an_error() {
        assert!(Codec::new(HEADER).is_err());
        assert!(Codec::new(0).is_err());
    }

    #[test]
    fn reassembly_stops_at_the_cap() {
        let mut codec = Codec::new(HEADER + 512).unwrap();

        // "More" fragments fed forever must sooner or later be refused.
        let mut fragment = vec![Channel::Blob as u8, FLAG_MORE];
        fragment.extend_from_slice(&[0u8; 512]);

        for _ in 0..MAX_FRAME_BYTES / 512 {
            assert!(codec.absorb(&fragment).unwrap().is_none());
        }

        // One more fragment crosses the cap.
        assert!(codec.absorb(&fragment).is_err());
    }
}
