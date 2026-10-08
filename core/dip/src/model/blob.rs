//! Blob metadata: what a stored event says about a file, and how much of that
//! file this device holds. The bytes themselves live outside the database,
//! keyed by [`BlobHash`].

use std::fmt;
use std::str::FromStr;

use anyhow::{Result, bail};
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
/// The value is parsed where it arrives from a peer, in the `x` of an `imeta`
/// tag on a gossiped event, and nowhere later. An unchecked hash that
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
    /// it can address no file and no row. A caller carrying one has a bug, and a
    /// peer sending one is not to be answered from the store.
    pub fn parse(value: &str) -> Result<Self> {
        if value.len() != HASH_CHARS || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("{value} is not 32 bytes of hex");
        }

        Ok(Self(value.to_ascii_lowercase()))
    }

    /// The hash of `bytes` — what a finished transfer is checked against.
    #[must_use]
    pub fn digest(bytes: &[u8]) -> Self {
        let mut digest = BlobDigest::new();

        digest.update(bytes);

        digest.finish()
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

/// A [`BlobHash`] taken a piece at a time.
///
/// The address covers the whole file, and a blob is as large as whatever the
/// user attached. The check that a transfer landed the right bytes reads the
/// store the same way the transfer wrote it — a group at a time — rather than
/// holding a copy of the file in memory to hash it.
#[derive(Default)]
pub struct BlobDigest(Sha256);

impl BlobDigest {
    /// A digest over no bytes yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Take in the next bytes of the file, in the order they appear in it.
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    /// The hash of everything taken in.
    #[must_use]
    pub fn finish(self) -> BlobHash {
        BlobHash(hex::encode(self.0.finalize()))
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

/// A blob this device knows about, whether or not it holds the bytes.
///
/// One row per hash, however many events reference it. Which events those are
/// is `blob_reference`, and both the blob's lifetime and its permissions come
/// from there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blob {
    /// SHA-256 of the whole file. The address it is fetched by.
    pub sha256: BlobHash,
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
    /// Whether the whole file is held and hashes to its address.
    pub complete: bool,
}

impl Blob {
    /// A blob known only by hash, with no metadata yet.
    #[must_use]
    pub fn new(sha256: BlobHash) -> Self {
        Self {
            sha256,
            url: None,
            mime_type: None,
            size: None,
            dim: None,
            blurhash: None,
            alt: None,
            blake3: None,
            imeta: Vec::new(),
            stored_bytes: 0,
            complete: false,
        }
    }

    /// Read a NIP-92 `imeta` tag into a blob.
    ///
    /// `None` when the tag carries no `x` or one that is not a hash, since a
    /// blob that cannot be addressed cannot be fetched or verified either —
    /// and this is the boundary a peer's bytes are checked at.
    #[must_use]
    pub fn from_imeta(tag: &Tag) -> Option<Self> {
        let imeta = tag.values().to_vec();
        let entries = || imeta.iter().filter_map(|entry| split_entry(entry));
        // The first `x` wins, and a second one cannot redirect the blob.
        let sha256 = entries().find_map(|(key, value)| (key == "x").then_some(value))?;
        let mut blob = Self::new(BlobHash::parse(sha256).ok()?);

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

    /// The whole file's length as the anchoring event committed to it.
    ///
    /// The columns are SQLite's signed integers and the transfer counts bytes.
    /// The two unit systems meet here rather than at each of the half-dozen
    /// places a fetch weighs a length. A negative size names no file and is
    /// read as no claim at all.
    #[must_use]
    pub fn declared_size(&self) -> Option<u64> {
        self.size.and_then(|size| u64::try_from(size).ok())
    }

    /// How many bytes are on disk, which is where a transfer resumes.
    #[must_use]
    pub fn stored(&self) -> u64 {
        u64::try_from(self.stored_bytes).unwrap_or(0)
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
/// them, and a value may carry spaces of its own.
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
        let blob = Blob::from_imeta(&tag(&[
            "url https://example.com/x.jpg",
            "m image/jpeg",
            &format!("x {}", hash(0xab)),
            "blake3 def456",
            "size 2048",
            "dim 640x480",
            "alt a dog, asleep",
            "unknown whatever",
        ]))
        .unwrap();

        assert_eq!(blob.sha256.as_str(), hash(0xab));
        assert_eq!(blob.blake3.as_deref(), Some("def456"));
        assert_eq!(blob.mime_type.as_deref(), Some("image/jpeg"));
        assert_eq!(blob.size, Some(2048));
        assert_eq!(blob.dim.as_deref(), Some("640x480"));
        // Values may carry spaces; only the first one separates key from value.
        assert_eq!(blob.alt.as_deref(), Some("a dog, asleep"));
        assert!(!blob.complete);

        // Everything the tag carried is kept in order, unknown keys included.
        assert_eq!(blob.imeta.len(), 8);
        assert_eq!(blob.imeta[0], "url https://example.com/x.jpg");
        assert_eq!(blob.imeta_value("unknown"), Some("whatever"));
        assert_eq!(blob.imeta_value("m"), Some("image/jpeg"));
        assert_eq!(blob.imeta_value("absent"), None);
    }

    #[test]
    fn a_blob_survives_a_json_round_trip() {
        let blob = Blob::from_imeta(&tag(&[
            &format!("x {}", hash(3)),
            "m image/jpeg",
            "size 2048",
        ]))
        .unwrap();

        let json = serde_json::to_string(&blob).unwrap();

        assert!(json.contains(blob.sha256.as_str()));
        assert_eq!(serde_json::from_str::<Blob>(&json).unwrap(), blob);
    }

    #[test]
    fn a_json_hash_is_parsed_like_any_other() {
        // Nothing may hand a Blob an unchecked hash, deserializing included.
        assert!(serde_json::from_str::<BlobHash>(&format!("\"{}\"", hash(7))).is_ok());
        assert!(serde_json::from_str::<BlobHash>("\"ab\"").is_err());
    }

    #[test]
    fn imeta_without_a_usable_hash_is_not_a_blob() {
        for values in [
            // No `x` at all.
            vec!["url https://example.com/x.jpg", "m image/jpeg"],
            // The reproducer: two characters where a hash should be, panicking every later prefix.
            vec!["x ab"],
            // Hex, but not enough of it, and not hex at all.
            vec!["x abcdef"],
            vec!["x not-a-hash-at-all"],
            // The right length, wrong alphabet.
            vec!["x zz"],
        ] {
            assert!(
                Blob::from_imeta(&tag(&values)).is_none(),
                "{values:?} became a blob"
            );
        }
    }

    #[test]
    fn a_hash_is_lowercase_however_it_arrived() {
        // An uppercase hash is a row nothing reads back, as SQLite compares TEXT byte for byte.
        let upper = hash(0xab).to_uppercase();
        let blob = Blob::from_imeta(&tag(&[&format!("x {upper}")])).unwrap();

        assert_eq!(blob.sha256.as_str(), hash(0xab));
    }

    #[test]
    fn a_hash_that_is_not_32_bytes_of_hex_does_not_parse() {
        assert!(BlobHash::parse("").is_err());
        assert!(BlobHash::parse("ab").is_err());
        assert!(BlobHash::parse(&"ab".repeat(31)).is_err());
        assert!(BlobHash::parse(&"ab".repeat(33)).is_err());
        assert!(BlobHash::parse(&format!("{}zz", "ab".repeat(31))).is_err());
        // 64 characters of non-ASCII, whose byte length and character length disagree.
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
}
