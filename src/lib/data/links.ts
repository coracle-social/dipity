// Which phones are in the room right now.
//
// A link is a number the shell assigned to one BLE session, and the core
// announces a pubkey against it once both ends have authenticated. That pair is
// the only handle the view has on a live peer, and everything a screen can ask
// of one is asked by link.
//
// Nothing here is remembered: a link is gone when the phone walks off, and the
// list is empty again the next time the app opens.

import {readable, type Readable} from "svelte/store"
import type {PluginListenerHandle} from "@capacitor/core"
import {Dip} from "$lib/core"

/** A phone in range that has said who it is. */
export type Link = {link: number; pubkey: string}

/**
 * The live links, oldest first.
 *
 * A device may prove several pubkeys over one link, so the newest announcement
 * for a link replaces the one before it rather than adding a row.
 */
export const links: Readable<Link[]> = readable<Link[]>([], set => {
  let live = true
  let open: Link[] = []
  const handles: PluginListenerHandle[] = []

  const hold = (listening: Promise<PluginListenerHandle>) => {
    listening
      .then(handle => {
        handles.push(handle)

        if (!live) handle.remove()
      })
      .catch(() => undefined)
  }

  const show = (next: Link[]) => {
    open = next
    set(open)
  }

  hold(
    Dip.addListener("peerIdentified", ({link, pubkey}) =>
      show([...open.filter(held => held.link !== link), {link, pubkey}]),
    ),
  )

  hold(Dip.addListener("linkClosed", ({link}) => show(open.filter(held => held.link !== link))))

  return () => {
    live = false

    for (const handle of handles) handle.remove()
  }
})
