//! Blob metadata: what a stored event says about a file, and how much of that
//! file this device holds. The bytes themselves live outside the database,
//! keyed by [`BlobHash`].

use std::fmt;
use std::str::FromStr;

use anyhow::{Result, bail};
use coracle_lib::events::EventId;
use coracle_lib::tags::Tag;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

/// A SHA-256 written as hex.
const HASH_CHARS: usize = 64;

/// Enough of a hash to tell one in-flight request from another.
const SHORT_CHARS: usize = 16;

/// A blob's address: the SHA-256 of the whole file, 64 lowercase hex
/// characters.
///
/// The value arrives from a peer, in the `x` of an `imeta` tag on a gossiped
/// event, so it is parsed there and nowhere later. An unchecked hash that
/// reached the store would be a durable row on the want list that no reader
/// could match back — every read normalizes — and a shorter one would be a
/// panic anywhere the code took a prefix of it.
///
/// Holding the invariant in the type is what removes the checks: the SQL key,
/// the filename and the request id are all `as_str`, and [`short`](Self::short)
/// can slice because the length is fixed at construction.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobHash(String);

impl BlobHash {
    /// Parse a hash, lowercasing it.
    ///
    /// Anything that is not 64 hex characters is an error rather than a miss:
    /// it can address no file and no row, so a caller carrying one has a bug
    /// and a peer sending one is not to be answered from the store.
    pub fn parse(value: &str) -> Result<Self> {
        if value.len() != HASH_CHARS || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("{value} is not 32 bytes of hex");
        }

        Ok(Self(value.to_ascii_lowercase()))
    }

    /// The hash of `bytes` — what a finished transfer is checked against.
    #[must_use]
    pub fn digest(bytes: &[u8]) -> Self {
        Self(hex::encode(Sha256::digest(bytes)))
    }

    /// The hash as the columns, the wire and the filenames hold it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The leading characters, enough to correlate one request with its answer.
    #[must_use]
    pub fn short(&self) -> &str {
        &self.0[..SHORT_CHARS]
    }
}

impl fmt::Display for BlobHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for BlobHash {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for BlobHash {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::parse(value)
    }
}

impl Serialize for BlobHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for BlobHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;

        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Whether a blob is the small version or the full one. An event tells them
/// apart with the `preview-of` of `docs/nips/imeta-preview.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlobRole {
    /// A downscaled stand-in, cheap enough to move over BLE unconditionally.
    Preview,
    /// The file as published.
    Original,
}

impl BlobRole {
    /// How the role is stored.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::Original => "original",
        }
    }

    /// Read a stored role back.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "preview" => Some(Self::Preview),
            "original" => Some(Self::Original),
            _ => None,
        }
    }
}

/// A blob this device knows about, whether or not it holds the bytes.
///
/// The row is anchored to the event that first referenced the hash, ordered by
/// seen time rather than `created_at`, and inherits that event's permissions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blob {
    /// SHA-256 of the whole file. The address it is fetched by.
    pub sha256: BlobHash,
    /// The event this blob's permissions come from.
    pub event_id: EventId,
    /// Preview or original.
    pub role: BlobRole,
    /// Where the author said it could be fetched.
    pub url: Option<String>,
    /// MIME type, as claimed by `imeta`.
    pub mime_type: Option<String>,
    /// Size in bytes, as claimed by `imeta`.
    pub size: Option<i64>,
    /// `<width>x<height>`, as claimed by `imeta`.
    pub dim: Option<String>,
    /// Blurhash placeholder, for rendering before the bytes arrive.
    pub blurhash: Option<String>,
    /// Alt text.
    pub alt: Option<String>,
    /// BLAKE3 root, lowercase hex, that each group is verified against.
    pub blake3: Option<String>,
    /// The `imeta` tag as it arrived, minus the tag name.
    pub imeta: Vec<String>,
    /// How many bytes are on disk, which is where a transfer resumes.
    pub stored_bytes: i64,
    /// The verified chaining value of every group, once a transfer has one.
    pub blake3_tree: Option<Vec<u8>>,
    /// Whether the whole file is held and hashes to its address.
    pub complete: bool,
    /// When it was last read, for LRU eviction.
    pub accessed_at: Option<i64>,
}

impl Blob {
    /// A blob known only by hash, with no metadata yet.
    #[must_use]
    pub fn new(sha256: BlobHash, event_id: EventId, role: BlobRole) -> Self {
        Self {
            sha256,
            event_id,
            role,
            url: None,
            mime_type: None,
            size: None,
            dim: None,
            blurhash: None,
            alt: None,
            blake3: None,
            imeta: Vec::new(),
            stored_bytes: 0,
            blake3_tree: None,
            complete: false,
            accessed_at: None,
        }
    }

