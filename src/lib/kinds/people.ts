// The list of ours that says the user wants nothing to do with somebody.
//
// Block is a kind of ours, spelled the way NIP-51's mute list is: a set of
// plain `p` tags, which is what the core reads to narrow the contact graph. So
// both go through `@welshman/domain`'s mute list classes at their own kind
// numbers. Neither ever carries a private entry — a NIP-51 private entry is
// readable by its author alone, and a contact has to read these for the graph
// to mean anything. `docs/policy.md#social-graph`.

import {KindFactory, MuteListQuery, MuteListReader, MuteListWriter} from "@welshman/domain"

/** Block, which refuses the peer's sessions and drops what they send. */
export const BLOCK = 16_018

export const Block = new KindFactory({
  kind: BLOCK,
  reader: MuteListReader,
  writer: MuteListWriter,
  query: MuteListQuery,
})
