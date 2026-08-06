# Policy

User and app-defined settings which determine what content gets replicated where.

## Hard limits

Content can never be propagated more than two hops away due to how [authorship proofs](./proofs.md) and [transport](./transport.md) are structured. Data will always flow over a local bluetooth connection, and is never valid unless accompanied by a cryptographic proof of authorship.

## Follows

## Friends

## Mutes

## Blocks

## Hops

Users may artificially limit gossip of their own events to one hop, rather than two. In this case, a proof of authorship for a user's own events is not shared with any peers; instead, a second-order inclusion proof in shared instead.

## Defaults
