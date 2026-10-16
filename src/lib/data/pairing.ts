// Asking to pair, and being asked: the requests in the tray and the name each
// one gets out of it.
//
// A request is made from a phone, by the person holding it, rather than by a
// phone noticing a stranger. Pressing add contact asks everybody in range the
// user has not named, and their phones show the request. Opening one asks back
// over that link, so that the first phone shows the same six digits while the
// second user compares them. Either user names the other once the digits
// match, and a name is what lets them set anything about that person. Somebody
// the user already named asks the same way when their phone has forgotten the
// user. `docs/discovery.md#meeting-somebody`.

import {get, readable, writable, type Readable} from "svelte/store"
import type {PluginListenerHandle} from "@capacitor/core"
import {Dip} from "$lib/core"
import {name, social, type Social} from "$lib/data/contacts"
import {links, type Link} from "$lib/data/links"
import {go} from "$lib/data/nav"
import {session} from "$lib/data/session"

/**
 * Somebody waiting on an answer. `known` is the user's name for somebody they
 * already named, who forgot them. `formerly` holds the links it moved off so
 * that a screen opened on one still finds it.
 */
export type Request = {
  link: number
  code: number
  pubkey: string
  asked: number
  known?: string
  formerly?: number[]
}

/** The request a screen opened on `link` is about, wherever it has moved since. */
export const requestOn = (waiting: Request[], link: number) =>
  waiting.find(request => request.link === link || request.formerly?.includes(link))

const pending = writable<Request[]>([])

/** People answered this session, who are not asked about again until they leave and come back. */
const answered = new Set<string>()

/** Links asked back over, once each, so that the other phone shows its digits. */
const looked = new Set<number>()

const drop = (link: number) =>
  pending.update(waiting => waiting.filter(request => request.link !== link))

/** Whether somebody is a stranger to the user: not them, and not anybody they have named. */
const unnamed = (known: Social, pubkey: string) =>
  pubkey !== get(session).identity && !known.people.get(pubkey)?.petname

/**
 * Who is asking, oldest first.
 *
 * A request leaves when its link closes, which is the person walking away, or
 * when the user answers it.
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

  // Every identified link so that a request can move to another link to the same person when its own one closes.
  const identified = new Map<number, Link>()

  // Two links to one person resolve to the one the lower pubkey dialed; both phones pick it and show its digits.
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

  // A link that left the list closed, whoever was on it and whether or not this view heard the announcements.
  const closed = (link: number) => {
    const gone = identified.get(link)

    identified.delete(link)
    looked.delete(link)

    // Somebody who left may ask again when they are back.
    if (gone && ![...identified.values()].some(peer => peer.pubkey === gone.pubkey)) {
      answered.delete(gone.pubkey)
    }

    pending.update(waiting =>
      waiting.flatMap(request => {
        if (request.link !== link) return [request]

        // The person is still here over another link: the request stays, under that link's digits.
        const still = [...identified].find(([, peer]) => peer.pubkey === request.pubkey)

        return still
          ? [
              {
                ...request,
                link: still[0],
                code: still[1].code,
                formerly: [...(request.formerly ?? []), link],
              },
            ]
          : []
      }),
    )
  }

  // The links store is the one reading of who is in range, kept whole by the shell.
  const following = links.subscribe(present => {
    const open = new Set(present.map(peer => peer.link))

    for (const link of [...identified.keys()]) {
      if (!open.has(link)) closed(link)
    }

    for (const peer of present) identified.set(peer.link, peer)
  })

  // One request per person: a device may prove several pubkeys, and two phones may briefly hold two links.
  hold(
    Dip.addListener("pairingRequested", ({link, pubkey, code}) => {
      if (pubkey === get(session).identity || answered.has(pubkey)) return

      // A contact this phone recognized is asking nothing new; one it did not has forgotten the user.
      if (!unnamed(known, pubkey) && identified.get(link)?.recognized) return

      const named = known.people.get(pubkey)?.petname

      pending.update(waiting => {
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

  return () => {
    live = false
    unsubscribe()
    unwatch()
    following()
    handles.forEach(handle => handle.remove())
  }
})

/** Whether the user pressed add contact and nobody has answered yet. */
export const seeking = writable(false)

/** Ask everybody in range the user has not named, and wait on the pairing screen for an answer. */
export const seek = async () => {
  seeking.set(true)
  go({at: "pairing"})

  await Dip.requestPairing({}).catch(error => console.error("asking to pair failed", error))
}

/** The user left the pairing screen, answered or not, so nobody who syncs from now on is asked. */
export const stopSeeking = async () => {
  seeking.set(false)

  await Dip.stopPairing().catch(error => console.error("ending the wait failed", error))
}

/**
 * The user is looking at a request. Ask back over its link, once, so that the
 * other phone stops waiting and shows the digits this one does.
 */
export const look = async (link: number) => {
  if (looked.has(link)) return

  looked.add(link)

  await Dip.requestPairing({link}).catch(error => console.error("asking to pair failed", error))
}

const waitingOn = (link: number) => requestOn(get(pending), link)

/** Pair with whoever is on a link, under the name the user gave them. */
export const accept = async (link: number, petname: string) => {
  const request = waitingOn(link)

  if (!request) return

  answered.add(request.pubkey)
  drop(request.link)

  if (petname !== request.known) await name(request.pubkey, petname)
}

/** Say no. They are left unnamed, and not asked about again until they leave and come back. */
export const decline = (link: number) => {
  const request = waitingOn(link)

  if (!request) return

  answered.add(request.pubkey)
  drop(request.link)
}
