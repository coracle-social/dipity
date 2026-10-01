// Every kind the app reads or writes, bound once.
//
// A reader is the only thing that touches tags, and a component reaches for one
// of the configured kinds below rather than for a tag name.
// `docs/ui.md#domain-kinds`.
//
// The resolver answers no relays, which is the truth rather than a stub: there
// are none, a writer's routing half is never asked, and publishing is one call
// to the core.

import CalendarDays from "@lucide/svelte/icons/calendar-days"
import ListChecks from "@lucide/svelte/icons/list-checks"
import MessageCircle from "@lucide/svelte/icons/message-circle"
import Newspaper from "@lucide/svelte/icons/newspaper"
import Reply from "@lucide/svelte/icons/reply"
import Repeat from "@lucide/svelte/icons/repeat-2"
import type {Component} from "svelte"
import {
  Article,
  BookmarkList,
  Comment,
  DateEvent,
  Delete,
  MuteList,
  Note,
  Poll,
  PollResponse,
  Reaction,
  TimeEvent,
  type KindContext,
} from "@welshman/domain"
import {
  COMMENT,
  EVENT_DATE,
  EVENT_TIME,
  GENERIC_REPOST,
  LONG_FORM,
  NOTE,
  POLL,
  POLL_RESPONSE,
  REACTION,
  REPOST,
  Resolver,
  type HashedEvent,
} from "@welshman/util"
import {ContactCard} from "$lib/kinds/contact"
import {Block, Trust} from "$lib/kinds/people"
import {Boost, GenericBoost} from "$lib/kinds/repost"

const context: KindContext = {resolver: new Resolver(() => [])}

export const note = Note.configure(context)

/** What somebody said about something already stored, which is how people answer each other here. */
export const comment = Comment.configure(context)

export const reaction = Reaction.configure(context)

export const removal = Delete.configure(context)

export const poll = Poll.configure(context)

export const pollResponse = PollResponse.configure(context)

export const article = Article.configure(context)

export const boost = Boost.configure(context)

export const genericBoost = GenericBoost.configure(context)

export const dateEvent = DateEvent.configure(context)

export const timeEvent = TimeEvent.configure(context)

/** What somebody calls somebody else, which is the only name anybody has here. */
export const contactCard = ContactCard.configure(context)

export const trust = Trust.configure(context)

export const block = Block.configure(context)

export const mute = MuteList.configure(context)

/**
 * What the user bookmarked.
 *
 * Only the public half is reachable: NIP-51 keeps private entries as ciphertext
 * and there is no signer here, so nothing could read them back. The list is
 * served to no peer instead, which is what keeps it private.
 * `docs/policy.md#visibility`.
 */
export const bookmarks = BookmarkList.configure(context)

/** Which boost carries a kind: 6 is for notes and 16 states what it holds. */
export const boostFor = (kind: number) => (kind === NOTE ? boost : genericBoost)

/** What a boost passes on, or undefined for anything that is not one. */
export const boostedBy = (event: HashedEvent) => {
  if (event.kind === REPOST) return boost.reader(event).parse().eventId()

  if (event.kind === GENERIC_REPOST) return genericBoost.reader(event).parse().eventId()

  return undefined
}

/** A calendar entry titles and locates itself the same either way; only `start` differs. */
export const calendarFor = (kind: number) => (kind === EVENT_DATE ? dateEvent : timeEvent)

/** When something happens, and whether it has a clock time at all. */
export type Occasion = {at: number; allDay: boolean}

/**
 * When a calendar entry starts, in the one shape a screen can draw.
 *
 * NIP-52 spells the two kinds the same way apart from `start`, which is a
 * `YYYY-MM-DD` day on 31922 and unix seconds on 31923 — so each reader answers
 * in its own units and this is where the two meet.
 */
export const occasionOf = (event: HashedEvent): Occasion | undefined => {
  if (event.kind === EVENT_TIME) {
    const at = timeEvent.reader(event).parse().start()

    return at === undefined ? undefined : {at, allDay: false}
  }

  const day = Date.parse(dateEvent.reader(event).parse().start() ?? "")

  return Number.isNaN(day) ? undefined : {at: day / 1000, allDay: true}
}

/** What a comment answers, or undefined for anything that is not one. */
export const commentedOn = (event: HashedEvent) =>
  event.kind === COMMENT ? comment.reader(event).parse().parent().id : undefined

/**
 * A comment on something, addressed to its parent and to the thread root.
 *
 * A comment on a comment keeps the root its parent named, so however deep a
 * conversation goes it still says what it started from.
 */
export const commentOn = (parent: HashedEvent) => comment.writer().replyTo(parent)

/**
 * One thing a person can choose to see, and the kinds it covers.
 *
 * `icon` and `noun` are what tell one card from another at a glance. The
 * palette carries none of that — `docs/ui.md#color` rations `primary` and gives
 * everything else one teal — so the shape of the mark is the whole signal.
 */
export type Category = {
  id: string
  label: string
  /** What one of them is called, for a card that names itself. */
  noun: string
  icon: Component
  kinds: number[]
}

/**
 * The content the feed shows, in the order the filter lists it.
 *
 * Reactions and deletions are absent on purpose: a reaction belongs on the
 * thing it is about, and a deletion removes one rather than being one.
 */
export const categories: Category[] = [
  {id: "notes", label: "Notes", noun: "Note", icon: MessageCircle, kinds: [NOTE]},
  {id: "comments", label: "Comments", noun: "Comment", icon: Reply, kinds: [COMMENT]},
  {
    id: "boosts",
    label: "Boosts",
    noun: "Passed on",
    icon: Repeat,
    kinds: [REPOST, GENERIC_REPOST],
  },
  {id: "polls", label: "Polls", noun: "Poll", icon: ListChecks, kinds: [POLL]},
  {
    id: "occasions",
    label: "Events",
    noun: "Event",
    icon: CalendarDays,
    kinds: [EVENT_DATE, EVENT_TIME],
  },
  {id: "articles", label: "Articles", noun: "Article", icon: Newspaper, kinds: [LONG_FORM]},
]

/** Which category a kind belongs to, for a card that has to draw its own mark. */
export const categoryOf = (kind: number) =>
  categories.find(category => category.kinds.includes(kind)) ?? categories[0]

/**
 * One line standing for an event somewhere it is not the subject.
 *
 * What a kind puts in its content varies: an article and an occasion title
 * themselves in a tag, and a boost says nothing at all. So a quote of one falls
 * back to what it is rather than drawing an empty line.
 */
export const summaryOf = (event: HashedEvent) => {
  const plain = event.content.trim() || categoryOf(event.kind).noun

  if (event.kind === LONG_FORM) return article.reader(event).parse().title() ?? plain

  if (event.kind === EVENT_DATE || event.kind === EVENT_TIME) {
    return calendarFor(event.kind).reader(event).parse().title() ?? plain
  }

  return plain
}

/**
 * The kinds that put a count or a mark on something rather than standing alone.
 *
 * A comment is not among them. It carries words of its own, so it is a card on
 * the board as well as a line under what it answers, and the screen that
 * collects it reads the parent tag instead.
 */
export const responseKinds = [REPOST, REACTION, GENERIC_REPOST, POLL_RESPONSE]
