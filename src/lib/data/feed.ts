// The board: what has reached the device, and what people did about it.
//
// Reads rather than one per card. The page is what the user is looking at; the
// rest answer questions a count on a card cannot ask for itself — what people
// said about it, and which of it this device is in a position to pass on at all.
// A deletion needs no reading here: the core applies one its author was entitled
// to make, and the deleted event is gone from the store.

import {derived, get, writable, type Readable} from "svelte/store"
import {randomId, uniq} from "@welshman/lib"
import {COMMENT, DELETE, POLL_RESPONSE, REACTION} from "@welshman/util"
import type {HashedEvent} from "@welshman/util"
import {Dip, type EventDetail, type Order} from "$lib/core"
import {muted, social} from "$lib/data/contacts"
import {trashed} from "$lib/data/trash"
import {peopleNamed, wordsOf} from "$lib/data/search"
import {answering, detailsOf, eventsOf, remembered, storedEvents} from "$lib/data/query"
import {publish} from "$lib/data/publish"
import {session, type Session} from "$lib/data/session"
import {
  article,
  boostFor,
  boostedBy,
  categories,
  commentOn,
  commentedOn,
  note,
  poll,
  pollResponse,
  reaction,
  responseKinds,
  timeEvent,
} from "$lib/kinds"
import {CONTACT} from "$lib/kinds/contact"

/** One stored event, and how it got here. */
export type Item = {
  event: EventDetail["event"]
  /** When it first reached this device, which is what the board is ordered by. */
  seenAt: number
  /** The most recent sighting, which is what the retention sweep measures. */
  lastSeenAt: number
  /** The peers it was seen from, earliest first. Never leaves the device. */
  from: string[]
  /** The peers this device passed it to, earliest first. Never leaves the device. */
  to: string[]
}

/** How far through its retention window an item is. */
export type Warmth = "warm" | "fading" | "cold" | "kept"

/** What people did about one event. */
export type Response = {
  boosts: string[]
  /** Who reacted with what, one entry per reaction. */
  reactions: {pubkey: string; emoji: string}[]
  /** Every answer to a poll, which the reader tallies itself. */
  votes: HashedEvent[]
}

/** Every response in the store, by what it responds to. */
export type Responses = {to: Map<string, Response>}

/** What the board is showing: the order, and the categories left switched on. */
export type View = {order: Order; showing: string[]}

/**
 * Everything about one item that is not on the event itself.
 *
 * One object rather than a prop each, so a card asks for what it draws and a
 * new fact about an item does not change every call site.
 */
export type Standing = {
  response: Response
  /** How many people commented on it, which is what its own screen lists. */
  saying: number
}

const toItem = ({event, sightings, shares}: EventDetail): Item => ({
  event,
  seenAt: Math.min(...sightings.map(sighting => sighting.seen_at)),
  lastSeenAt: Math.max(...sightings.map(sighting => sighting.seen_at)),
  from: sightings.map(sighting => sighting.pubkey),
  to: shares.map(share => share.pubkey),
})

const nothing = (): Response => ({boosts: [], reactions: [], votes: []})

/**
 * The board's own controls, kept in the store so they survive being suspended.
 *
 * Switching Articles off is a decision about what the board is for rather than
 * where the user happens to be, so coming back to everything switched on again
 * is the app forgetting something it was told.
 */
export const view = remembered<View>("ui.board", {
  order: "seenAt",
  showing: categories.map(({id}) => id),
})

export const setOrder = (order: Order) => view.set({...get(view), order})

export const toggleCategory = (id: string) => {
  const {order, showing} = get(view)

  view.set({
    order,
    showing: showing.includes(id) ? showing.filter(shown => shown !== id) : [...showing, id],
  })
}

/** The kinds the switched-on categories cover. */
const shownKinds = ({showing}: View) =>
  categories.filter(({id}) => showing.includes(id)).flatMap(({kinds}) => kinds)

/** Which event a response is about, or undefined for one that names none. */
const respondedTo = (event: EventDetail["event"]): string | undefined => {
  if (event.kind === REACTION) return reaction.reader(event).parse().eventId()

  if (event.kind === POLL_RESPONSE) return pollResponse.reader(event).parse().pollId()

  return boostedBy(event)
}

