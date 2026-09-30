// The three lists that say what the user thinks of somebody.
//
// Trust and block are kinds of ours and mute is NIP-51's, and all three are
// plain `p` tags: the core reads them with one `PeopleListReader` and derives
// the trust graph from them. Not `@welshman/domain`'s `MuteList`, which keeps
// private entries as NIP-44 ciphertext — trusted peers are meant to read these,
// and nothing in the view signs. `docs/policy.md#social-graph`.

import {EventReader, EventWriter, KindFactory, type KindContext} from "@welshman/domain"
import {MUTES} from "@welshman/util"

/** Trust, which is what lets a peer carry the user's events one hop further. */
export const TRUST = 16_017

/** Block, which refuses the peer's sessions and drops what they send. */
export const BLOCK = 16_018

export class PeopleListReader extends EventReader {
  pubkeys(): string[] {
    return this.event.tags.filter(tag => tag[0] === "p" && tag[1]).map(tag => tag[1])
  }

  includes(pubkey: string): boolean {
    return this.pubkeys().includes(pubkey)
  }
}

export class PeopleListWriter extends EventWriter<PeopleListReader> {
  private pubkeys: string[]

  constructor(kind: number, context: KindContext, reader?: PeopleListReader) {
    super(kind, context, reader)
    this.pubkeys = reader ? reader.pubkeys() : []
    this.dropTags(tag => tag[0] === "p")
  }

  add(pubkey: string) {
    this.pubkeys = [...new Set([...this.pubkeys, pubkey])]

    return this
  }

  remove(pubkey: string) {
    this.pubkeys = this.pubkeys.filter(listed => listed !== pubkey)

    return this
  }

  protected renderDomainTags(): string[][] {
    return this.pubkeys.map(pubkey => ["p", pubkey])
  }
}

const list = (kind: number) =>
  new KindFactory({kind, reader: PeopleListReader, writer: PeopleListWriter})

export const Trust = list(TRUST)

export const Block = list(BLOCK)

export const Mute = list(MUTES)
