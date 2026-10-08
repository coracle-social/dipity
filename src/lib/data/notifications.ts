// Which notifications the user wants, and asking them once.
//
// The core reads these preferences to decide when to notify. Switching one on
// asks the phone for permission first. `docs/storage.md#notifications`.

import {get, writable} from "svelte/store"
import {Dip, type NotificationPermission} from "$lib/core"
import {eventsOf, remembered} from "$lib/data/query"
import {session} from "$lib/data/session"

/** The kinds that count as writing, the same list the core counts arrivals by. */
export const CONTENT_KINDS = [1, 1111, 1068, 31922, 31923, 30023]

/** Notify that somebody nearby wants to pair. */
export const notifyPairing = remembered("notifications.pairing", false)

/** Notify that new writing arrived. */
export const notifyContent = remembered("notifications.content", false)

/** Whether the user has been offered notifications so that they are offered once. */
const offered = remembered("notifications.asked", false)

/** What the phone says about letting the app notify. */
export const permission = writable<NotificationPermission>("prompt")

/** Whether the offer to switch notifications on is showing. */
export const offering = writable(false)

/** Read what the phone says now, which changes behind the app's back in its settings. */
export const refreshPermission = () =>
  Dip.notificationPermission()
    .then(({permission: answer}) => permission.set(answer))
    .catch(() => undefined)

/**
 * Switch one notification on or off. On asks the phone first, and stays off
 * if the phone says no.
 */
export const setNotify = async (which: typeof notifyPairing, on: boolean) => {
  offered.set(true)

  if (!on) return which.set(false)

  const {permission: answer} = await Dip.requestNotificationPermission()

  permission.set(answer)

  if (answer === "granted") await which.set(true)
}

/** Switch both on, which is what the offer does. */
export const enableAll = async () => {
  offering.set(false)
  await setNotify(notifyPairing, true)
  await setNotify(notifyContent, true)
}

/** Put the offer away for good. */
export const declineOffer = () => {
  offering.set(false)
  offered.set(true)
}

/** Whether a preference has been written true, read from the core rather than a store that may not have loaded. */
const isSet = (key: string) =>
  Dip.preference({key})
    .then(({value}) => value === "true")
    .catch(() => true)

/** Offer notifications once the user has published writing twice, unless they have already decided. */
export const afterPublish = async (kind: number) => {
  const {identity} = get(session)

  if (!identity || !CONTENT_KINDS.includes(kind)) return

  const decided = await Promise.all(
    ["notifications.asked", "notifications.pairing", "notifications.content"].map(isSet),
  )

  if (decided.some(Boolean)) return

  const mine = await eventsOf({
    filter: JSON.stringify({kinds: CONTENT_KINDS, authors: [identity], limit: 2}),
  })

  if (mine.length >= 2) {
    offered.set(true)
    offering.set(true)
  }
}
