// A boost: somebody passing a thing on unchanged, one hop further than it would have gone.
//
// NIP-18 puts the reposted event in `content` as JSON. Ours carries a reference
// and nothing else: reconciliation diffs id sets, so a peer in scope for the
// boost is in scope for the thing it names and gets both, and an embedded copy
// would arrive with no authorship proof of its own. A boost whose subject never
// turned up is a real state the screen has to draw.

import {EventReader, EventWriter, KindFactory} from "@welshman/domain"
import {GENERIC_REPOST, REPOST, type TrustedEvent} from "@welshman/util"

export class RepostReader extends EventReader {
  eventId(): string | undefined {
    return this.event.tags.find(tag => tag[0] === "e")?.[1]
  }

  pubkey(): string | undefined {
    return this.event.tags.find(tag => tag[0] === "p")?.[1]
  }

  /** The boosted event's kind, which only kind 16 states. */
  eventKind(): number | undefined {
    const stated = this.event.tags.find(tag => tag[0] === "k")?.[1]

    return stated === undefined ? undefined : Number(stated)
  }
}

export class RepostWriter extends EventWriter<RepostReader> {
  private boosted?: TrustedEvent

  setEvent(event: TrustedEvent) {
    this.boosted = event

    return this
  }

  validate() {
    if (!this.boosted) {
      throw new Error("A boost has to name the event it boosts. Call setEvent.")
    }
  }

  protected renderDomainTags(): string[][] {
    const boosted = this.boosted

    if (!boosted) return []

    const named = [
      ["e", boosted.id],
      ["p", boosted.pubkey],
    ]

    return this.kind === GENERIC_REPOST ? [...named, ["k", String(boosted.kind)]] : named
  }
}

/** A boost of a note. */
export const Repost = new KindFactory({kind: REPOST, reader: RepostReader, writer: RepostWriter})

/** A boost of anything else, which states the kind it carries. */
export const GenericRepost = new KindFactory({
  kind: GENERIC_REPOST,
  reader: RepostReader,
  writer: RepostWriter,
})
