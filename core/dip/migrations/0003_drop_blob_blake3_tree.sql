-- Ranges are proved with a Bao slice now: the subtree hashes on the path down
-- to the bytes ride with the bytes, so nothing is fetched or stored ahead of
-- them and a peer needs no tree of its own to check what it is sent. What a
-- device keeps is the outboard tree over its own copy, which lives beside the
-- bytes in the blob store rather than in a row, since it is a fraction of the
-- file rather than a fact about it. docs/sync.md#blob-sync.
ALTER TABLE blob DROP COLUMN blake3_tree;