/**
 * A page with everything whose subject this device does not hold left out.
 *
 * A boost carries no words of its own, so one that outran what it passes on
 * says nothing at all. A comment opens what it answers, so one whose parent
 * has not arrived opens onto nothing; it waits for the parent the way a boost
 * waits for its subject. `docs/stories.md`.
 */
const grounded = async (items: Item[]): Promise<Item[]> => {
  const subjects = new Map<string, string>()

  for (const {event} of items) {
    const about =
      commentedOn(event) ?? (responseKinds.includes(event.kind) ? respondedTo(event) : undefined)

    if (about) subjects.set(event.id, about)
  }

  if (subjects.size === 0) return items

  const found = await eventsOf({
    order: "seenAt",
    filter: JSON.stringify({ids: uniq(Array.from(subjects.values()))}),
  })
  const held = new Set(found.map(event => event.id))

  return items.filter(({event}) => {
    const about = subjects.get(event.id)

    return about === undefined || held.has(about)
  })
}

const index = (found: EventDetail[]): Responses => {
  const to = new Map<string, Response>()
  const at = (id: string) => {
    const response = to.get(id) ?? nothing()

    to.set(id, response)

    return response
  }

  for (const {event} of found) {
    const about = respondedTo(event)

    if (about && event.kind === REACTION) {
      at(about).reactions.push({pubkey: event.pubkey, emoji: event.content || "+"})
    } else if (about && event.kind === POLL_RESPONSE) {
      at(about).votes.push(event)
    } else if (about) {
      at(about).boosts.push(event.pubkey)
    }
  }

  return {to}
}

const responding = answering(
  storedEvents,
  () => detailsOf({order: "seenAt", filter: JSON.stringify({kinds: responseKinds})}),
  [] as EventDetail[],
)

/** Every response the store holds, which is what puts a count on a card, less the muted's. */
export const responses: Readable<Responses> = derived([responding, muted], ([found, $muted]) =>
  index(found.filter(({event}) => !$muted.has(event.pubkey))),
)

/**
 * What the user is looking for on the board, if anything.
 *
 * A passing question rather than a choice about what the board is for, so it
 * is not remembered as a preference the way the order and the categories are.
 */
export const search = writable("")

/** What the board asks the store for: the view, the search, and who the search names. */
type Asked = {view: View; search: string; named: string[]}

/** The page's order, by the field the store orders by. */
const before = (order: Order) => (a: Item, b: Item) =>
  order === "seenAt" ? b.seenAt - a.seenAt : b.event.created_at - a.event.created_at

/**
 * A page of sixty: a phone scrolls, and nothing here pages.
 *
 * A search matches what people wrote, and also who wrote it: a note by
 * somebody whose name matches comes back whatever it says. Names live in cards
 * rather than in the store's text index, so that half is a second query by
 * author, and the two pages merge.
 */
const page = async ({view: asked, search: words, named}: Asked): Promise<Item[]> => {
  const kinds = shownKinds(asked)
  const queries: Record<string, unknown>[] = [{kinds, limit: 60, ...(words ? {search: words} : {})}]

  if (named.length > 0) queries.push({kinds, limit: 60, authors: named})

  const found = await Promise.all(
    queries.map(filter => detailsOf({order: asked.order, filter: JSON.stringify(filter)})),
  )
  const byId = new Map(found.flat().map(detail => [detail.event.id, toItem(detail)]))

  return grounded([...byId.values()].sort(before(asked.order)).slice(0, 60))
}

const arrived = answering(
  derived([view, search, social, storedEvents], ([$view, $search, $social]): Asked => {
    const words = $search.trim()

    return {view: $view, search: words, named: words ? peopleNamed($social, wordsOf(words)) : []}
  }),
  page,
  [] as Item[],
)

/** What the board draws, with nothing by anybody the user muted and nothing in the trash. */
export const board: Readable<Item[]> = derived(
  [arrived, muted, trashed],
  ([$arrived, $muted, $trashed]) =>
    $arrived.filter(({event}) => !$muted.has(event.pubkey) && !$trashed.has(event.id)),
)

