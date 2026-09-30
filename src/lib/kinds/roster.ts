// What one person calls the people they have paired with.
//
// Nobody publishes a profile here, so a name is always somebody's name for
// somebody else, and NIP-02's pet name slot is where one travels. Kind 3 is
// replaceable, which is what the retention sweep spares, so a roster is never
// swept out from under the names in it.
//
// `@welshman/domain`'s `FollowList` owns the tag layout. The one thing it has no
// reader for is the pet name, which here is the whole payload — following
// nobody is the point, since this says what somebody is called rather than that
// they are read.

import {FollowListQuery, FollowListReader, FollowListWriter, KindFactory} from "@welshman/domain"
import {FOLLOWS, hexTags, matchTags} from "@welshman/util"

/** One name, and who it is for. */
export type Naming = {pubkey: string; petname: string}

export class RosterReader extends FollowListReader {
  namings(): Naming[] {
    return matchTags(hexTags("p"), this.tags())
      .filter(tag => tag[3])
      .map(tag => ({pubkey: tag[1], petname: tag[3]}))
  }

  petnameFor(pubkey: string): string | undefined {
    return this.namings().find(naming => naming.pubkey === pubkey)?.petname
  }
}

export class RosterWriter extends FollowListWriter {
  declare readonly reader?: RosterReader

  /** Name somebody, replacing whatever they were called before. */
  name(pubkey: string, petname: string) {
    this.unfollow(pubkey)

    return this.follow(pubkey, "", petname)
  }

  forget(pubkey: string) {
    return this.unfollow(pubkey)
  }
}

export const Roster = new KindFactory({
  kind: FOLLOWS,
  reader: RosterReader,
  writer: RosterWriter,
  query: FollowListQuery,
})
