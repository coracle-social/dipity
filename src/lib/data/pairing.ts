// Somebody in range asking to pair, and the name they get out of it.
//
// The gate runs before either side has named a pubkey, so a request carries a
// link and a code and nothing about the person. The user compares the code's
// five shapes against the other phone and types a name for whoever is standing
// there; the name is held against the link until the peer identifies itself,
// which is the only thing that says who it belongs to.
// `docs/discovery.md#the-consent-gate`.

import {readable, writable, type Readable} from "svelte/store"
import type {PluginListenerHandle} from "@capacitor/core"
import {Dip} from "$lib/core"
import {name} from "$lib/data/contacts"

/** Somebody waiting on an answer. */
export type Request = {link: number; code: number; asked: number}

const pending = writable<Request[]>([])

/**
 * Who is asking, oldest first.
 *
 * A request leaves when its link closes, which is the person walking away or
 * the gate's five-minute hold lapsing; the shell reports both. The hold is
 * also kept here, for a link whose close nobody heard.
 */
export const requests: Readable<Request[]> = readable<Request[]>([], set => {
  let live = true
  const handles: PluginListenerHandle[] = []
  const unsubscribe = pending.subscribe(set)

  const hold = (listening: Promise<PluginListenerHandle>) =>
    listening
      .then(listener => {
        handles.push(listener)

        if (!live) listener.remove()
      })
      .catch(() => undefined)

  const lapse = setInterval(
    () => pending.update(waiting => waiting.filter(({asked}) => Date.now() - asked < 5 * 60_000)),
    10_000,
  )

  hold(
    Dip.addListener("requestApproval", ({link, code}) =>
      pending.update(waiting => [...waiting, {link, code, asked: Date.now()}]),
    ),
  )

  hold(
    Dip.addListener("linkClosed", ({link}) =>
      pending.update(waiting => waiting.filter(request => request.link !== link)),
    ),
  )

  return () => {
    live = false
    clearInterval(lapse)
    unsubscribe()
    handles.forEach(handle => handle.remove())
  }
})

/** Names waiting on the peer that earned them to say who it is. */
const promised = new Map<number, string>()

const answer = async (link: number, approved: boolean) => {
  pending.update(waiting => waiting.filter(request => request.link !== link))

  await Dip.approve({link, approved})
}

/** Pair with whoever is on a link, under the name the user gave them. */
export const accept = async (link: number, petname: string) => {
  promised.set(link, petname)

  await answer(link, true)
}

/** Refuse, which the core respects for long enough that they are not asked about again. */
export const decline = (link: number) => answer(link, false)

/**
 * Bind promised names to the peers that turn out to hold them.
 *
 * Started once, from the shell, because writing the card is a publish and not a
 * screen: the user may have moved on by the time the peer identifies itself.
 *
 * A device may prove several pubkeys over one link and the announcement comes
 * once per pubkey, so the name is kept until the link is answered for rather
 * than spent on the first.
 */
export const watchPairings = async () => {
  const listener = await Dip.addListener("peerIdentified", ({link, pubkey}) => {
    const petname = promised.get(link)

    if (petname) {
      name(pubkey, petname).catch(error => console.error("the name could not be stored", error))
    }
  })

  return () => listener.remove()
}
