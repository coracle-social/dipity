-- The schema. See docs/storage.md for what each table is for.
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
    address    TEXT,
    -- When this device first saw the event: the earliest row in `event_seen`,
    -- kept here because that aggregate cannot be indexed and arrival order is
    -- how the feed reads. Provenance, so it is never part of an event and
    -- never served.
    seen_at    INTEGER NOT NULL
) STRICT;

CREATE INDEX event_author ON event (pubkey, created_at DESC);
CREATE INDEX event_kind ON event (kind, created_at DESC);
CREATE INDEX event_created_at ON event (created_at DESC);
CREATE INDEX event_address ON event (address, created_at DESC) WHERE address IS NOT NULL;
CREATE INDEX event_arrival ON event (seen_at DESC, id ASC);

-- Redundant on its own — `id` is already the primary key — and here only so
-- `recipient_signature` can name (id, pubkey) as a composite foreign key. That
-- is what makes a signature's author the event's author by construction rather
-- than by convention; SQLite requires the parent columns be uniquely indexed.
CREATE UNIQUE INDEX event_id_author ON event (id, pubkey);

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
-- and never updated. An event's seen_at is the earliest of them, which
-- `event.seen_at` carries so it can be indexed. This never leaves the device
-- and is never served to a peer — together the rows record the user's movements
-- and who they were with. See docs/privacy.md.
--
-- Locally authored events carry one row naming the author's own pubkey, so
-- every stored event has a seen time and the column stays NOT NULL.
CREATE TABLE event_seen (
    event_id    TEXT    NOT NULL REFERENCES event (id) ON DELETE CASCADE,
    pubkey      TEXT    NOT NULL,
    seen_at     INTEGER NOT NULL,
    PRIMARY KEY (event_id, pubkey)
) STRICT;

-- No index on seen_at alone: reads by time go through `event.seen_at`, and the
-- sightings of one event are a primary key lookup.
CREATE INDEX event_seen_pubkey ON event_seen (pubkey, seen_at DESC);

-- Provenance in the other direction: one row per event per peer this device has
-- handed it to. `event_seen` says where something came from and this says where
-- it went, so the two together are the user's movements and who they were with
-- — never served to a peer, and as readable as `event_seen` on a seized device.
--
-- The first handoff is kept and a later one to the same peer is ignored: a row
-- says the device carried something to somebody, and re-serving what a peer
-- already has is a reconciliation detail rather than another share. `signed`
-- says the user's recipient signature went with it, so one withheld while the
-- peer was not trusted to forward is sent once they are.
CREATE TABLE event_shared (
    event_id  TEXT    NOT NULL REFERENCES event (id) ON DELETE CASCADE,
    pubkey    TEXT    NOT NULL,
    shared_at INTEGER NOT NULL,
    signed    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (event_id, pubkey)
) STRICT;

-- What the user put in the trash, and when. Local, like a bookmark kept on
-- this phone: nothing here is served. Emptying the trash, by hand or once a row
-- is a week old, deletes the event, and a row goes with its event by cascade.
CREATE TABLE event_trashed (
    event_id   TEXT    PRIMARY KEY REFERENCES event (id) ON DELETE CASCADE,
    trashed_at INTEGER NOT NULL
) STRICT;

-- Ids this device was offered and declined to store, for a reason that holds
-- on the next encounter too: the author deleted the event, a newer version
-- holds its address, or the user's Accept scope leaves its author out.
-- Reconciliation counts these as held, so a peer stops delivering them every
-- time the two meet. No peer is named and nothing here is served.
--
-- `by_policy` marks a refusal the user's settings made, which a change to them
-- can undo, so those rows go whenever the policy is recompiled. The rest go
-- with the retention sweep, measured from the last time the id was refused.
CREATE TABLE event_refused (
    id         TEXT    PRIMARY KEY,
    created_at INTEGER NOT NULL,
    by_policy  INTEGER NOT NULL,
    refused_at INTEGER NOT NULL
) STRICT;