    /// Read a NIP-92 `imeta` tag into a blob.
    ///
    /// `None` when the tag carries no `x` or one that is not a hash, since a
    /// blob that cannot be addressed cannot be fetched or verified either —
    /// and this is the boundary a peer's bytes are checked at.
    ///
    /// The role is the tag's own: a `preview-of` naming the original it stands
    /// in for makes it a preview, and anything else is the file as published.
    /// A `preview-of` that is not a hash names no original, so it is read as
    /// the original it claims not to be — the direction that grants nothing.
    #[must_use]
    pub fn from_imeta(tag: &Tag, event_id: EventId) -> Option<Self> {
        let imeta = tag.values().to_vec();
        let entries = || imeta.iter().filter_map(|entry| split_entry(entry));
        let role = entries()
            .find_map(|(key, value)| {
                (key == "preview-of" && BlobHash::parse(value).is_ok()).then_some(BlobRole::Preview)
            })
            .unwrap_or(BlobRole::Original);

        // The first `x` wins, so a second one cannot redirect the blob.
        let sha256 = entries().find_map(|(key, value)| (key == "x").then_some(value))?;
        let mut blob = Self::new(BlobHash::parse(sha256).ok()?, event_id, role);

        for (key, value) in entries() {
            match key {
                "url" => blob.url = Some(value.to_string()),
                "m" => blob.mime_type = Some(value.to_string()),
                "size" => blob.size = value.parse().ok(),
                "dim" => blob.dim = Some(value.to_string()),
                "blurhash" => blob.blurhash = Some(value.to_string()),
                "alt" => blob.alt = Some(value.to_string()),
                "blake3" => blob.blake3 = Some(value.to_string()),
                _ => {}
            }
        }

        blob.imeta = imeta;

        Some(blob)
    }

    /// The value of an `imeta` key, whether or not this build models it.
    #[must_use]
    pub fn imeta_value(&self, key: &str) -> Option<&str> {
        self.imeta
            .iter()
            .filter_map(|entry| split_entry(entry))
            .find_map(|(name, value)| (name == key).then_some(value))
    }
}

