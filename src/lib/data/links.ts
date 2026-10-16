// Which phones are in the room right now.
//
// A link is a number the shell assigned to one BLE session, and the core
// announces a pubkey against it once both ends have authenticated. That pair is
// the only handle the view has on a live peer, and everything a screen can ask
// of one is asked by link.
//
// Nothing here is remembered: a link is gone when the phone walks off, and the
// list is empty again the next time the app opens.

import {writable, type Readable} from "svelte/store"
import {Dip, type PeerIdentity} from "$lib/core"

/** A phone in range that has said who it is. */
export type Link = PeerIdentity

const open = writable<Link[]>([])

/** The live links, oldest first. */
export const links: Readable<Link[]> = open

/** The newest announcement for a link replaces the one before it, because a device may prove several pubkeys over one link. */
const place = (held: Link[], peer: Link) => [...held.filter(each => each.link !== peer.link), peer]

/** Ask the core who is on each link, which is what a view that was not attached for the announcements has to do. */
const refresh = async () => {
  try {
    const {peers} = await Dip.inRange()

    open.set(peers.reduce(place, []))
  } catch (error) {
    console.error("reading who is in range failed", error)
  }
}

/**
 * Follow links as they are identified and as they close.
 *
 * Started once, from the shell, rather than by the screen that lists them,
 * because a peer is announced once and a screen opened after it would never
 * hear of it. The list is read whole at the start and whenever the app comes
 * back to the front, because the core announces a peer whether or not a view
 * is attached to hear it, and on iOS the view often is not.
 */
export const watchLinks = async () => {
  const identified = await Dip.addListener("peerIdentified", peer =>
    open.update(held => place(held, peer)),
  )
  const closed = await Dip.addListener("linkClosed", ({link}) =>
    open.update(held => held.filter(each => each.link !== link)),
  )

  const shown = () => {
    if (document.visibilityState === "visible") refresh()
  }

  document.addEventListener("visibilitychange", shown)
  await refresh()

  return () => {
    document.removeEventListener("visibilitychange", shown)
    identified.remove()
    closed.remove()
  }
}
