//! The codec: one characteristic carrying three channels, fragmented to the
//! MTU and scheduled by priority.
//!
//! Separate characteristics would not give QoS isolation, because the ATT queue
//! is per connection. The sender interleaves instead, which keeps the heartbeat
//! alive through a media transfer.
//!
//! Everything here deals in plaintext. The outbox holds fragments rather than
//! finished writes because the scheduler reorders them across channels, while
//! the transport cipher is a strict sequence with no window: a payload has to
//! be sealed in the order it goes on the wire, which is [`Wire`](super::Wire)'s
//! job at dequeue.
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

    /// Which pipe this channel leaves on. Only the blob channel ever moves.
    fn pipe(self, bulk: bool) -> Pipe {
        match (self, bulk) {
            (Channel::Blob, true) => Pipe::Bulk,
            _ => Pipe::Gatt,
        }
    }
}

/// Which of a link's two pipes a fragment travels on.
///
/// GATT carries everything until an L2CAP channel opens; from then on the blob
/// channel rides the bulk pipe and the rest stay where they are. The two are
/// independent — a write in flight on one does not hold up the other, which is
/// most of the point of opening the second.
/// `docs/transport.md#the-l2cap-bandwidth-upgrade`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pipe {
    /// The GATT characteristic, which every link has.
    Gatt,
    /// An open L2CAP channel, which carries bulk only.
    Bulk,
}

/// Set on every fragment but the last of a frame.
const FLAG_MORE: u8 = 0b0000_0001;

/// The bits `docs/transport.md#the-wire-format` requires to be zero.
const FLAG_RESERVED: u8 = !FLAG_MORE;

/// The header, which every fragment carries.
///
/// It stays in the clear, because the receiver routes and reassembles on it
/// before there is a channel to hand the payload to.
pub const HEADER: usize = 2;

/// The largest reassembled frame the codec accepts.
///
/// A peer that exceeds this is misbehaving, because a message envelope around
/// the largest stored event fits comfortably in it. The link that let it
/// through is dropped rather than fed. `docs/sync.md` caps events at 64 KB; this is the
/// reassembly ceiling for whatever carries them.
const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// One message on one channel, before fragmentation or after reassembly.
///
/// Always plaintext: sealing is per fragment, either side of the codec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Which channel it belongs to.
    pub channel: Channel,
    /// The message.
    pub payload: Vec<u8>,
}

/// One write's worth of a frame: the header, and the slice of payload under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    /// The channel of the frame being carried.
    pub channel: Channel,
    /// Whether further fragments of the same frame follow.
    pub more: bool,
    /// The slice this write carries. Plaintext here; ciphertext only between
    /// [`Wire`](super::Wire) sealing it and the peer opening it.
    pub payload: Vec<u8>,
}

impl Fragment {
    /// The bytes for the characteristic: the header, then the payload.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut write = Vec::with_capacity(HEADER + self.payload.len());

        write.push(self.channel as u8);
        write.push(if self.more { FLAG_MORE } else { 0 });
        write.extend_from_slice(&self.payload);

        write
    }

    /// Read one write off the characteristic.
    pub fn decode(write: &[u8]) -> Result<Self> {
        let [channel, flags, payload @ ..] = write else {
            bail!(
                "a write of {} bytes is too short to be a fragment",
                write.len()
            );
        };

        Ok(Self {
            channel: Channel::from_byte(*channel)?,
            more: Self::more_from_flags(*flags)?,
            payload: payload.to_vec(),
        })
    }

    /// Whether more fragments follow, refusing a reserved bit.
    fn more_from_flags(flags: u8) -> Result<bool> {
        match flags & FLAG_RESERVED {
            0 => Ok(flags & FLAG_MORE != 0),
            reserved => bail!("frame sets reserved flag bits {reserved:#010b}"),
        }
    }
}

/// Whether a queued fragment is sealed on its way out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Secrecy {
    /// Sealed with the transport cipher as it leaves the outbox.
    Sealed,
    /// Written as it stands. Only handshake messages travel this way: the
    /// peer's handshake state is what reads them.
    Clear,
}

/// Cuts frames into fragments, and reassembles the fragments that arrive.
///
/// One per link, because reassembly is per channel and per connection.
#[derive(Debug, Default)]
pub struct Codec {
    /// Partial frames, one per channel, since channels interleave.
    partial: BTreeMap<Channel, Vec<u8>>,
}

