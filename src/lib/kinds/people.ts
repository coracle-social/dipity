// The list of ours that says whose posts the user never passes on.
//
// Withhold is a kind of ours, spelled the way NIP-51's mute list is: a set of
// plain `p` tags, which is what the core reads to narrow the contact graph. So
// both go through `@welshman/domain`'s mute list classes at their own kind
// numbers. Neither ever carries a private entry — a NIP-51 private entry is
// readable by its author alone, and a contact has to read these for the graph
// to mean anything. `docs/policy.md#social-graph`.

import {KindFactory, MuteListQuery, MuteListReader, MuteListWriter} from "@welshman/domain"

/** Withhold, whose people the device never passes on and the user's cards vouch for to nobody. */
export const WITHHOLD = 16_018

export const Withhold = new KindFactory({
  kind: WITHHOLD,
  reader: MuteListReader,
  writer: MuteListWriter,
  query: MuteListQuery,
})
