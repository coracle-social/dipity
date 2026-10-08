// Moving the identity to a second phone, from either end of it.
//
// Both users act and both compare the same six digits before anything moves,
// because this is deliberate key exfiltration. Either may answer first, and the core
// releases the key once the second one has. `docs/keys.md#login-with-device`.
//
// One flow at a time, whichever phone started it. The two ends see the same
// screen and different endings — the source learns the key went, the target
// holds one it has not stored yet — so the step below carries which side this
// device is on.

import {get, writable, type Readable} from "svelte/store"
import type {PluginListenerHandle} from "@capacitor/core"
import {Dip} from "$lib/core"
import {takeIdentity} from "$lib/data/session"

/** Where the flow has got to on this device. */
export type Step =
  | {at: "idle"}
  /** The offer has gone and the core has not put the digits up yet. */
  | {at: "offering"; link: number}
  /** Both phones are showing these digits and this user has not answered. */
  | {at: "comparing"; link: number; code: number; source: boolean}
  /** This user said yes and the other one has not. */
  | {at: "waiting"; link: number; source: boolean}
  /** The key left, and the other phone is this person too. */
  | {at: "sent"}
  /** The key arrived and this phone is now the identity it carries. */
  | {at: "arrived"}
  /** Somebody said no, or the phone walked off mid-flow. */
  | {at: "refused"}
  /** The core would not run it, and said why. */
  | {at: "failed"; why: string}

const store = writable<Step>({at: "idle"})

export const step: Readable<Step> = store

const said = (error: unknown) => (error instanceof Error ? error.message : "it could not be run")

const linkOf = (step: Step) => ("link" in step ? step.link : undefined)

/** Offer this device's identity to the phone on a link. */
export const offer = async (link: number) => {
  store.set({at: "offering", link})

  try {
    await Dip.offerIdentity({link})
  } catch (error) {
    store.set({at: "failed", why: said(error)})
    console.error("that identity could not be offered", error)
  }
}

/** Answer the digits: yes they match on both phones, or no they do not. */
export const compare = async (confirmed: boolean) => {
  const asked = get(store)

  if (asked.at !== "comparing") return

  store.set(confirmed ? {at: "waiting", link: asked.link, source: asked.source} : {at: "refused"})

  try {
    await Dip.answerIdentityTransfer({link: asked.link, confirmed})
  } catch (error) {
    store.set({at: "failed", why: said(error)})
    console.error("that answer could not be recorded", error)
  }
}

/**
 * Put the screen away once the flow has ended, however the screen was left.
 *
 * A flow still running keeps its step so that leaving mid-comparison does not take
 * the question away from a user who has yet to answer it.
 */
export const clear = () => {
  if (["sent", "arrived", "refused", "failed"].includes(get(store).at)) {
    store.set({at: "idle"})
  }
}

/**
 * Watch the flow, from the shell, because two of its three endings are not
 * answers to anything a screen asked.
 *
 * An offer arriving from the other phone is the whole of the target's side, and
 * a link going down while the flow runs ends it with nothing announced. This is
 * the only notice of it, because the core has closed the session by then.
 */
export const watchTransfers = async () => {
  const handles: PluginListenerHandle[] = []

  handles.push(
    await Dip.addListener("confirmIdentityTransfer", ({link, code}) =>
      store.update(asked => ({
        at: "comparing",
        link,
        code,
        source: asked.at === "offering" && asked.link === link,
      })),
    ),
  )

  handles.push(
    await Dip.addListener("identityTransfer", ({link, outcome}) => {
      if (outcome === "received") {
        takeIdentity(link)
          .then(() => store.set({at: "arrived"}))
          .catch(error => {
            store.set({at: "failed", why: said(error)})
            console.error("the transferred identity could not be adopted", error)
          })
      } else {
        store.set(outcome === "sent" ? {at: "sent"} : {at: "refused"})
      }
    }),
  )

  handles.push(
    await Dip.addListener("linkClosed", ({link}) =>
      store.update(asked => (linkOf(asked) === link ? {at: "refused"} : asked)),
    ),
  )

  return () => {
    for (const handle of handles) handle.remove()
  }
}