/// One `imeta` entry as its key and its value; only the first space separates
/// them, so a value may carry spaces of its own.
fn split_entry(entry: &str) -> Option<(&str, &str)> {
    entry
        .split_once(' ')
        .map(|(key, value)| (key, value.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(values: &[&str]) -> Tag {
        Tag::new("imeta", values.iter().copied())
    }

    /// A hash that is really 32 bytes of hex, since nothing else parses.
    fn hash(seed: u8) -> String {
        hex::encode([seed; 32])
    }

    #[test]
    fn imeta_is_read_key_by_key() {
        let blob = Blob::from_imeta(
            &tag(&[
                "url https://example.com/x.jpg",
                "m image/jpeg",
                &format!("x {}", hash(0xab)),
                "blake3 def456",
                "size 2048",
                "dim 640x480",
                "alt a dog, asleep",
                "unknown whatever",
            ]),
            EventId::new([1u8; 32]),
        )
        .unwrap();

        assert_eq!(blob.sha256.as_str(), hash(0xab));
        assert_eq!(blob.blake3.as_deref(), Some("def456"));
        assert_eq!(blob.mime_type.as_deref(), Some("image/jpeg"));
        assert_eq!(blob.size, Some(2048));
        assert_eq!(blob.dim.as_deref(), Some("640x480"));
        // Values may carry spaces; only the first one separates key from value.
        assert_eq!(blob.alt.as_deref(), Some("a dog, asleep"));
        assert!(!blob.complete);

        // Everything the tag carried is kept, in tag order and including the
        // key this build has no field for.
        assert_eq!(blob.imeta.len(), 8);
        assert_eq!(blob.imeta[0], "url https://example.com/x.jpg");
        assert_eq!(blob.imeta_value("unknown"), Some("whatever"));
        assert_eq!(blob.imeta_value("m"), Some("image/jpeg"));
        assert_eq!(blob.imeta_value("absent"), None);
    }

    #[test]
    fn a_tag_naming_the_original_it_stands_in_for_is_a_preview() {
        let id = EventId::new([1u8; 32]);
        let role = |values: &[&str]| Blob::from_imeta(&tag(values), id).unwrap().role;

        assert_eq!(
            role(&[
                &format!("x {}", hash(2)),
                &format!("preview-of {}", hash(1))
            ]),
            BlobRole::Preview
        );
        assert_eq!(role(&[&format!("x {}", hash(2))]), BlobRole::Original);

        // A preview outranks an original on the want list and is never evicted,
        // so a marker naming no original it could stand in for buys neither.
        for malformed in [
            "preview-of",
            "preview-of ",
            "preview-of yes",
            "preview-of ab",
        ] {
            assert_eq!(
                role(&[&format!("x {}", hash(2)), malformed]),
                BlobRole::Original,
                "{malformed:?} was read as a preview"
            );
        }
    }

    #[test]
    fn a_blob_survives_a_json_round_trip() {
        // The event id is a type now, and it serializes as the hex the column
        // holds, so a Blob is still a plain JSON object.
        let blob = Blob::from_imeta(
            &tag(&[&format!("x {}", hash(3)), "m image/jpeg", "size 2048"]),
            EventId::new([1u8; 32]),
        )
        .unwrap();

        let json = serde_json::to_string(&blob).unwrap();

        assert!(json.contains(&blob.event_id.to_hex()));
        assert!(json.contains(blob.sha256.as_str()));
        assert_eq!(serde_json::from_str::<Blob>(&json).unwrap(), blob);
    }

    #[test]
    fn a_json_hash_is_parsed_like_any_other() {
        // Nothing may hand a Blob a hash that skipped the check, deserializing
        // included: the row it would write is the one no read can find.
        assert!(serde_json::from_str::<BlobHash>(&format!("\"{}\"", hash(7))).is_ok());
        assert!(serde_json::from_str::<BlobHash>("\"ab\"").is_err());
    }

    #[test]
    fn imeta_without_a_usable_hash_is_not_a_blob() {
        let id = EventId::new([1u8; 32]);

        for values in [
            // No `x` at all.
            vec!["url https://example.com/x.jpg", "m image/jpeg"],
            // The reproducer: two characters where a hash should be. Stored as
            // it arrived, it panicked every later fetch that took a prefix.
            vec!["x ab"],
            // Hex, but not enough of it, and not hex at all.
            vec!["x abcdef"],
            vec!["x not-a-hash-at-all"],
            // The right length, wrong alphabet.
            vec!["x zz"],
        ] {
            assert!(
                Blob::from_imeta(&tag(&values), id).is_none(),
                "{values:?} became a blob"
            );
        }
    }

    #[test]
    fn a_hash_is_lowercase_however_it_arrived() {
        // SQLite compares TEXT byte for byte, so an uppercase hash stored as it
        // arrived is a row that its own hash cannot read back.
        let upper = hash(0xab).to_uppercase();
        let blob =
            Blob::from_imeta(&tag(&[&format!("x {upper}")]), EventId::new([1u8; 32])).unwrap();

        assert_eq!(blob.sha256.as_str(), hash(0xab));
    }

    #[test]
    fn a_hash_that_is_not_32_bytes_of_hex_does_not_parse() {
        assert!(BlobHash::parse("").is_err());
        assert!(BlobHash::parse("ab").is_err());
        assert!(BlobHash::parse(&"ab".repeat(31)).is_err());
        assert!(BlobHash::parse(&"ab".repeat(33)).is_err());
        assert!(BlobHash::parse(&format!("{}zz", "ab".repeat(31))).is_err());
        // 64 characters of something that is not ASCII, so byte length and
        // character length disagree.
        assert!(BlobHash::parse(&"é".repeat(64)).is_err());
        assert!(BlobHash::parse("../../etc/passwd").is_err());
    }

    #[test]
    fn a_short_hash_is_a_prefix_of_the_whole_one() {
        let parsed = BlobHash::parse(&hash(0xcd)).unwrap();

        assert_eq!(parsed.short().len(), SHORT_CHARS);
        assert!(parsed.as_str().starts_with(parsed.short()));
        assert_eq!(parsed.to_string(), parsed.as_str());
    }

    #[test]
    fn a_hash_is_the_digest_of_the_bytes_it_addresses() {
        let hash = BlobHash::digest(b"the quick brown fox");

        assert_eq!(BlobHash::parse(hash.as_str()).unwrap(), hash);
        assert_ne!(hash, BlobHash::digest(b"the quick brown fox!"));
    }

    #[test]
    fn roles_round_trip() {
        for role in [BlobRole::Preview, BlobRole::Original] {
            assert_eq!(BlobRole::parse(role.as_str()), Some(role));
        }

        assert_eq!(BlobRole::parse("thumbnail"), None);
    }
}
