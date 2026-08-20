-- Rolling quotas: one row per event accepted from a peer, so the 24 h budget
-- is a sum over recent rows rather than a counter that outlives its session.
-- See docs/sync.md#quotas.
CREATE TABLE spending (
    pubkey TEXT NOT NULL,
    at      INTEGER NOT NULL,
    bytes   INTEGER NOT NULL
) STRICT;

CREATE INDEX spending_pubkey ON spending (pubkey, at);