impl Codec {
    /// Cut `frame` into fragments carrying at most `capacity` payload bytes.
    ///
    /// The caller passes the capacity because a sealed payload gives up room to
    /// the AEAD tag and a handshake message does not.
    pub fn fragment(&self, frame: &Frame, capacity: usize) -> Result<Vec<Fragment>> {
        if capacity == 0 {
            bail!("a fragment with room for no payload byte would never finish a frame");
        }

        // An empty payload is still one fragment: the frame itself is the signal.
        if frame.payload.is_empty() {
            return Ok(vec![Fragment {
                channel: frame.channel,
                more: false,
                payload: Vec::new(),
            }]);
        }

        let mut chunks = frame.payload.chunks(capacity).peekable();
        let mut fragments = Vec::new();

        while let Some(chunk) = chunks.next() {
            fragments.push(Fragment {
                channel: frame.channel,
                more: chunks.peek().is_some(),
                payload: chunk.to_vec(),
            });
        }

        Ok(fragments)
    }

    /// Take one opened fragment, returning a frame once its last one lands.
    pub fn absorb(&mut self, fragment: Fragment) -> Result<Option<Frame>> {
        let buffered = self.partial.entry(fragment.channel).or_default();

        // The link ends on fragments past the frame cap, which are an attack on memory.
        if buffered.len() + fragment.payload.len() > MAX_FRAME_BYTES {
            self.partial.remove(&fragment.channel);
            bail!(
                "a frame on {:?} exceeds the reassembly cap",
                fragment.channel
            );
        }

        buffered.extend_from_slice(&fragment.payload);

        if fragment.more {
            return Ok(None);
        }

        Ok(Some(Frame {
            channel: fragment.channel,
            payload: self.partial.remove(&fragment.channel).unwrap_or_default(),
        }))
    }

    /// Throw away whatever of a channel has arrived but not completed a frame.
    pub fn forget(&mut self, channel: Channel) {
        self.partial.remove(&channel);
    }
}

/// What is waiting to go out on one link, and the order it goes.
///
/// Holds fragments rather than frames, so that a bulk transfer already cut up does
/// not have to be re-fragmented when the heartbeat jumps the queue.
#[derive(Debug, Default)]
pub struct Outbox {
    /// One queue per channel, drained most urgent first.
    queues: BTreeMap<Channel, VecDeque<(Fragment, Secrecy)>>,
    /// Which pipes have an unacknowledged write out. Each is one deep as far as
    /// the core is concerned: the shell acknowledges a write, which releases the
    /// next on that pipe alone.
    in_flight: BTreeMap<Pipe, bool>,
    /// Whether the blob channel has an L2CAP channel of its own to ride.
    bulk: bool,
}

impl Outbox {
    /// Queue every fragment of a frame, behind whatever its channel already
    /// holds.
    pub fn push(&mut self, fragments: Vec<Fragment>, secrecy: Secrecy) {
        for fragment in fragments {
            self.queues
                .entry(fragment.channel)
                .or_default()
                .push_back((fragment, secrecy));
        }
    }

    /// Throw away everything queued on a channel.
    pub fn discard(&mut self, channel: Channel) {
        self.queues.remove(&channel);
    }

    /// Move the blob channel onto its own pipe, or back onto GATT.
    ///
    /// Whatever is already queued goes with it: the fragments are the same
    /// either way and only the pipe they leave on changes.
    pub fn set_bulk(&mut self, open: bool) {
        self.bulk = open;
    }

    /// The next fragment to write on `pipe`, or `None` if a write is in flight
    /// there or that pipe has nothing queued.
    ///
    /// Marks the returned fragment in flight; [`acknowledge`](Self::acknowledge)
    /// releases the next on that pipe.
    pub fn next_write(&mut self, pipe: Pipe) -> Option<(Fragment, Secrecy)> {
        if self.in_flight.get(&pipe).copied().unwrap_or(false) {
            return None;
        }

        let bulk = self.bulk;
        let queued = Channel::ALL
            .iter()
            .filter(|channel| channel.pipe(bulk) == pipe)
            .find_map(|channel| self.queues.get_mut(channel)?.pop_front())?;

        self.in_flight.insert(pipe, true);

        Some(queued)
    }

    /// Record that the write the shell was handed on `pipe` has been
    /// acknowledged.
    pub fn acknowledge(&mut self, pipe: Pipe) {
        self.in_flight.insert(pipe, false);
    }

    /// Whether anything is queued or in flight on either pipe.
    /// [`Draining`](crate::session::State::Draining) waits on it.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        !self.in_flight.values().any(|out| *out) && self.queues.values().all(VecDeque::is_empty)
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
    fn a_fragment_round_trips_through_its_header() {
        let subject = Fragment {
            channel: Channel::Blob,
            more: true,
            payload: b"bulk".to_vec(),
        };
        let write = subject.encode();

        assert_eq!(write.len(), HEADER + 4);
        assert_eq!(Fragment::decode(&write).unwrap(), subject);
    }

