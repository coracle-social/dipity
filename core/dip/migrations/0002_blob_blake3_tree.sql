-- Blob transfers verify group by group against the BLAKE3 root in `imeta`, so
-- the tree of chaining values they verify against outlives the session that
-- fetched it: a device holding part of a blob can still prove that part to the
-- next peer, and a resume does not re-fetch the list. docs/sync.md#blob-sync.
ALTER TABLE blob ADD COLUMN blake3_tree BLOB;

-- Groups are fetched in order over a serial link, so what is on disk is always
-- a prefix and `stored_bytes` is the resume point on its own. The bitmap this
-- column was reserved for would say the same thing a second time, and a second
-- record of one fact is a record that can disagree with it.
ALTER TABLE blob DROP COLUMN chunks;