/** Stored items by id, for a screen that knows which ones it wants. */
export const itemsByIds = (ids: string[]): Promise<Item[]> =>
  ids.length === 0
    ? Promise.resolve([])
    : detailsOf({order: "seenAt", filter: JSON.stringify({ids})}).then(found => found.map(toItem))

/** The comments answering any of `ids`, which is what people said about them. */
export const commentsOn = (ids: string[]): Promise<HashedEvent[]> =>
  ids.length === 0
    ? Promise.resolve([])
    : eventsOf({
        order: "createdAt",
        filter: JSON.stringify({kinds: [COMMENT], "#e": ids}),
      })

/**
 * How many comments each stored event has.
 *
 * Every comment in the store rather than the ones answering what the board is
 * showing, the way `responses` reads every reaction: a bookmarked item is drawn on a
 * screen of its own, and narrowing to the board's page put a zero under it.
 */
export const saying: Readable<Map<string, number>> = derived(
  [
    answering(
      storedEvents,
      () => eventsOf({order: "createdAt", filter: JSON.stringify({kinds: [COMMENT]})}),
      [] as HashedEvent[],
    ),
    muted,
  ],
  ([found, $muted]) => {
    const counts = new Map<string, number>()

    for (const event of found) {
      const about = $muted.has(event.pubkey) ? undefined : commentedOn(event)

      if (about) counts.set(about, (counts.get(about) ?? 0) + 1)
    }

    return counts
  },
)

export const responseTo = (responses: Responses, id: string) => responses.to.get(id) ?? nothing()

/** Everything a card draws about one item beyond the event itself. */
export const standingOf = (
  responses: Responses,
  saying: Map<string, number>,
  id: string,
): Standing => ({
  response: responseTo(responses, id),
  saying: saying.get(id) ?? 0,
})

/**
 * Whether the retention sweep will ever take an item.
 *
 * The core's rule, restated rather than taken from `isReplaceableKind` in
 * `@welshman/util`: that one counts addressable kinds as replaceable and the
 * sweep does not, so an article would read as permanent when it is not.
 * A bookmarked event is spared too, which is what `bookmarked` is for.
 * `core/dip/src/db/event/command.rs`.
 */
const spared = (item: Item, identity?: string) =>
  item.event.pubkey === identity ||
  item.event.kind === 0 ||
  item.event.kind === 3 ||
  item.event.kind === DELETE ||
  item.event.kind === CONTACT ||
  (item.event.kind >= 10_000 && item.event.kind < 20_000)

/**
 * When the sweep takes an item, or undefined for one it never will.
 *
 * `retentionDays` is read from `policy` rather than held here, so an edit on the
 * settings screen moves every ring on the board instead of waiting for the next
 * time the app opens. It is undefined until the core has answered, and an item
 * then reads as kept rather than as fading on a window nobody confirmed.
 */
export const sweptAt = (
  item: Item,
  session: Session,
  retentionDays: number | undefined,
  bookmarked = false,
) =>
  retentionDays === undefined || spared(item, session.identity) || bookmarked
    ? undefined
    : item.lastSeenAt + retentionDays * 86_400

/** How much of an item's life is left, as the one word the screen ever says. */
export const warmthOf = (item: Item, swept: number | undefined, now: number): Warmth => {
  if (swept) {
    const spent = (now - item.lastSeenAt) / (swept - item.lastSeenAt)

    return spent < 0.5 ? "warm" : spent < 0.9 ? "fading" : "cold"
  } else {
    return "kept"
  }
}

/** Write something of the user's own. */
export const write = async (content: string) =>
  publish(await note.writer().setContent(content).renderTemplate())

/** Ask the neighbourhood something, with the answers to choose from. */
export const ask = async (title: string, options: string[]) => {
  const writer = poll.writer().setTitle(title)

  for (const label of options) writer.addOption(label)

  return publish(await writer.renderTemplate())
}

/** Put something on the calendar, at a time rather than on a date. */
export const arrange = async (title: string, at: number, where: string, about: string) => {
  const writer = timeEvent
    .writer()
    .setIdentifier(randomId())
    .setTitle(title)
    .setStart(at)
    .setContent(about)

  if (where) writer.setLocation(where)

  return publish(await writer.renderTemplate())
}