-- The author's signature over `event_id ‖ recipient_pubkey`, held by the peer
-- it names. It is portable evidence, so it is never served to anyone: a second
-- hop gets an authorship proof derived from it instead. See docs/proofs.md.
--
-- The author is carried rather than joined for: a signature is unverifiable
-- without the key it is by, so the row is the whole witness or it is a fragment
-- that needs a second lookup to mean anything. The composite foreign key is
-- what keeps the copy honest — an author that is not the event's is a write
-- failure, the same way a signature cannot outlive its event.
CREATE TABLE recipient_signature (
    event_id         TEXT NOT NULL,
    author_pubkey    TEXT NOT NULL,
    recipient_pubkey TEXT NOT NULL,
    sig              TEXT NOT NULL,
    PRIMARY KEY (event_id, recipient_pubkey),
    FOREIGN KEY (event_id, author_pubkey) REFERENCES event (id, pubkey) ON DELETE CASCADE
) STRICT;

CREATE INDEX recipient_signature_recipient ON recipient_signature (recipient_pubkey);

-- Key/value store for app policy and UI preferences. Values are JSON so a
-- preference can grow structure without a migration.
CREATE TABLE pref (
    key        TEXT PRIMARY KEY,
    value      TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

-- Blob metadata, extracted from the imeta tag of an event that references the
-- hash. Bytes live outside the database, keyed by sha256. The row lives as long
-- as any reference in `blob_reference` does.
CREATE TABLE blob (
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
    -- survives to be read later.
    imeta        TEXT    NOT NULL DEFAULT '[]',
    -- Groups are fetched in order, so what is on disk is always a prefix and
    -- this is the resume point on its own.
    stored_bytes INTEGER NOT NULL DEFAULT 0,
    complete     INTEGER NOT NULL DEFAULT 0,
    -- Last read, for LRU eviction of originals.
    accessed_at  INTEGER
) STRICT;

-- Each mirrors its read's ORDER BY exactly, or the planner sorts the whole partition in a temp b-tree.
CREATE INDEX blob_wanted ON blob (
    CASE role WHEN 'preview' THEN 0 ELSE 1 END,
    stored_bytes DESC,
    sha256
) WHERE complete = 0;
CREATE INDEX blob_lru ON blob (role, accessed_at, sha256) WHERE complete = 1;

-- Every event whose imeta names a hash references it, not only the first to
-- arrive, so a blob outlives any one of them and goes with the last.
-- docs/sync.md#blob-sync.
CREATE TABLE blob_reference (
    sha256   TEXT NOT NULL,
    event_id TEXT NOT NULL REFERENCES event (id) ON DELETE CASCADE,
    PRIMARY KEY (sha256, event_id)
) STRICT;

-- Deleting an event asks for its references by event, which the primary key
-- orders the other way round.
CREATE INDEX blob_reference_event ON blob_reference (event_id);

-- Pairing: what lets one encounter recognize the next, and what bounds how
-- many strangers it discloses to.

-- A pair secret is derived from a completed session's handshake hash and stored
-- against the peer, so a later encounter is recognized from its tags before
-- either side names a pubkey. One row per pubkey this device has paired with,
-- replaced by a session that authenticated the peer without recognizing it.
CREATE TABLE pair_secret (
    pubkey     TEXT PRIMARY KEY,
    secret     TEXT NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

-- The disclosure budget: one row per AUTH response this device handed to an
-- unrecognized peer, with when. No pubkey — the dialer discloses before the
-- peer has named itself, and a harvester that never answers would otherwise
-- cost nothing. Rows outside the current window are pruned on write.
CREATE TABLE disclosure (
    id           INTEGER PRIMARY KEY,
    disclosed_at INTEGER NOT NULL
) STRICT;

-- The quota ledger: what each peer wrote to this device in the last 24 hours,
-- kept so the window survives the process. A background relaunch otherwise
-- refills every meter, and the stranger pool is the ceiling that has to hold.
-- `docs/sync.md#quotas`.
--
-- `meter` is `event`, `blob` for bytes taken or `served` for bytes given. `pooled` is whether the row counts against the
-- pool every untrusted peer shares. Rows older than the window are deleted as
-- new ones are written, and none is ever served: like `event_seen`, it says who
-- handed this device something and when.
CREATE TABLE spending (
    pubkey   TEXT    NOT NULL,
    meter    TEXT    NOT NULL,
    pooled   INTEGER NOT NULL,
    spent_at INTEGER NOT NULL,
    bytes    INTEGER NOT NULL
) STRICT;

CREATE INDEX spending_spent_at ON spending (spent_at);
