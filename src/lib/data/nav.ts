// Where the user is. View state, so it survives nothing — the app is suspended
// in the background and anything that has to outlive that lives in the core.
// `docs/ui.md#held-by-review`.

import {get, writable} from "svelte/store"
import {tick} from "svelte"

/** The four places the bottom bar goes, plus the four that open over them. */
export type Place =
  | {at: "board"}
  | {at: "kept"}
  | {at: "people"}
  | {at: "settings"}
  | {at: "contact"; pubkey: string}
  | {at: "item"; id: string}
  | {at: "pairing"; link: number}
  | {at: "device"}

export const place = writable<Place>({at: "board"})

/** How far down the board the user had read, so going back lands where they left. */
let read = 0

/**
 * Go somewhere, holding the board's place.
 *
 * Everything else opens at the top. A screen reached by tapping a thing is about
 * that thing, and the board is the only one a person is partway through.
 */
export const go = (to: Place) => {
  if (get(place).at === "board") read = window.scrollY

  place.set(to)

  void tick().then(() => window.scrollTo(0, to.at === "board" ? read : 0))
}

/** Which bar item is lit: every screen that opens over one sits under it. */
export const tabOf = (place: Place) => {
  if (place.at === "contact") return "people"
  if (place.at === "device") return "settings"

  return place.at === "pairing" || place.at === "item" ? "board" : place.at
}
