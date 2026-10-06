// Somebody in range asking to pair, and the name they get out of it.
//
// A request comes one of two ways. The gate holds a stranger before either side
// has named a pubkey, so that request carries a link and a code and nothing
// about the person; the name typed for it is held against the link until the
// peer identifies itself. Or the gate let a stranger through, and the request
// comes once they have identified, carrying their pubkey: nothing waits on it,
// and answering it names them. Either way the user compares the same five
// shapes against the other phone, and a name is what lets them set anything
// about that person. Somebody the user already named is asked about too when
// this phone did not recognize them, since they have forgotten the user and
// are being asked on their own phone. `docs/discovery.md#the-consent-gate`,
// `docs/discovery.md#meeting-somebody`.

import {get, readable, writable, type Readable} from "svelte/store"
import type {PluginListenerHandle} from "@capacitor/core"
import {Dip} from "$lib/core"
import {name, social, type Social} from "$lib/data/contacts"
import {session} from "$lib/data/session"

/**
 * Somebody waiting on an answer. `held` is set when the gate is holding them,
 * which is a request to admit them as well as to name them. `pubkey` is set
 * once they have said who they are, which a dialer does before the gate holds
 * them. `known` is the user's name for somebody they already named, who
 * forgot them. `formerly` holds the links it moved off, so a screen opened on
 * one still finds it.
 */
export type Request = {
  link: number
  code: number
  asked: number
  held?: boolean
  pubkey?: string
  known?: string
  formerly?: number[]
}

/** The request a screen opened on `link` is about, wherever it has moved since. */
export const requestOn = (waiting: Request[], link: number) =>
  waiting.find(request => request.link === link || request.formerly?.includes(link))

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
        waiting.filter(({asked, held}) => !held || Date.now() - asked < 5 * 60_000),
      ),
    10_000,
  )

  hold(
    Dip.addListener("requestApproval", ({link, code}) =>
      pending.update(waiting => [...waiting, {link, code, asked: Date.now(), held: true}]),
    ),
  )

  // Every identified link, so a request can move to another link to the same person when its own one closes.
  const identified = new Map<number, {pubkey: string; code: number; dialed: boolean}>()

  // Two links to one person resolve to the one the lower pubkey dialed; both phones pick it, so both show its shapes.
  const dialer = (link: number) => {
    const peer = identified.get(link)

    return peer && (peer.dialed ? (get(session).identity ?? "") : peer.pubkey)
  }

  const prefer = (request: Request, link: number, code: number): Request => {
    const ours = dialer(link)
    const theirs = dialer(request.link)

    return ours !== undefined && theirs !== undefined && ours < theirs
      ? {...request, link, code, formerly: [...(request.formerly ?? []), request.link]}
      : request
  }

  // One request per link and per person: a device may prove several pubkeys, and two phones may briefly hold two links.
  hold(
    Dip.addListener("peerIdentified", ({link, pubkey, code, dialed, recognized}) => {
      identified.set(link, {pubkey, code, dialed})

      if (promised.has(link) || (!unnamed(known, pubkey) && recognized)) return
      if (pubkey === get(session).identity) return

      const named = known.people.get(pubkey)?.petname

      pending.update(waiting => {
        // A held dialer names itself before the hold, so its request learns who it is and stands for them.
        if (waiting.some(request => request.link === link && request.held)) {
          return waiting
            .filter(request => request.held || request.pubkey !== pubkey)
            .map(request => (request.link === link ? {...request, pubkey, known: named} : request))
        }

        if (waiting.some(request => request.link === link)) return waiting

        if (waiting.some(request => request.pubkey === pubkey)) {
          return waiting.map(request =>
            request.pubkey === pubkey ? prefer(request, link, code) : request,
          )
        }

        return [...waiting, {link, code, pubkey, known: named, asked: Date.now()}]
      })
    }),
  )

  hold(
    Dip.addListener("linkClosed", ({link}) => {
      identified.delete(link)

      pending.update(waiting =>
        waiting.flatMap(request => {
          if (request.link !== link) return [request]

          // The person is still here over another link: the request stays, under that link's shapes.
          const still = [...identified].find(([, peer]) => peer.pubkey === request.pubkey)

          return still && request.pubkey
            ? [
                {
                  ...request,
                  held: false,
                  link: still[0],
                  code: still[1].code,
                  formerly: [...(request.formerly ?? []), link],
                },
              ]
            : []
        }),
      )
    }),
  )

  return () => {
    live = false
    clearInterval(lapse)
    unsubscribe()
    unwatch()
    handles.forEach(handle => handle.remove())
  }
})

const waitingOn = (link: number) => requestOn(get(pending), link)

/** Pair with whoever is on a link, under the name the user gave them. */
export const accept = async (link: number, petname: string) => {
  const request = waitingOn(link)

  drop(request?.link ?? link)

  if (request?.held) {
    if (request.pubkey && petname !== request.known) await name(request.pubkey, petname)
    else if (!request.pubkey) promised.set(link, petname)

    return Dip.approve({link: request.link, approved: true})
  }

  if (request?.pubkey && petname !== request.known) return name(request.pubkey, petname)
}

/**
 * Say no. A held stranger is refused, which the core respects for long enough
 * that they are not asked about again; one already through is left unnamed, and
 * asked about at the next meeting.
 */
export const decline = async (link: number) => {
  const request = waitingOn(link)

  drop(request?.link ?? link)

  if (request?.held) await Dip.approve({link: request.link, approved: false})
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
