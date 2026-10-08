// Opening the core, which every other read here waits on.
//
// A screen's first question is not "what arrived" but "is there anything to
// ask", because `listDetails` and friends answer only once the shell has opened
// the store and the node. The four states below are the whole of that.

import {writable, type Readable} from "svelte/store"
import {nip19} from "nostr-tools"
import {Dip} from "$lib/core"
import {reset} from "$lib/data/nav"

/** How far the core has got, from the view's side of the bridge. */
export type SessionState = "opening" | "ready" | "absent" | "unavailable"

/** The core as the view sees it: whether it can be asked, and what it said. */
export type Session = {
  state: SessionState
  /** The identity the core started under, once it has. */
  identity?: string
  /** What the shell said when the core could not be reached. */
  why?: string
  /** The identity is a stand-in, made only so the user's other phone can hand over theirs. */
  receiving?: boolean
}

const store = writable<Session>({state: "opening"})

export const session: Readable<Session> = store

const ready = async () => {
  const {identity} = await Dip.start()

  store.set({state: "ready", identity})
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
  } catch (error) {
    console.error("the core could not be opened", error)
    store.set({state: "unavailable", why: error instanceof Error ? error.message : String(error)})
  }
}

/** Make an identity and open the store under it. */
export const createIdentity = async () => {
  await Dip.createIdentity()
  await ready()
}

/** Take an identity the user pasted in, and open the store under it. */
export const importIdentity = async (nsec: string) => {
  await Dip.importIdentity({nsec: nsec.trim()})
  await ready()
}

/**
 * Make a stand-in identity and wait on the device screen for the user's other
 * phone to replace it.
 *
 * A phone needs a key to authenticate the session a key arrives over, and only
 * one that has published nothing under its own will take another.
 * `docs/keys.md#login-with-device`.
 */
export const awaitIdentity = async () => {
  await Dip.createIdentity()

  const {identity} = await Dip.start()

  reset({at: "device"})
  store.set({state: "ready", identity, receiving: true})
}

/** Erase the key and everything gathered under it, and go back to first run. */
export const logOut = async () => {
  await Dip.deleteIdentity()

  reset({at: "board"})
  store.set({state: "absent"})
}

/**
 * Take the identity another phone handed over.
 *
 * The shell has already written the key and reopened the core under it. This
 * answers what `start` answers, and there is nothing left to open.
 */
export const takeIdentity = async (link: number) => {
  const {identity} = await Dip.takeTransferredIdentity({link})

  store.set({state: "ready", identity})
}

/** The identity as a person would copy it down. */
export const npubOf = (identity: string) => nip19.npubEncode(identity)
