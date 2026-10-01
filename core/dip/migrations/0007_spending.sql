-- The quota ledger: what each peer wrote to this device in the last 24 hours,
-- kept so the window survives the process. A background relaunch otherwise
-- refills every meter, and the stranger pool is the ceiling that has to hold.
-- `docs/sync.md#quotas`.
--
-- `meter` is `event` or `blob`. `pooled` is whether the row counts against the
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
