// What the user bookmarked.
//
// A bookmark is the only way to spare something somebody else wrote from the
// retention sweep, which takes whatever stops circulating except what this list
// names. `docs/storage.md#retention`.
//
// The list is one replaceable event, so bookmarking is a read of the current
// one and a write of the next. It is the user's own, so nothing here reads
// anybody else's.

import {derived, get, type Readable} from "svelte/store"
import {BOOKMARKS} from "@welshman/util"
import {answering, eventsOf, storedEvents} from "$lib/data/query"
import {publish} from "$lib/data/publish"
import {session} from "$lib/data/session"
import {bookmarks as list} from "$lib/kinds"
import {itemsByIds, type Item} from "$lib/data/feed"

/** The event ids the user bookmarked, newest addition last. */
const held = async (identity?: string): Promise<string[]> => {
  if (!identity) return []

  const [current] = await eventsOf({
    filter: JSON.stringify({kinds: [BOOKMARKS], authors: [identity], limit: 1}),
  })

  return current ? (await list.reader(current).parse()).ids() : []
}

export const bookmarked: Readable<Set<string>> = answering(
  derived([session, storedEvents], ([$session, revision]) => ({
    identity: $session.identity,
    revision,
  })),
  ({identity}) => held(identity).then(ids => new Set(ids)),
  new Set<string>(),
)

export const isBookmarked = (bookmarked: Set<string>, id: string) => bookmarked.has(id)

/**
 * Bookmark an item, or remove its bookmark.
 *
 * The whole list is rewritten either way, because a replaceable event is the
 * list rather than a change to it. Reading it again here rather than off `bookmarked`
 * keeps the write based on what the store holds instead of on what a screen
 * last drew.
 */
export const toggleBookmark = async (item: Item) => {
  const {identity} = get(session)
  const [current] = await eventsOf({
    filter: JSON.stringify({kinds: [BOOKMARKS], authors: [identity], limit: 1}),
  })

  const reader = current ? await list.reader(current).parse() : undefined
  const writer = reader ? list.writer(reader) : list.writer()

  if (reader?.ids().includes(item.event.id)) {
    writer.removeBookmark(item.event.id)
  } else {
    writer.bookmarkPublicly(["e", item.event.id])
  }

  return publish(await writer.renderTemplate(), current)
}

/**
 * What the bookmarks screen draws, newest arrival first.
 *
 * Its own read rather than the board filtered: the board is a page of sixty
 * with categories switched off, and a bookmarked thing has to be there whatever the
 * user is looking at elsewhere. An id the store no longer holds simply does not
 * come back, which is what a bookmark of something dropped by hand looks like.
 */
export const bookmarkedItems: Readable<Item[]> = answering(
  derived([bookmarked, storedEvents], ([$bookmarked, revision]) => ({
    ids: [...$bookmarked],
    revision,
  })),
  ({ids}) => itemsByIds(ids),
  [] as Item[],
)
