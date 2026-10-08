// A boost: somebody passing a thing on unchanged, one hop further than it would have gone.
//
// `@welshman/domain`'s `Repost` owns NIP-18's tags. What it cannot own is the
// content: NIP-18 embeds the boosted event there as JSON and a boost here
// carries a reference alone. A peer in scope for the boost is in scope for the
// thing it names and gets both, because reconciliation diffs id sets. An embedded
// copy would also arrive with no authorship proof of its own, which is why
// `RepostWriter` validates the signature on one and dip's unsigned events could
// never satisfy it. A boost whose subject never turned up is a real state the
// screen has to draw.

import {KindFactory, RepostQuery, RepostReader, RepostWriter} from "@welshman/domain"
import {GENERIC_REPOST, REPOST, type TrustedEvent} from "@welshman/util"

export class BoostWriter extends RepostWriter {
  setEvent(event: TrustedEvent) {
    super.setEvent(event)

    return this.setContent("")
  }
}

/** A boost of a note. */
export const Boost = new KindFactory({
  kind: REPOST,
  reader: RepostReader,
  writer: BoostWriter,
  query: RepostQuery,
})

/** A boost of anything else, which states the kind it carries. */
export const GenericBoost = new KindFactory({
  kind: GENERIC_REPOST,
  reader: RepostReader,
  writer: BoostWriter,
  query: RepostQuery,
})
