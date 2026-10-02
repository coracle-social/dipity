// Where the user is, and how they leave.
//
// View state, so it survives nothing — the app is suspended in the background
// and anything that has to outlive that lives in the core.
// `docs/ui.md#held-by-review`.
//
// The trail is what back walks, and there is one of it: the button on a screen
// and the phone's own button leave by the same door. Android closes an app on a
// back press nobody claims, so the shell is told whether there is anywhere to go
// before the press rather than asked during one — at the root the press stays
// Android's and the app closes the way it always has.

import {derived, get, writable} from "svelte/store"
import {tick} from "svelte"
import {Dip} from "$lib/core"

/** The four places the bottom bar goes, plus the four that open over them. */
export type Place =
  | {at: "board"}
  | {at: "bookmarks"}
  | {at: "people"}
  | {at: "settings"}
  | {at: "contact"; pubkey: string}
  | {at: "item"; id: string}
  | {at: "pairing"; link: number}
  | {at: "device"}

export const place = writable<Place>({at: "board"})

/** Where the user has been, newest last, each with how far down it they were. */
const trail = writable<{place: Place; scroll: number}[]>([])

/** What is open over the screen, newest last, each knowing how to close itself. */
const overlays = writable<(() => void)[]>([])

/** Whether back has anywhere to go, which is the whole of what the shell needs. */
const canGoBack = derived(
  [trail, overlays],
  ([$trail, $overlays]) => $trail.length + $overlays.length > 0,
)

/** What a screen is about, when it is about something. */
const about = (place: Place) => {
  if (place.at === "contact") return place.pubkey
  if (place.at === "item") return place.id
  if (place.at === "pairing") return place.link

  return undefined
}

const same = (a: Place, b: Place) => a.at === b.at && about(a) === about(b)

/**
 * Put the page back where it was, once there is page enough to hold it.
 *
 * A screen re-reads its query as it mounts, so its content lands a frame or two
 * after the place changes and scrolling any earlier clamps to the top.
 */
const restore = (scroll: number) => {
  let frames = 0

  const settle = () => {
    window.scrollTo(0, scroll)

    if (window.scrollY < scroll && frames++ < 30) requestAnimationFrame(settle)
  }

  settle()
}

/**
 * Go somewhere, remembering where the user was.
 *
 * Forward opens at the top. A screen reached by tapping a thing is about that
 * thing, and how far the user had read is theirs to come back to rather than to
 * arrive in. Going where they already are is not a move.
 */
export const go = (to: Place) => {
  const here = get(place)

  if (same(here, to)) return

  trail.update(held => [...held, {place: here, scroll: window.scrollY}])
  place.set(to)

  void tick().then(() => window.scrollTo(0, 0))
}

/**
 * Move to a sibling of where the user is, such as the next of several pairing
 * requests, without making it a step back has to retrace.
 */
export const swap = (to: Place) => {
  place.set(to)

  void tick().then(() => window.scrollTo(0, 0))
}

/**
 * Leave the top thing: whatever is open over the screen, then the screen.
 *
 * Answers whether there was anything to leave, so a caller at the root can tell.
 */
export const back = () => {
  const open = get(overlays).at(-1)

  if (open) {
    overlays.update(rest => rest.filter(close => close !== open))
    open()

    return true
  }

  const held = get(trail).at(-1)

  if (!held) return false

  trail.update(rest => rest.slice(0, -1))
  place.set(held.place)

  void tick().then(() => restore(held.scroll))

  return true
}

/**
 * Have back close this while it is open, and answer how to stop saying so.
 *
 * A drawer sits over the screen, so back closes the drawer and leaves the screen
 * under it where it is.
 */
export const dismissable = (close: () => void) => {
  overlays.update(open => [...open, close])

  return () => overlays.update(open => open.filter(held => held !== close))
}

/** Which bar item is lit: every screen that opens over one sits under it. */
export const tabOf = (place: Place) => {
  if (place.at === "contact") return "people"
  if (place.at === "device") return "settings"

  return place.at === "pairing" || place.at === "item" ? "board" : place.at
}

/**
 * Hand the phone's back button to the trail.
 *
 * Started once, from the app root rather than the shell: the button is there
 * before there is an identity to open a core under, and backing out of the
 * first-run form is going back like anything else.
 */
export const watchBack = async () => {
  const handle = await Dip.addListener("backPressed", () => {
    back()
  })

  const unsubscribe = canGoBack.subscribe(can => {
    Dip.setCanGoBack({can}).catch(() => undefined)
  })

  return () => {
    unsubscribe()
    handle.remove()
  }
}