    #[test]
    fn a_write_too_short_to_hold_a_header_is_not_a_fragment() {
        assert!(Fragment::decode(&[]).is_err());
        assert!(Fragment::decode(&[Channel::Sync as u8]).is_err());

        // Two bytes are a whole fragment: an empty payload is a signal.
        assert!(Fragment::decode(&[Channel::Sync as u8, 0]).is_ok());
    }

    #[test]
    fn a_reserved_flag_bit_is_refused() {
        for bit in 1..8 {
            assert!(
                Fragment::decode(&[Channel::Sync as u8, 1 << bit]).is_err(),
                "flag bit {bit} is reserved and was accepted"
            );
        }

        assert!(Fragment::decode(&[Channel::Sync as u8, FLAG_MORE]).is_ok());
    }

    #[test]
    fn a_frame_round_trips_through_fragmentation() {
        let codec = Codec::default();
        let subject = frame(Channel::Sync, b"the quick brown fox");
        let fragments = codec.fragment(&subject, 4).unwrap();

        assert!(fragments.len() > 1, "a long payload was not fragmented");
        assert!(
            fragments
                .iter()
                .all(|fragment| fragment.encode().len() <= HEADER + 4)
        );

        let mut reassembling = Codec::default();
        let mut reassembled = None;

        for fragment in fragments {
            reassembled = reassembling.absorb(fragment).unwrap();
        }

        assert_eq!(reassembled, Some(subject));
    }

    #[test]
    fn channels_reassemble_independently() {
        let codec = Codec::default();
        let bulk = codec
            .fragment(&frame(Channel::Blob, b"aaaaaaaa"), 4)
            .unwrap();
        let beat = codec
            .fragment(&frame(Channel::Control, b"ping"), 4)
            .unwrap();

        let mut reassembling = Codec::default();

        // The heartbeat lands in the middle of the transfer.
        assert!(reassembling.absorb(bulk[0].clone()).unwrap().is_none());
        assert_eq!(
            reassembling.absorb(beat[0].clone()).unwrap(),
            Some(frame(Channel::Control, b"ping"))
        );
        assert_eq!(
            reassembling.absorb(bulk[1].clone()).unwrap(),
            Some(frame(Channel::Blob, b"aaaaaaaa"))
        );
    }

    #[test]
    fn control_pre_empts_bulk() {
        let codec = Codec::default();
        let mut outbox = Outbox::default();

        outbox.push(
            codec.fragment(&frame(Channel::Blob, b"bulk"), 64).unwrap(),
            Secrecy::Sealed,
        );
        outbox.push(
            codec
                .fragment(&frame(Channel::Control, b"ping"), 64)
                .unwrap(),
            Secrecy::Sealed,
        );

        let (first, _) = outbox.next_write(Pipe::Gatt).unwrap();
        assert_eq!(first.channel, Channel::Control);

        // Nothing else goes out until the shell acknowledges the write.
        assert!(outbox.next_write(Pipe::Gatt).is_none());
        outbox.acknowledge(Pipe::Gatt);

        let (second, _) = outbox.next_write(Pipe::Gatt).unwrap();
        assert_eq!(second.channel, Channel::Blob);
    }

    #[test]
    fn a_handshake_fragment_keeps_its_secrecy_through_the_queue() {
        let codec = Codec::default();
        let mut outbox = Outbox::default();

        outbox.push(
            codec
                .fragment(&frame(Channel::Control, b"handshake"), 64)
                .unwrap(),
            Secrecy::Clear,
        );

        let (_, secrecy) = outbox.next_write(Pipe::Gatt).unwrap();
        assert_eq!(secrecy, Secrecy::Clear);
    }

    #[test]
    fn a_capacity_with_no_room_for_a_payload_is_an_error() {
        assert!(
            Codec::default()
                .fragment(&frame(Channel::Sync, b"x"), 0)
                .is_err()
        );
    }

    #[test]
    fn reassembly_stops_at_the_cap() {
        let mut codec = Codec::default();

        // "More" fragments fed forever must sooner or later be refused.
        let fragment = Fragment {
            channel: Channel::Blob,
            more: true,
            payload: vec![0u8; 512],
        };

        for _ in 0..MAX_FRAME_BYTES / 512 {
            assert!(codec.absorb(fragment.clone()).unwrap().is_none());
        }

        // One more fragment crosses the cap.
        assert!(codec.absorb(fragment).is_err());
    }
}
