// The two lists of ours that say what the user thinks of somebody.
//
// Trust and block are kinds of ours, spelled the way NIP-51's mute list is: a
// set of plain `p` tags, which is what the core reads to derive the trust graph.
// So all three go through `@welshman/domain`'s mute list classes at their own
// kind numbers. None of the three ever carries a private entry — a NIP-51
// private entry is readable by its author alone, and a trusted peer has to read
// these for the graph to mean anything. `docs/policy.md#social-graph`.

import {KindFactory, MuteListQuery, MuteListReader, MuteListWriter} from "@welshman/domain"

/** Trust, which is what lets a peer carry the user's events one hop further. */
export const TRUST = 16_017

/** Block, which refuses the peer's sessions and drops what they send. */
export const BLOCK = 16_018

const list = (kind: number) =>
  new KindFactory({kind, reader: MuteListReader, writer: MuteListWriter, query: MuteListQuery})

export const Trust = list(TRUST)

export const Block = list(BLOCK)
