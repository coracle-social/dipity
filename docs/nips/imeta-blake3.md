# imeta: BLAKE3 root

An optional field proposed for [NIP-94](https://github.com/coracle-social/nips/blob/nip-be-dipity/94.md), which `imeta` tags take up through [NIP-92](https://github.com/nostr-protocol/nips/blob/master/92.md). The verified range fetch that uses it is in [NIP-BE](https://github.com/coracle-social/nips/blob/nip-be-dipity/BE.md#blobs). Those are the specification; this note keeps the reasoning.

## The change

`imeta` and NIP-94 file metadata gain one optional key, the BLAKE3 root of the blob:

```
blake3 <64-hex>
```

## Why

`x` is a SHA-256 over the whole file. A receiver learns nothing from it until the last byte arrives: an unbounded stream cannot be capped, bad bytes cannot be rejected early, and a failed transfer cannot be attributed. BLAKE3 is a merkle tree, whose root alone permits [verified streaming](https://github.com/oconnor663/bao): every chunk checks on arrival, with no chunk list on the wire. This matters wherever a file is assembled from several untrusted sources.
