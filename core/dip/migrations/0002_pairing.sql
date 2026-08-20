-- Pairing: what lets one encounter recognize the next, and what bounds how
-- many strangers it discloses to. See docs/discovery.md#recognition and
-- docs/policy.md#discoverability.

-- A pair secret is derived from a completed session's handshake hash and stored
-- against the peer, so a later encounter is recognized from its tags before
-- either side names a pubkey. One row per pubkey this device has paired with;
-- the first pairing establishes it and later encounters leave it alone.
CREATE TABLE pair_secret (
    pubkey     TEXT PRIMARY KEY,
    secret     TEXT NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

-- The disclosure budget: each pubkey this device has disclosed its identity
-- to, with when. A harvester that reconnects under a fresh burner pubkey is
-- bounded by how many first-time disclosures fall inside one window.
CREATE TABLE disclosure (
    pubkey       TEXT PRIMARY KEY,
    disclosed_at INTEGER NOT NULL
) STRICT;
