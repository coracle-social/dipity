// Opening the core, which every other read here waits on.
//
// `listDetails` and friends answer only once the shell has opened the store and
// the node, so a screen's first question is not "what arrived" but "is there
// anything to ask". The four states below are the whole of that: the view has
// no identity to create and no store to open itself.

import {writable, type Readable} from "svelte/store"
import {Dip} from "$lib/core"

/** How far the core has got, from the view's side of the bridge. */
export type SessionState = "opening" | "ready" | "absent" | "unavailable"

/** The core as the view sees it: whether it can be asked, and what it said. */
export type Session = {
  state: SessionState
  /** The identity the core started under, once it has. */
  identity?: string
  /** `policy.retention_days`, which is what decides when an arrival is swept. */
  retentionDays: number
}

/** The core's own default, restated so a screen has a number before the read lands. */
const RETENTION_DAYS = 30

const store = writable<Session>({state: "opening", retentionDays: RETENTION_DAYS})

export const session: Readable<Session> = store

const retentionDays = async () => {
  const {value} = await Dip.preference({key: "policy.retention_days"})

  return value === null ? RETENTION_DAYS : Number(JSON.parse(value))
}

/**
 * Start the core, and say what happened.
 *
 * Never rejects: a browser has no plugin behind the proxy and a fresh device
 * has no identity, and both are screens rather than errors.
 */
export const open = async () => {
  try {
    const {exists} = await Dip.hasIdentity()

    if (exists) {
      const {identity} = await Dip.start()

      store.set({state: "ready", identity, retentionDays: await retentionDays()})
    } else {
      store.set({state: "absent", retentionDays: RETENTION_DAYS})
    }
  } catch {
    store.set({state: "unavailable", retentionDays: RETENTION_DAYS})
  }
}
