//! Blob metadata: what a stored event says about a file, and how much of that
//! file this device holds.
//!
//! The bytes themselves live outside the database, keyed by SHA-256. What is
//! here is the `imeta` tag of the first event seen to reference the hash, plus
//! the progress of any transfer.

use coracle_lib::tags::Tag;
use serde::{Deserialize, Serialize};

/// Whether a blob is the small version or the full one.
///
/// Previews are kept as long as the events referencing them; originals are a
/// cache with a byte ceiling, evicted least-recently-used. Previews take
/// precedence when both are wanted.
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
/// seen time rather than `created_at`, and inherits that event's permissions:
/// ingest already decided the event was in scope, so its blobs are too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blob {
    /// SHA-256 of the whole file, lowercase hex. The address it is fetched by.
    pub sha256: String,
    /// The event this blob's permissions come from.
    pub event_id: String,
    /// Preview or original.
    pub role: BlobRole,
    /// Where the author said it could be fetched. Not used on the gossip path,
    /// which fetches from the peer standing in front of it.
    pub url: Option<String>,
    /// MIME type, as claimed by `imeta`.
    pub mime_type: Option<String>,
    /// Size in bytes, as claimed by `imeta`. A claim until the bytes arrive;
    /// [`stored_bytes`](Self::stored_bytes) is what is actually on disk.
    pub size: Option<i64>,
    /// `<width>x<height>`, as claimed by `imeta`.
    pub dim: Option<String>,
    /// Blurhash placeholder, for rendering before the bytes arrive.
    pub blurhash: Option<String>,
    /// Alt text.
    pub alt: Option<String>,
    /// BLAKE3 root, lowercase hex. What each chunk verifies against as it
    /// arrives, so a bad chunk costs one chunk and names the peer that sent it.
    /// See `docs/nips/imeta-blake3.md`.
    pub blake3: Option<String>,
    /// The `imeta` tag as it arrived, minus the tag name: one `key value`
    /// string per entry, in tag order.
    ///
    /// The fields above are the keys this build reads. This is everything the
    /// event carried, so a key that is not modeled here — a NIP-92 addition, or
    /// something a peer's build knows and ours does not — is still readable
    /// through [`imeta_value`](Self::imeta_value) rather than being discarded
    /// at the point the event was parsed, where it is unrecoverable.
    pub imeta: Vec<String>,
    /// How many bytes are on disk.
    pub stored_bytes: i64,
    /// Bitmap of verified chunks, so a transfer interrupted on BLE resumes
    /// rather than restarting. `None` before the first chunk arrives.
    pub chunks: Option<Vec<u8>>,
    /// Whether the whole file is held and verified.
    pub complete: bool,
    /// When it was last read, for LRU eviction.
    pub accessed_at: Option<i64>,
}

impl Blob {
    /// A blob known only by hash, with no metadata yet.
    #[must_use]
    pub fn new(sha256: impl Into<String>, event_id: impl Into<String>, role: BlobRole) -> Self {
        Self {
            sha256: sha256.into(),
            event_id: event_id.into(),
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
            chunks: None,
            complete: false,
            accessed_at: None,
        }
    }

    /// Read a NIP-92 `imeta` tag into a blob.
    ///
    /// `None` when the tag carries no `x`, since a blob with no hash cannot be
    /// addressed, fetched or verified. Every entry is kept in
    /// [`imeta`](Self::imeta), whether or not this build has a field for it —
    /// which is what lets `imeta` grow, as it did for `blake3`.
    #[must_use]
    pub fn from_imeta(tag: &Tag, event_id: &str, role: BlobRole) -> Option<Self> {
        let mut blob = Self::new(String::new(), event_id, role);
        let imeta = tag.values().to_vec();

        for entry in &imeta {
            let Some((key, value)) = entry.split_once(' ') else {
                continue;
            };
            let value = value.trim();

            match key {
                "x" => blob.sha256 = value.to_string(),
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

        (!blob.sha256.is_empty()).then_some(blob)
    }

    /// The value of an `imeta` key, whether or not this build models it.
    ///
    /// The first entry wins, as NIP-92 has no meaning for a repeated key.
    #[must_use]
    pub fn imeta_value(&self, key: &str) -> Option<&str> {
        self.imeta.iter().find_map(|entry| {
            entry
                .split_once(' ')
                .filter(|(name, _)| *name == key)
                .map(|(_, value)| value.trim())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(values: &[&str]) -> Tag {
        Tag::new("imeta", values.iter().copied())
    }

    #[test]
    fn imeta_is_read_key_by_key() {
        let blob = Blob::from_imeta(
            &tag(&[
                "url https://example.com/x.jpg",
                "m image/jpeg",
                "x abc123",
                "blake3 def456",
                "size 2048",
                "dim 640x480",
                "alt a dog, asleep",
                "unknown whatever",
            ]),
            "event",
            BlobRole::Original,
        )
        .unwrap();

        assert_eq!(blob.sha256, "abc123");
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
    fn imeta_without_a_hash_is_not_a_blob() {
        let tag = tag(&["url https://example.com/x.jpg", "m image/jpeg"]);

        assert!(Blob::from_imeta(&tag, "event", BlobRole::Original).is_none());
    }

    #[test]
    fn roles_round_trip() {
        for role in [BlobRole::Preview, BlobRole::Original] {
            assert_eq!(BlobRole::parse(role.as_str()), Some(role));
        }

        assert_eq!(BlobRole::parse("thumbnail"), None);
    }
}
