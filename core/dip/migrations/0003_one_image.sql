-- A picture is one blob, kept as long as an event references it: there is no
-- preview standing in for an original, and no cache of originals to evict from.
-- The table is rebuilt rather than altered, because SQLite cannot drop a column
-- whose definition ends the table under a comment. docs/sync.md#blob-sync.
CREATE TABLE blob_new (
    -- 64 lowercase hex, parsed before it is written: TEXT compares byte for
    -- byte, so a hash in any other spelling is a row nothing can read back.
    sha256       TEXT PRIMARY KEY,
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
    -- survives to be read later.
    imeta        TEXT    NOT NULL DEFAULT '[]',
    -- Groups are fetched in order, so what is on disk is always a prefix and
    -- this is the resume point on its own.
    stored_bytes INTEGER NOT NULL DEFAULT 0,
    complete     INTEGER NOT NULL DEFAULT 0
) STRICT;

INSERT INTO blob_new (
    sha256, url, mime_type, size, dim, blurhash, alt, blake3, imeta, stored_bytes, complete
)
SELECT sha256, url, mime_type, size, dim, blurhash, alt, blake3, imeta, stored_bytes, complete
FROM blob;

DROP TABLE blob;
ALTER TABLE blob_new RENAME TO blob;

-- Mirrors the want list's ORDER BY exactly, or the planner sorts the whole partition in a temp b-tree.
CREATE INDEX blob_wanted ON blob (stored_bytes DESC, sha256) WHERE complete = 0;
