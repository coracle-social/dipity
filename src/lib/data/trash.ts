// What the user put in the trash.
//
// Trashing is a mark the core keeps on this phone and tells nobody about. A
// trashed thing leaves the board and waits under Trash, where it can be put
// back. Emptying the trash, by hand or once a thing has been there a week,
// deletes it: the user's own writing is retracted, and anybody else's dropped
// from this phone. `docs/storage.md#the-trash`.

import {derived, type Readable} from "svelte/store"
import {Dip} from "$lib/core"
import {answering, storedEvents} from "$lib/data/query"
import type {Item} from "$lib/data/feed"

/** How long a thing stays in the trash before it is deleted, in seconds. */
export const TRASH_SECONDS = 7 * 86_400

/** Log a trash call that failed, which a tap on a card has nowhere else to say. */
const logged = (what: string) => (error: unknown) => console.error(`${what} failed`, error)

/** What is in the trash, by id, with when each went in. */
export const trashed: Readable<Map<string, number>> = answering(
  derived(storedEvents, revision => revision),
  () =>
    Dip.trashed()
      .then(
        ({trashed}) =>
          new Map(
            trashed.map(row => {
              const {id, trashed_at} = JSON.parse(row) as {id: string; trashed_at: number}

              return [id, trashed_at] as const
            }),
          ),
      )
      .catch(error => {
        logged("reading the trash")(error)

        return new Map<string, number>()
      }),
  new Map<string, number>(),
)

/** Put a thing in the trash. */
export const trash = (item: Item) =>
  Dip.setTrashed({id: item.event.id, trashed: true}).catch(logged("trashing"))

/** Take a thing back out of the trash. */
export const restore = (item: Item) =>
  Dip.setTrashed({id: item.event.id, trashed: false}).catch(logged("restoring"))

/** Delete everything in the trash now. */
export const emptyTrash = () => Dip.emptyTrash().catch(logged("emptying the trash"))
