// What the user put in the trash.
//
// Trashing is a mark the core keeps on this phone. A trashed thing leaves the
// board and waits under Trash, where it can be put back. The user's own writing
// is retracted with a kind 5 as it goes in, and a kind 5 that arrives puts what
// it names in here too. Emptying the trash, by hand or once a thing has been
// there a week, drops it from this phone. `docs/storage.md#the-trash`.

import {derived, type Readable} from "svelte/store"
import {Dip} from "$lib/core"
import {answering, storedEvents} from "$lib/data/query"
import type {Item} from "$lib/data/feed"

/** How long a thing stays in the trash before it is deleted, in seconds. */
export const TRASH_SECONDS = 7 * 86_400

/** Log a trash call that failed, which a tap on a card has nowhere else to say. */
const logged = (what: string) => (error: unknown) => console.error(`${what} failed`, error)

/** One thing in the trash: when it went in, and whether its author retracted it. */
export type Trashed = {at: number; retracted: boolean}

/** What is in the trash, by id. */
export const trashed: Readable<Map<string, Trashed>> = answering(
  derived(storedEvents, revision => revision),
  () =>
    Dip.trashed()
      .then(
        ({trashed}) =>
          new Map(
            trashed.map(row => {
              const {id, trashed_at, retracted} = JSON.parse(row) as {
                id: string
                trashed_at: number
                retracted: boolean
              }

              return [id, {at: trashed_at, retracted}] as const
            }),
          ),
      )
      .catch(error => {
        logged("reading the trash")(error)

        return new Map<string, Trashed>()
      }),
  new Map<string, Trashed>(),
)

/** Put a thing in the trash. */
export const trash = (item: Item) => Dip.trash({id: item.event.id}).catch(logged("trashing"))

/** Take a thing back out of the trash, which the core refuses for somebody else's retracted thing. */
export const restore = (item: Item) => Dip.restore({id: item.event.id}).catch(logged("restoring"))

/** Delete everything in the trash now. */
export const emptyTrash = () => Dip.emptyTrash().catch(logged("emptying the trash"))
