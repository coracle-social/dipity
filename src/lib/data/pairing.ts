// Somebody in range asking to pair, and the name they get out of it.
//
// A request comes one of two ways. The gate holds a stranger before either side
// has named a pubkey, so that request carries a link and a code and nothing
// about the person; the name typed for it is held against the link until the
// peer identifies itself. Or the gate let a stranger through, and the request
// comes once they have identified, carrying their pubkey: nothing waits on it,
// and answering it names them. Either way the user compares the same five
// shapes against the other phone, and a name is what lets them set anything
// about that person. `docs/discovery.md#the-consent-gate`,
// `docs/discovery.md#meeting-somebody`.

import {get, readable, writable, type Readable} from "svelte/store"
import type {PluginListenerHandle} from "@capacitor/core"
import {Dip} from "$lib/core"
import {name, social, type Social} from "$lib/data/contacts"
import {session} from "$lib/data/session"

/**
 * Somebody waiting on an answer. `pubkey` is set when the gate already let them
 * through, which is a request to name them rather than to admit them.
 */
export type Request = {link: number; code: number; asked: number; pubkey?: string}

const pending = writable<Request[]>([])

/** Names waiting on the peer that earned them to say who it is. */
const promised = new Map<number, string>()

const drop = (link: number) =>
  pending.update(waiting => waiting.filter(request => request.link !== link))

/** Whether somebody is a stranger to the user: not them, and not anybody they have named. */
const unnamed = (known: Social, pubkey: string) =>
  pubkey !== get(session).identity && !known.people.get(pubkey)?.petname

/**
 * Who is asking, oldest first.
 *
 * A request leaves when its link closes, which is the person walking away or a
 * held gate lapsing; the shell reports both. A held gate's five-minute hold is
 * also kept here, for a link whose close nobody heard.
 */
export const requests: Readable<Request[]> = readable<Request[]>([], set => {
  let live = true
  const handles: PluginListenerHandle[] = []
  const unsubscribe = pending.subscribe(set)

  // Kept live rather than read on demand: an unwatched read answers the empty list it starts from.
  let known!: Social
  const unwatch = social.subscribe(value => (known = value))

  const hold = (listening: Promise<PluginListenerHandle>) =>
    listening
      .then(listener => {
        handles.push(listener)

        if (!live) listener.remove()
      })
      .catch(() => undefined)

  const lapse = setInterval(
    () =>
      pending.update(waiting =>
        waiting.filter(({asked, pubkey}) => pubkey || Date.now() - asked < 5 * 60_000),
      ),
    10_000,
  )

  hold(
    Dip.addListener("requestApproval", ({link, code}) =>
      pending.update(waiting => [...waiting, {link, code, asked: Date.now()}]),
    ),
  )

  // A device proving several pubkeys is one person, so a link is asked about once.
  hold(
    Dip.addListener("peerIdentified", ({link, pubkey, code}) => {
      if (promised.has(link) || !unnamed(known, pubkey)) return

      pending.update(waiting =>
        waiting.some(request => request.link === link)
          ? waiting
          : [...waiting, {link, code, pubkey, asked: Date.now()}],
      )
    }),
  )

  hold(Dip.addListener("linkClosed", ({link}) => drop(link)))

  return () => {
    live = false
    clearInterval(lapse)
    unsubscribe()
    unwatch()
    handles.forEach(handle => handle.remove())
  }
})

const requestOn = (link: number) => get(pending).find(request => request.link === link)

/** Pair with whoever is on a link, under the name the user gave them. */
export const accept = async (link: number, petname: string) => {
  const pubkey = requestOn(link)?.pubkey

  drop(link)

  if (pubkey) return name(pubkey, petname)

  promised.set(link, petname)

  await Dip.approve({link, approved: true})
}

/**
 * Say no. A held stranger is refused, which the core respects for long enough
 * that they are not asked about again; one already through is left unnamed, and
 * asked about at the next meeting.
 */
export const decline = async (link: number) => {
  const pubkey = requestOn(link)?.pubkey

  drop(link)

  if (!pubkey) await Dip.approve({link, approved: false})
}

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
