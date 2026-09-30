-- Provenance in the other direction: one row per event per peer this device has
-- handed it to. `event_seen` says where something came from and this says where
-- it went, so the two together are the user's movements and who they were with
-- — never served to a peer, and as readable as `event_seen` on a seized device.
--
-- The first handoff is kept and a later one to the same peer is ignored: a row
-- says the device carried something to somebody, and re-serving what a peer
-- already has is a reconciliation detail rather than another share.
CREATE TABLE event_shared (
    event_id  TEXT    NOT NULL REFERENCES event (id) ON DELETE CASCADE,
    pubkey    TEXT    NOT NULL,
    shared_at INTEGER NOT NULL,
    PRIMARY KEY (event_id, pubkey)
) STRICT;