/** Write something long enough to want a title. */
export const compose = async (title: string, summary: string, body: string) => {
  const writer = article.writer().setIdentifier(randomId()).setTitle(title).setContent(body)

  if (summary) writer.setSummary(summary)

  return publish(await writer.renderTemplate())
}

/**
 * Pass something on, with or without something to say about it.
 *
 * Said nothing, and it goes on unchanged as a boost. Said something, and it is
 * a comment on the thing, which is what puts it under what it answers.
 */
export const boostItem = async (item: Item, said = "") =>
  said.trim()
    ? publish(await commentOn(item.event).setContent(said.trim()).renderTemplate())
    : publish(await boostFor(item.event.kind).writer().setEvent(item.event).renderTemplate())

/** React, unless this device already sent that reaction. */
export const react = async (item: Item, emoji: string) => {
  const mine = get(session).identity
  const already = responseTo(get(responses), item.event.id).reactions.some(
    sent => sent.pubkey === mine && sent.emoji === emoji,
  )

  if (!already) {
    await publish(await reaction.writer().setEvent(item.event).setContent(emoji).renderTemplate())
  }
}

/**
 * Answer a poll, replacing whatever this device answered before.
 *
 * The reader keeps only each pubkey's newest response, so a change of mind is
 * another event rather than an edit.
 */
export const answer = async (item: Item, selections: string[]) => {
  const writer = pollResponse.writer().setPollId(item.event.id)

  for (const id of selections) writer.addSelection(id)

  return publish(await writer.renderTemplate())
}

/** Whether an item is the user's own. */
export const isMine = (item: Item, session: Session) => item.event.pubkey === session.identity

/** One stored event by id, or null for a thing this device does not have. */
const heldEvent = (id: string) =>
  Dip.getEvent({id}).then(({event}) => (event ? (JSON.parse(event) as EventDetail["event"]) : null))

/** One thing and what was said about it, which is what the detail screen draws. */
export type Detail = {item?: Item; comments: HashedEvent[]}

/**
 * Everything one event's own screen needs, re-read whenever the store moves.
 *
 * What people said is the comments naming it as their parent, oldest first,
 * less any by somebody the user muted.
 */
export const detailOf = (id: string): Readable<Detail> =>
  derived(
    [
      answering(
        derived(storedEvents, revision => ({id, revision})),
        async () => ({
          item: await itemsByIds([id]).then(found => found[0]),
          comments: await commentsOn([id]).then(found =>
            [...found].sort((a, b) => a.created_at - b.created_at),
          ),
        }),
        {item: undefined, comments: []} as Detail,
      ),
      muted,
    ],
    ([detail, $muted]) => ({
      ...detail,
      comments: detail.comments.filter(({pubkey}) => !$muted.has(pubkey)),
    }),
  )

/**
 * One stored event by id, re-read whenever the store moves.
 *
 * What a line stands for can arrive after the line was drawn, so reading it
 * once says the thing is missing for as long as the screen stays open.
 */
export const heldEventOf = (id: string): Readable<EventDetail["event"] | null | undefined> =>
  answering(
    derived(storedEvents, revision => ({id, revision})),
    () => heldEvent(id),
    undefined,
  )

/** The id of the current event at an address, live: `null` when this device holds none. */
export const heldAtAddress = (kind: number, pubkey: string, identifier: string) =>
  answering(
    derived(storedEvents, revision => ({kind, pubkey, identifier, revision})),
    () =>
      eventsOf({
        filter: JSON.stringify({kinds: [kind], authors: [pubkey], "#d": [identifier], limit: 1}),
      }).then(([event]) => event?.id ?? null),
    undefined as string | null | undefined,
  )

/**
 * The page a card opens, which is its own only when it is about nothing else.
 *
 * A boost carries no words of its own and a comment belongs under what it
 * answers, so both open their subject. A comment on a comment opens that
 * comment, which is the page the conversation above it is drawn on.
 */
export const opensId = (item: Item) =>
  boostedBy(item.event) ?? commentedOn(item.event) ?? item.event.id
