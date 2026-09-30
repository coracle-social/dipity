// Putting something into the store, which is the whole of sending it.
//
// There is no relay, no outbox and no thunk: an event is stored and offered to
// whoever comes into range next. It is not signed either — authorship is shown
// to one peer at a time. `docs/proofs.md#events-are-not-signed`.

import {get} from "svelte/store"
import {hash, own, stamp, type EventTemplate, type HashedEvent} from "@welshman/util"
import {Dip} from "$lib/core"
import {session} from "$lib/data/session"

/** Store an event under this device's identity, and answer what was stored. */
export const publish = async (template: EventTemplate): Promise<HashedEvent> => {
  const {identity} = get(session)

  if (!identity) {
    throw new Error("Nothing can be published before the core has an identity.")
  }

  const event = hash(own(stamp(template), identity))

  await Dip.publish({event: JSON.stringify(event)})

  return event
}
