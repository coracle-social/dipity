// Asking the core something, and asking it again when the answer changed.
//
// The core announces the group of tables that moved and not the rows, so a
// query is the only thing that knows what belongs on screen: everything here
// re-reads rather than patches. `docs/ui.md#components-do-not-query-the-core`.

import {derived, readable, writable, type Readable} from "svelte/store"
import type {PluginListenerHandle} from "@capacitor/core"
import type {HashedEvent} from "@welshman/util"
import {Dip, type Change, type EventDetail, type Query} from "$lib/core"

/** A counter the core bumps whenever that group of tables moved. */
const revision = (group: Change["group"]): Readable<number> =>
  readable(0, set => {
    let live = true
    let handle: PluginListenerHandle | undefined
    let seen = 0

    Dip.addListener("storeChanged", change => {
      if (change.group === group) set((seen += 1))
    })
      .then(listener => {
        handle = listener

        if (!live) listener.remove()
      })
      .catch(() => undefined)

    return () => {
      live = false
      handle?.remove()
    }
  })

export const storedEvents = revision("events")

export const storedPreferences = revision("preferences")

/**
 * A store whose value is whatever the last read answered.
 *
 * Reads are dropped rather than applied out of order: a filter the user has
 * already moved past must not overwrite the one they are looking at.
 */
export const answering = <Asked, Answer>(
  source: Readable<Asked>,
  read: (asked: Asked) => Promise<Answer>,
  initial: Answer,
): Readable<Answer> => {
  let pending = 0

  return derived(
    source,
    (asked: Asked, set: (answer: Answer) => void) => {
      const mine = (pending += 1)

      read(asked).then(answer => {
        if (mine === pending) set(answer)
      })
    },
    initial,
  )
}

/** One stored preference, as the document it was written as. */
export const parsed = <Value>(key: string, value: string | null, fallback: Value): Value => {
  if (value === null) return fallback

  try {
    return JSON.parse(value) as Value
  } catch {
    console.error(`${key} is not a JSON document`, value)

    return fallback
  }
}

/**
 * A choice the user made, kept where it survives the app being suspended.
 *
 * `docs/ui.md#held-by-review` puts durable state in the plugin rather than in
 * the view, and a preference is the plugin's own vocabulary for one. A value set
 * here is answered at once, ahead of the store's round trip, so two changes in
 * quick succession build on each other rather than both on the value before.
 */
export const remembered = <Value>(key: string, fallback: Value) => {
  const read = () =>
    Dip.preference({key})
      .catch(() => ({value: null}))
      .then(({value}) => parsed(key, value, fallback))

  const local = writable<{value: Value} | undefined>(undefined)
  const stored = answering(storedPreferences, read, fallback)
  const {subscribe} = derived([stored, local], ([$stored, $local]) =>
    $local ? $local.value : $stored,
  )

  return {
    subscribe,
    set: (value: Value) => {
      local.set({value})

      return Dip.setPreference({key, value: JSON.stringify(value)}).catch(error => {
        local.set(undefined)
        console.error(`${key} could not be written`, error)
      })
    },
  }
}

/** A read that failed is an empty screen, since the query is the only thing that knows better. */
const unread = (error: unknown) => {
  console.error("the store could not be read", error)

  return []
}

/** Stored events matching a query, each with its media and how it got here. */
export const detailsOf = (query: Query): Promise<EventDetail[]> =>
  Dip.listDetails(query)
    .then(({details}) => details.map(detail => JSON.parse(detail) as EventDetail))
    .catch(unread)

/** The same events without their provenance, for a read that only needs the tags. */
export const eventsOf = (query: Query): Promise<HashedEvent[]> =>
  Dip.listEvents(query)
    .then(({events}) => events.map(event => JSON.parse(event) as HashedEvent))
    .catch(unread)
