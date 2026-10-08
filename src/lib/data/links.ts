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
import {Dip} from "$lib/core"

/** A phone in range that has said who it is. */
export type Link = {link: number; pubkey: string}

const open = writable<Link[]>([])

/** The live links, oldest first. */
export const links: Readable<Link[]> = open

/**
 * Follow links as they are identified and as they close.
 *
 * Started once, from the shell, rather than by the screen that lists them,
 * because a peer is announced once and a screen opened after it would never
 * hear of it. The newest announcement for a link replaces the one before it
 * rather than adding a row, because a device may prove several pubkeys over
 * one link.
 */
export const watchLinks = async () => {
  const identified = await Dip.addListener("peerIdentified", ({link, pubkey}) =>
    open.update(held => [...held.filter(each => each.link !== link), {link, pubkey}]),
  )
  const closed = await Dip.addListener("linkClosed", ({link}) =>
    open.update(held => held.filter(each => each.link !== link)),
  )

  return () => {
    identified.remove()
    closed.remove()
  }
}
