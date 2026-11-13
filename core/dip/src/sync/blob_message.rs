//! The blob channel's messages: a binary request and response pair, because
//! blob bytes are most of what crosses a link and a text encoding would add a
//! third to them. `docs/sync.md#blob-sync`, NIP-BE.
//!
//! Every message opens with a kind byte and a two-byte id. A response's kind is
//! its request's with the high bit set, and it carries the request's id.
//! Integers are big-endian.
//!
//! | Kind | Message | After the id |
//! | --- | --- | --- |
//! | `0x01` | Query | the SHA-256, 32 bytes |
//! | `0x02` | Fetch | the SHA-256, offset u64, length u32, flags u8 |
//! | `0x81` | Holding | status u8, total u64, held u64, flags u8 |
//! | `0x82` | Bytes | status u8, then the bytes or the Bao slice over them |
//!
//! Bit 0 of the flags is the proof: asked for in a fetch, offered in a
//! holding. A kind this build does not know is one a later version added, and
//! is skipped. Fields past the ones a kind defines are ignored, so that a later
//! version can append to it.

use anyhow::{Result, bail};

use crate::model::BlobHash;

/// The flag bit that asks for a Bao slice in a fetch and offers one in a holding.
const FLAG_PROOF: u8 = 0b0000_0001;

/// The kind byte and the id ahead of every message.
const PREFIX: usize = 3;

/// The kind bytes.
mod kind {
    /// How much of a blob the peer holds.
    pub const QUERY: u8 = 0x01;
    /// One range of a blob.
    pub const FETCH: u8 = 0x02;
    /// The answer to a query.
    pub const HOLDING: u8 = 0x81;
    /// The answer to a fetch.
    pub const BYTES: u8 = 0x82;
}

/// How a request went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobStatus {
    /// Answered.
    Ok = 0,
    /// Not served, whether or not this device holds the blob, and neither
    /// answer says which.
    NotServed = 1,
    /// The requester has spent its blob budget with this device.
    BudgetSpent = 2,
    /// A proof was asked for and this device cannot give one.
    CannotProve = 3,
    /// The range is empty or starts past what this device holds.
    OutOfRange = 4,
}

impl BlobStatus {
    /// The status a byte names, reading one this build does not know as a
    /// refusal, which is the answer that asks nothing more of either side.
    fn from_byte(byte: u8) -> Self {
        match byte {
            0 => Self::Ok,
            2 => Self::BudgetSpent,
            3 => Self::CannotProve,
            4 => Self::OutOfRange,
            _ => Self::NotServed,
        }
    }
}

/// One message on the blob channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobMessage {
    /// How much of a blob the peer holds.
    Query {
        /// Matches the answer to this request.
        id: u16,
        /// The blob.
        hash: BlobHash,
    },
    /// One range of a blob, as a Bao slice when `proof` is set.
    Fetch {
        /// Matches the answer to this request.
        id: u16,
        /// The blob.
        hash: BlobHash,
        /// The first byte wanted.
        offset: u64,
        /// How many bytes are wanted from there.
        length: u32,
        /// Whether the range comes back as a Bao slice.
        proof: bool,
    },
    /// The answer to a query.
    Holding {
        /// The query's id.
        id: u16,
        /// How it went.
        status: BlobStatus,
        /// The blob's whole length, where the holder knows it.
        total: Option<u64>,
        /// The length of the prefix the holder has.
        held: u64,
        /// Whether the holder can prove a range, which only a whole copy can.
        provable: bool,
    },
    /// The answer to a fetch: the bytes, or the Bao slice over them.
    Bytes {
        /// The fetch's id.
        id: u16,
        /// How it went.
        status: BlobStatus,
        /// The bytes, empty unless the status is [`BlobStatus::Ok`].
        body: Vec<u8>,
    },
}

impl BlobMessage {
    /// The id that pairs a request with its answer.
    #[must_use]
    pub fn id(&self) -> u16 {
        match self {
            Self::Query { id, .. }
            | Self::Fetch { id, .. }
            | Self::Holding { id, .. }
            | Self::Bytes { id, .. } => *id,
        }
    }

    /// Whether this asks something of the device that receives it.
    #[must_use]
    pub fn is_request(&self) -> bool {
        matches!(self, Self::Query { .. } | Self::Fetch { .. })
    }

    /// How an answer went, or `None` for a request.
    #[must_use]
    pub fn status(&self) -> Option<BlobStatus> {
        match self {
            Self::Holding { status, .. } | Self::Bytes { status, .. } => Some(*status),
            Self::Query { .. } | Self::Fetch { .. } => None,
        }
    }

    /// The bytes for the blob channel.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();

        match self {
            Self::Query { id, hash } => {
                out.push(kind::QUERY);
                out.extend_from_slice(&id.to_be_bytes());
                out.extend_from_slice(&hash.to_bytes());
            }
            Self::Fetch {
                id,
                hash,
                offset,
                length,
                proof,
            } => {
                out.push(kind::FETCH);
                out.extend_from_slice(&id.to_be_bytes());
                out.extend_from_slice(&hash.to_bytes());
                out.extend_from_slice(&offset.to_be_bytes());
                out.extend_from_slice(&length.to_be_bytes());
                out.push(if *proof { FLAG_PROOF } else { 0 });
            }
            Self::Holding {
                id,
                status,
                total,
                held,
                provable,
            } => {
                out.push(kind::HOLDING);
                out.extend_from_slice(&id.to_be_bytes());
                out.push(*status as u8);
                // Zero is no length at all, which no blob has, so it stands for unknown.
                out.extend_from_slice(&total.unwrap_or(0).to_be_bytes());
                out.extend_from_slice(&held.to_be_bytes());
                out.push(if *provable { FLAG_PROOF } else { 0 });
            }
            Self::Bytes { id, status, body } => {
                out.push(kind::BYTES);
                out.extend_from_slice(&id.to_be_bytes());
                out.push(*status as u8);
                out.extend_from_slice(body);
            }
        }

