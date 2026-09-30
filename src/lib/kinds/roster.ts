// What one person calls the people they have paired with.
//
// Nobody publishes a profile here, so a name is always somebody's name for
// somebody else. NIP-02's `p` tag has a pet name slot and kind 3 is replaceable,
// which is what the retention sweep spares — so a roster travels with content
// and is never swept out from under the names in it.
//
// Not `@welshman/domain`'s `FollowList`: its reader answers pubkeys and drops
// the pet name, which here is the whole payload. Following nobody is the point —
// this says what somebody is called, not that they are read.

import {EventReader, EventWriter, KindFactory, type KindContext} from "@welshman/domain"
import {FOLLOWS} from "@welshman/util"

/** One name, and who it is for. */
export type Naming = {pubkey: string; petname: string}

export class RosterReader extends EventReader {
  namings(): Naming[] {
    return this.event.tags
      .filter(tag => tag[0] === "p" && tag[1] && tag[3])
      .map(tag => ({pubkey: tag[1], petname: tag[3]}))
  }

  petnameFor(pubkey: string): string | undefined {
    return this.namings().find(naming => naming.pubkey === pubkey)?.petname
  }
}

export class RosterWriter extends EventWriter<RosterReader> {
  private namings: Naming[]

  constructor(kind: number, context: KindContext, reader?: RosterReader) {
    super(kind, context, reader)
    this.namings = reader ? reader.namings() : []
    this.dropTags(tag => tag[0] === "p")
  }

  /** Name somebody, replacing whatever they were called before. */
  name(pubkey: string, petname: string) {
    this.namings = [...this.namings.filter(naming => naming.pubkey !== pubkey), {pubkey, petname}]

    return this
  }

  forget(pubkey: string) {
    this.namings = this.namings.filter(naming => naming.pubkey !== pubkey)

    return this
  }

  protected renderDomainTags(): string[][] {
    return this.namings.map(({pubkey, petname}) => ["p", pubkey, "", petname])
  }
}

export const Roster = new KindFactory({kind: FOLLOWS, reader: RosterReader, writer: RosterWriter})
