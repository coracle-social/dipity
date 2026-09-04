-- A blob is referenced by every event whose imeta names its hash, not by the
-- one that happened to arrive first. `blob.event_id` recorded only that first
-- event and cascaded the whole row away with it, so a hash a second event also
-- referenced was dropped while that event still pointed at it — and the want
-- list, built from this table, never asked for it again. docs/sync.md#blob-sync.
CREATE TABLE blob_reference (
    sha256   TEXT NOT NULL,
    event_id TEXT NOT NULL REFERENCES event (id) ON DELETE CASCADE,
    PRIMARY KEY (sha256, event_id)
) STRICT;

-- Deleting an event asks for its references by event, which the primary key
-- orders the other way round.
CREATE INDEX blob_reference_event ON blob_reference (event_id);

INSERT INTO blob_reference (sha256, event_id) SELECT sha256, event_id FROM blob;

-- `blob` keeps only what the hash is, and the references say who wants it.
-- Rebuilt rather than altered: the column is a foreign key, and SQLite refuses
-- to drop one of those.
CREATE TABLE blob_next (
    -- 64 lowercase hex, parsed before it is written: TEXT compares byte for
    -- byte, so a hash in any other spelling is a row nothing can read back.
    sha256       TEXT PRIMARY KEY,
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
    -- BLAKE3 root, which a transfer verifies each group against as it arrives.
    blake3       TEXT,
    -- The imeta tag as it arrived, JSON, minus the tag name. The columns above
    -- are the keys this build reads; this is everything the event carried, so a
    -- key we do not model yet — or one a peer's build knows and ours does not —
    -- survives to be read later without a migration to recover it.
    imeta        TEXT    NOT NULL DEFAULT '[]',
    stored_bytes INTEGER NOT NULL DEFAULT 0,
    complete     INTEGER NOT NULL DEFAULT 0,
    -- Last read, for LRU eviction of originals.
    accessed_at  INTEGER
) STRICT;

INSERT INTO blob_next
SELECT sha256, role, url, mime_type, size, dim, blurhash, alt, blake3,
       imeta, stored_bytes, complete, accessed_at
FROM blob;

DROP TABLE blob;
ALTER TABLE blob_next RENAME TO blob;

-- Each mirrors its read's ORDER BY exactly, or the planner sorts the whole partition in a temp b-tree.
CREATE INDEX blob_wanted ON blob (
    CASE role WHEN 'preview' THEN 0 ELSE 1 END,
    stored_bytes DESC,
    sha256
) WHERE complete = 0;
CREATE INDEX blob_lru ON blob (role, accessed_at, sha256) WHERE complete = 1;
