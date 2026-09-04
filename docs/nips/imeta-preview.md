# imeta: preview of

A change to [NIP-92](https://github.com/nostr-protocol/nips/blob/master/92.md) and [NIP-94](https://github.com/nostr-protocol/nips/blob/master/94.md).

## The change

An event describes a preview as a file in its own right — its own `imeta` tag, with its own `x`, `m`, `dim`, `size` and `blake3` — and one optional key naming the file it stands in for:

```
preview-of <64-hex>
```

The value is the `x` of another blob the same event references. A tag carrying it is a preview; a tag without it is the file as published.

## Why

NIP-94 already has `thumb` and `image`, and both are urls. A url is unreachable on a transport with no network: a device that meets a peer in a corridor can fetch only what is content-addressed, so a preview it can actually receive needs a hash, and a hash of its own means the rest of the metadata comes with it — a preview has a different size, a different mime type and a different BLAKE3 root than the file it stands for.

Naming the original rather than tagging a role is what makes the relation usable. A reader holding a preview and not the original can put the two together and render the small one in place of the big one; a bare `role preview` says only that something is small.

The direction matters. The preview points at the original, so the original's tag is exactly what it would have been without a preview, and a reader that has never heard of this key sees an event with two attachments rather than one it cannot parse.
