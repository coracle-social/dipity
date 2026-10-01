// Opening the core, which every other read here waits on.
//
// `listDetails` and friends answer only once the shell has opened the store and
// the node, so a screen's first question is not "what arrived" but "is there
// anything to ask". The five states below are the whole of that.

import {writable, type Readable} from "svelte/store"
import {nip19} from "nostr-tools"
import {Dip} from "$lib/core"

/**
 * How far the core has got, from the view's side of the bridge. `backup` is a
 * key made on this phone that nothing has taken a copy of yet.
 */
export type SessionState = "opening" | "ready" | "backup" | "absent" | "unavailable"

/** The core as the view sees it: whether it can be asked, and what it said. */
export type Session = {
  state: SessionState
  /** The identity the core started under, once it has. */
  identity?: string
}

const store = writable<Session>({state: "opening"})

export const session: Readable<Session> = store

const ready = async (state: SessionState = "ready") => {
  const {identity} = await Dip.start()

  store.set({state, identity})
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
      await ready()
    } else {
      store.set({state: "absent"})
    }
  } catch {
    store.set({state: "unavailable"})
  }
}

/** Make an identity and open the store under it, holding the app until it is written down. */
export const createIdentity = async () => {
  await Dip.createIdentity()
  await ready("backup")
}

/** The new identity has a copy somewhere, so the app opens on it. */
export const backedUp = () => store.update(session => ({...session, state: "ready"}))

/** Take an identity the user pasted in, and open the store under it. */
export const importIdentity = async (nsec: string) => {
  await Dip.importIdentity({nsec: nsec.trim()})
  await ready()
}

/**
 * Take the identity another phone handed over.
 *
 * The shell has already written the key and reopened the core under it, so this
 * answers what `start` answers and there is nothing left to open.
 */
export const takeIdentity = async (link: number) => {
  const {identity} = await Dip.takeTransferredIdentity({link})

  store.set({state: "ready", identity})
}

/** The identity as a person would copy it down. */
export const npubOf = (identity: string) => nip19.npubEncode(identity)
