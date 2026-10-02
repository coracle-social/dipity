// Putting something into the store, which is the whole of sending it.
//
// There is no relay, no outbox and no thunk: an event is stored and offered to
// whoever comes into range next. It is not signed either — authorship is shown
// to one peer at a time. `docs/proofs.md#events-are-not-signed`.

import {get} from "svelte/store"
import {hash, own, stamp, type EventTemplate, type HashedEvent} from "@welshman/util"
import {Dip} from "$lib/core"
import {afterPublish} from "$lib/data/notifications"
import {session} from "$lib/data/session"

const now = () => Math.floor(Date.now() / 1000)

/**
 * Store an event under this device's identity, and answer what was stored.
 *
 * `replacing` is the event this one supersedes, for a replaceable kind. Two
 * versions stamped in the same second are settled by the lower id, so an edit
 * made within a second of the last is refused and nothing says so — the store
 * answers with the old one and the screen redraws it. Stamping after the
 * version being replaced is what makes the write land.
 * `core/dip/src/db/event/command.rs`.
 */
export const publish = async (
  template: EventTemplate,
  replacing?: HashedEvent,
): Promise<HashedEvent> => {
  const {identity} = get(session)

  if (!identity) {
    throw new Error("Nothing can be published before the core has an identity.")
  }

  const at = Math.max(now(), (replacing?.created_at ?? 0) + 1)
  const event = hash(own(stamp(template, at), identity))

  await Dip.publish({event: JSON.stringify(event)})

  afterPublish(event.kind).catch(error => console.error("the notification offer failed", error))

  return event
}
