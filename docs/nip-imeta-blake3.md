# imeta: BLAKE3 root

A change to [NIP-92](https://github.com/nostr-protocol/nips/blob/master/92.md) and [NIP-94](https://github.com/nostr-protocol/nips/blob/master/94.md).

## The change

`imeta` and NIP-94 file metadata gain one optional key, the BLAKE3 root of the blob:

```
blake3 <64-hex>
```

## Why

`x` is a SHA-256 over the whole file, so a receiver learns nothing until the last byte arrives: an unbounded stream cannot be capped, bad bytes cannot be rejected early, and a failed transfer cannot be attributed. BLAKE3 is a merkle tree, so the root alone permits [verified streaming](https://github.com/oconnor663/bao): every chunk checks on arrival, with no chunk list on the wire. This matters wherever a file is assembled from several untrusted sources.