        out
    }

    /// Read one message off the blob channel, or `None` for a kind this build
    /// does not know.
    pub fn decode(bytes: &[u8]) -> Result<Option<Self>> {
        let Some((&[kind, high, low], rest)) = bytes.split_first_chunk::<PREFIX>() else {
            bail!("a blob message of {} bytes has no kind and id", bytes.len());
        };

        let id = u16::from_be_bytes([high, low]);
        let mut fields = Fields(rest);

        let message = match kind {
            kind::QUERY => Self::Query {
                id,
                hash: fields.hash()?,
            },
            kind::FETCH => Self::Fetch {
                id,
                hash: fields.hash()?,
                offset: u64::from_be_bytes(fields.take()?),
                length: u32::from_be_bytes(fields.take()?),
                proof: fields.byte()? & FLAG_PROOF != 0,
            },
            kind::HOLDING => Self::Holding {
                id,
                status: BlobStatus::from_byte(fields.byte()?),
                total: Some(u64::from_be_bytes(fields.take()?)).filter(|total| *total != 0),
                held: u64::from_be_bytes(fields.take()?),
                provable: fields.byte()? & FLAG_PROOF != 0,
            },
            kind::BYTES => Self::Bytes {
                id,
                status: BlobStatus::from_byte(fields.byte()?),
                body: fields.0.to_vec(),
            },
            _ => return Ok(None),
        };

        Ok(Some(message))
    }
}

/// What is left of a message as its fields are read off the front.
struct Fields<'a>(&'a [u8]);

impl Fields<'_> {
    /// The next `N` bytes, refusing a message too short to hold them.
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let Some((field, rest)) = self.0.split_first_chunk::<N>() else {
            bail!("a blob message ended {} bytes short", N - self.0.len());
        };

        self.0 = rest;

        Ok(*field)
    }

    /// The next byte.
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take::<1>()?[0])
    }

    /// The next 32 bytes, as the hash they are.
    fn hash(&mut self) -> Result<BlobHash> {
        Ok(BlobHash::from_bytes(&self.take()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash() -> BlobHash {
        BlobHash::digest(b"the quick brown fox")
    }

    fn round_trips(message: BlobMessage) {
        assert_eq!(
            BlobMessage::decode(&message.encode()).unwrap(),
            Some(message)
        );
    }

    #[test]
    fn every_message_round_trips() {
        round_trips(BlobMessage::Query {
            id: 1,
            hash: hash(),
        });
        round_trips(BlobMessage::Fetch {
            id: 2,
            hash: hash(),
            offset: 16_384,
            length: 16_384,
            proof: true,
        });
        round_trips(BlobMessage::Holding {
            id: 3,
            status: BlobStatus::Ok,
            total: Some(2 * 1024 * 1024),
            held: 4096,
            provable: false,
        });
        round_trips(BlobMessage::Bytes {
            id: 4,
            status: BlobStatus::Ok,
            body: vec![0, 1, 2, 255],
        });
        round_trips(BlobMessage::Bytes {
            id: 5,
            status: BlobStatus::OutOfRange,
            body: Vec::new(),
        });
    }

    #[test]
    fn a_query_is_the_kind_the_id_and_the_raw_hash() {
        let encoded = BlobMessage::Query {
            id: 0x0102,
            hash: hash(),
        }
        .encode();

        assert_eq!(encoded.len(), PREFIX + 32);
        assert_eq!(&encoded[..PREFIX], &[kind::QUERY, 0x01, 0x02]);
        assert_eq!(&encoded[PREFIX..], &hash().to_bytes());
    }

    #[test]
    fn a_total_nobody_knows_travels_as_zero() {
        let encoded = BlobMessage::Holding {
            id: 1,
            status: BlobStatus::Ok,
            total: None,
            held: 10,
            provable: false,
        }
        .encode();

        assert!(matches!(
            BlobMessage::decode(&encoded).unwrap(),
            Some(BlobMessage::Holding { total: None, .. })
        ));
    }

    #[test]
    fn a_kind_a_later_version_added_is_skipped() {
        assert_eq!(BlobMessage::decode(&[0x03, 0, 1, 9, 9]).unwrap(), None);
    }

    #[test]
    fn a_status_a_later_version_added_reads_as_a_refusal() {
        assert_eq!(
            BlobMessage::decode(&[kind::BYTES, 0, 1, 0x7f]).unwrap(),
            Some(BlobMessage::Bytes {
                id: 1,
                status: BlobStatus::NotServed,
                body: Vec::new(),
            })
        );
    }

    #[test]
    fn fields_a_later_version_appends_are_ignored() {
        let mut encoded = BlobMessage::Query {
            id: 7,
            hash: hash(),
        }
        .encode();
        encoded.extend_from_slice(&[1, 2, 3]);

        assert_eq!(
            BlobMessage::decode(&encoded).unwrap(),
            Some(BlobMessage::Query {
                id: 7,
                hash: hash()
            })
        );
    }

    #[test]
    fn a_message_too_short_for_its_kind_is_an_error() {
        assert!(BlobMessage::decode(&[]).is_err());
        assert!(BlobMessage::decode(&[kind::QUERY, 0]).is_err());
        assert!(BlobMessage::decode(&[kind::QUERY, 0, 1, 0xab]).is_err());
        assert!(BlobMessage::decode(&[kind::BYTES, 0, 1]).is_err());
    }
}
