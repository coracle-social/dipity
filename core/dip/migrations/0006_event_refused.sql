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
