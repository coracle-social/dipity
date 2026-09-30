// The board: what has reached the device, and what people did about it.
//
// Reads rather than one per card. The page is what the user is looking at; the
// rest answer questions a count on a card cannot ask for itself — what people
// said about it, what was deleted, and which of it this device is in a position
// to pass on at all.

import {derived, get, type Readable} from "svelte/store"
import {randomId} from "@welshman/lib"
import {COMMENT, DELETE, POLL_RESPONSE, REACTION} from "@welshman/util"
import type {HashedEvent} from "@welshman/util"
import {Dip, type EventDetail, type Order} from "$lib/core"
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
  removal,
  responseKinds,
  timeEvent,
} from "$lib/kinds"

/** One stored event, and how it got here. */
export type Item = {
  event: EventDetail["event"]
  /** When it first reached this device, which is what the board is ordered by. */
  seenAt: number
  /** The most recent sighting, which is what the retention sweep measures. */
  lastSeenAt: number
  /** The peers it was seen from, earliest first. Never leaves the device. */
  from: string[]
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

/** Every response in the store, and every event a deletion asked to remove. */
export type Responses = {to: Map<string, Response>; deleted: Set<string>}

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

const toItem = ({event, sightings}: EventDetail): Item => ({
  event,
  seenAt: Math.min(...sightings.map(sighting => sighting.seen_at)),
  lastSeenAt: Math.max(...sightings.map(sighting => sighting.seen_at)),
  from: sightings.map(sighting => sighting.pubkey),
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

const index = (found: EventDetail[]): Responses => {
  const to = new Map<string, Response>()
  const deleted = new Set<string>()
  const at = (id: string) => {
    const response = to.get(id) ?? nothing()

    to.set(id, response)

    return response
  }

  for (const {event} of found) {
    if (event.kind === DELETE) {
      for (const id of removal.reader(event).parse().ids()) deleted.add(id)
    } else {
      const about = respondedTo(event)

      if (about && event.kind === REACTION) {
        at(about).reactions.push({pubkey: event.pubkey, emoji: event.content || "+"})
      } else if (about && event.kind === POLL_RESPONSE) {
        at(about).votes.push(event)
      } else if (about) {
        at(about).boosts.push(event.pubkey)
      }
    }
  }

  return {to, deleted}
}

/** Every response the store holds, which is what puts a count on a card. */
export const responses: Readable<Responses> = answering(
  storedEvents,
  () => detailsOf({order: "seenAt", filter: JSON.stringify({kinds: responseKinds})}).then(index),
  {to: new Map(), deleted: new Set()},
)

/** A page of sixty: a phone scrolls, and nothing here pages. */
const page = (asked: View): Promise<Item[]> =>
  detailsOf({
    order: asked.order,
    filter: JSON.stringify({kinds: shownKinds(asked), limit: 60}),
  }).then(found => found.map(toItem))

const arrived = answering(
  derived([view, storedEvents], ([$view]) => $view),
  page,
  [] as Item[],
)

/** What the board draws, with whatever a deletion asked to remove left out. */
export const board: Readable<Item[]> = derived([arrived, responses], ([$arrived, $responses]) =>
  $arrived.filter(item => !$responses.deleted.has(item.event.id)),
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
 * showing, the way `responses` reads every reaction: a kept item is drawn on a
 * screen of its own, and narrowing to the board's page put a zero under it.
 */
export const saying: Readable<Map<string, number>> = answering(
  storedEvents,
  () =>
    eventsOf({order: "createdAt", filter: JSON.stringify({kinds: [COMMENT]})}).then(found => {
      const counts = new Map<string, number>()

      for (const event of found) {
        const about = commentedOn(event)

        if (about) counts.set(about, (counts.get(about) ?? 0) + 1)
      }

      return counts
    }),
  new Map<string, number>(),
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
 * A bookmarked event is spared too, which is what `kept` is for.
 * `core/dip/src/db/event/command.rs`.
 */
const spared = (item: Item, identity?: string) =>
  item.event.pubkey === identity ||
  item.event.kind === 0 ||
  item.event.kind === 3 ||
  (item.event.kind >= 10_000 && item.event.kind < 20_000)

/**
 * When the sweep takes an item, or undefined for one it never will.
 *
 * `retentionDays` is read from `policy` rather than held here, so an edit on the
 * settings screen moves every ring on the board instead of waiting for the next
 * time the app opens.
 */
export const sweptAt = (item: Item, session: Session, retentionDays: number, kept = false) =>
  spared(item, session.identity) || kept ? undefined : item.lastSeenAt + retentionDays * 86_400

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

/** Ask for something the user published to be forgotten, wherever it has reached. */
export const retract = async (item: Item) =>
  publish(await removal.writer().addEvent(item.event).renderTemplate())

/**
 * Drop something off this device, and say nothing to anybody.
 *
 * What the user can do about somebody else's writing: only its author can ask
 * the network to forget it, so this is local. The sightings, the authorship
 * proof and the media go with it.
 */
export const drop = async (item: Item) => {
  await Dip.forgetEvent({id: item.event.id})
}

/** Whether an item is the user's own, which is the only thing they may retract. */
export const isMine = (item: Item, session: Session) => item.event.pubkey === session.identity

/**
 * One stored event by id, or null for a thing this device does not have.
 *
 * A boost names an event rather than embedding one, so a boost whose subject
 * never arrived is a state the card has to draw.
 */
export const heldEvent = (id: string) =>
  Dip.getEvent({id}).then(({event}) => (event ? (JSON.parse(event) as EventDetail["event"]) : null))

/** One thing and what was said about it, which is what the detail screen draws. */
export type Detail = {item?: Item; comments: HashedEvent[]}

/**
 * Everything one event's own screen needs, re-read whenever the store moves.
 *
 * What people said is the comments naming it as their parent, oldest first.
 */
export const detailOf = (id: string): Readable<Detail> =>
  answering(
    derived(storedEvents, revision => ({id, revision})),
    async () => ({
      item: await itemsByIds([id]).then(found => found[0]),
      comments: await commentsOn([id]).then(found =>
        [...found].sort((a, b) => a.created_at - b.created_at),
      ),
    }),
    {item: undefined, comments: []},
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
