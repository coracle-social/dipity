-- The initial schema. See docs/storage.md for what each table is for.
--
-- Ids, pubkeys and signatures are lowercase hex TEXT rather than BLOB. They are
-- hex on the wire, in every log line and in every test fixture, and a phone's
-- store is small enough that the extra bytes buy readability cheaply.
--
-- Tables are STRICT so a type error is a write failure here rather than a
-- surprise several hops away, where a column silently holds the wrong affinity.

-- The events themselves. There is no `sig` column: content events carry no
-- signature (docs/proofs.md), and the one signed kind this app produces —
-- 22242 auth — is ephemeral and never stored. Every row here is a
-- `coracle_lib::events::HashedEvent`.
CREATE TABLE event (
    id         TEXT PRIMARY KEY,
    pubkey     TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    kind       INTEGER NOT NULL,
    -- The NIP-01 tags array, verbatim JSON. `event_tag` indexes it; this is
    -- what the event is rebuilt from, so its ordering is the authoritative one.
    tags       TEXT    NOT NULL DEFAULT '[]',
    content    TEXT    NOT NULL,
    -- `kind:pubkey:d` for addressable events and `kind:pubkey:` for plain
    -- replaceable ones, NULL for the rest. Supersession is then one lookup
    -- rather than a kind test plus a tag join.
    address    TEXT
) STRICT;

CREATE INDEX event_author ON event (pubkey, created_at DESC);
CREATE INDEX event_kind ON event (kind, created_at DESC);
CREATE INDEX event_created_at ON event (created_at DESC);
CREATE INDEX event_address ON event (address, created_at DESC) WHERE address IS NOT NULL;

-- One row per tag, so a NIP-01 tag filter is an index lookup. Only
-- single-letter tags are indexed, which is exactly the set NIP-01 filters can
-- name; everything else is read from `event.tags`.
CREATE TABLE event_tag (
    event_id TEXT    NOT NULL REFERENCES event (id) ON DELETE CASCADE,
    -- Position in the event's tags array, so the primary key is stable even
    -- when an event repeats a tag with the same value.
    position INTEGER NOT NULL,
    name     TEXT    NOT NULL,
    value    TEXT    NOT NULL,
    PRIMARY KEY (event_id, position)
) STRICT;

CREATE INDEX event_tag_lookup ON event_tag (name, value);

-- Full-text index over content, for NIP-50 search. A standalone FTS5 table
-- rather than an external-content one: external content is keyed on rowid, and
-- a VACUUM may renumber the rowids of a table whose primary key is not an
-- INTEGER, which would silently desync the index against `event`.
CREATE VIRTUAL TABLE event_fts USING fts5 (
    event_id UNINDEXED,
    content,
    tokenize = 'unicode61 remove_diacritics 2'
);

-- Provenance: one row per event per peer it has been seen from, written once
-- and never updated. An event's seen_at is the earliest of them. This never
-- leaves the device and is never served to a peer — together the rows record
-- the user's movements and who they were with. See docs/privacy.md.
--
-- Locally authored events carry one row naming the author's own pubkey, so
-- every stored event has a seen time and the column stays NOT NULL.
CREATE TABLE event_seen (
    event_id    TEXT    NOT NULL REFERENCES event (id) ON DELETE CASCADE,
    peer_pubkey TEXT    NOT NULL,
    seen_at     INTEGER NOT NULL,
    PRIMARY KEY (event_id, peer_pubkey)
) STRICT;

CREATE INDEX event_seen_at ON event_seen (seen_at DESC);
CREATE INDEX event_seen_peer ON event_seen (peer_pubkey, seen_at DESC);

-- The author's signature over `event_id ‖ recipient_pubkey`, held by the peer
-- it names. It is portable evidence, so it is never served to anyone: a second
-- hop gets an authorship proof derived from it instead. See docs/proofs.md.
CREATE TABLE recipient_signature (
    event_id         TEXT NOT NULL REFERENCES event (id) ON DELETE CASCADE,
    recipient_pubkey TEXT NOT NULL,
    sig              TEXT NOT NULL,
    PRIMARY KEY (event_id, recipient_pubkey)
) STRICT;

CREATE INDEX recipient_signature_recipient ON recipient_signature (recipient_pubkey);

-- Key/value store for app policy and UI preferences. Values are JSON so a
-- preference can grow structure without a migration.
CREATE TABLE pref (
    key        TEXT PRIMARY KEY,
    value      TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

-- Blob metadata, extracted from the imeta tag of the first event seen to
-- reference the hash. That event anchors the blob's permissions, so the row
-- dies with it and is rebuilt from whatever else references the hash.
-- Bytes live outside the database, keyed by sha256. See docs/sync.md.
CREATE TABLE blob (
    sha256       TEXT PRIMARY KEY,
    event_id     TEXT    NOT NULL REFERENCES event (id) ON DELETE CASCADE,
    -- 'preview' or 'original'. Previews take precedence and are kept as long as
    -- their events are; originals are a cache with an LRU ceiling.
    role         TEXT    NOT NULL,
    url          TEXT,
    mime_type    TEXT,
    -- Declared size from imeta, which is a claim by the author until the bytes
    -- arrive. `stored_bytes` is what is actually on disk.
    size         INTEGER,
    dim          TEXT,
    blurhash     TEXT,
    alt          TEXT,
    -- BLAKE3 root, for verified streaming. See docs/nips/imeta-blake3.md.
    blake3       TEXT,
    -- The imeta tag as it arrived, JSON, minus the tag name. The columns above
    -- are the keys this build reads; this is everything the event carried, so a
    -- key we do not model yet — or one a peer's build knows and ours does not —
    -- survives to be read later without a migration to recover it.
    imeta        TEXT    NOT NULL DEFAULT '[]',
    stored_bytes INTEGER NOT NULL DEFAULT 0,
    -- Bitmap of verified chunks, so a transfer interrupted on BLE resumes.
    chunks       BLOB,
    complete     INTEGER NOT NULL DEFAULT 0,
    -- Last read, for LRU eviction of originals.
    accessed_at  INTEGER
) STRICT;

CREATE INDEX blob_event ON blob (event_id);
CREATE INDEX blob_wanted ON blob (role, accessed_at) WHERE complete = 0;
CREATE INDEX blob_lru ON blob (role, accessed_at) WHERE complete = 1;
