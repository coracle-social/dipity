// Flags: what the user and the contacts they trust say is wrong with a post.
//
// A flag is a NIP-56 report, kind 1984, naming the post and its author with a
// reason and an optional message. A counted flag draws its post as a warning.
// The core decides what flags hold back from peers, because it serves while the
// view is suspended. A stranger's flag never counts. `docs/policy.md#flags`.

import {derived, get, type Readable} from "svelte/store"
import {REPORT, type HashedEvent} from "@welshman/util"
import {social} from "$lib/data/contacts"
import {publish} from "$lib/data/publish"
import {answering, eventsOf, remembered, storedEvents} from "$lib/data/query"
import {untrusted} from "$lib/data/policy"
import {session} from "$lib/data/session"
import {trashId, trashed} from "$lib/data/trash"
import {boostedBy, report} from "$lib/kinds"

/** The reasons the dialog offers: NIP-56's, and violence. */
export const reasons = [
  {id: "spam", label: "Spam"},
  {id: "nudity", label: "Nudity"},
  {id: "profanity", label: "Profanity"},
  {id: "violence", label: "Violence"},
  {id: "illegal", label: "Illegal"},
  {id: "malware", label: "Malware"},
  {id: "impersonation", label: "Impersonation"},
  {id: "other", label: "Other"},
]

export const reasonLabel = (id: string) => reasons.find(reason => reason.id === id)?.label ?? id

/** The posts the user looked at past their flags, newest first. */
const dismissed = remembered<string[]>("display.dismissed_flags", [])

/** How many dismissals are remembered, which bounds a list that would otherwise only grow. */
const DISMISSALS_KEPT = 500

/** How many words a flag's message may run to. */
export const FLAG_WORDS = 100

export const wordsIn = (text: string) => text.trim().split(/\s+/).filter(Boolean).length

/** One reason a person gave, and the report carrying it. */
export type Reason = {reason: string; report: HashedEvent}

/** One person's flag on a post: every reason they gave and what they said. */
export type Flagging = {pubkey: string; reasons: Reason[]; message: string; at: number}

/** One reason a post was flagged for, and by how many people. */
export type Tally = {reason: string; count: number}

/** The posts whose flags mark an item: its own, and what it passes on if it is a boost. */
export const flaggedIds = (event: HashedEvent) =>
  [event.id, boostedBy(event)].filter((id): id is string => Boolean(id))

const stored = answering(
  storedEvents,
  () => eventsOf({filter: JSON.stringify({kinds: [REPORT]})}),
  [] as HashedEvent[],
)

/**
 * Every flag the user or a contact made, by the post it names, newest first.
 *
 * A flag in the trash counts for nothing and is left out. An ignored contact's
 * flags are here, so the details can offer to trust them again, and a
 * stranger's never are.
 */
export const flaggings: Readable<Map<string, Flagging[]>> = derived(
  [stored, trashed, social, session],
  ([$stored, $trashed, $social, $session]) => {
    const known = (pubkey: string) =>
      pubkey === $session.identity || Boolean($social.people.get(pubkey)?.petname)
    const byPost = new Map<string, Map<string, Flagging>>()

    for (const event of $stored) {
      const parsed = report.reader(event).parse()
      const id = parsed.eventId()

      if (!id || $trashed.has(event.id) || !known(event.pubkey)) continue

      const byPerson = byPost.get(id) ?? new Map<string, Flagging>()
      const held = byPerson.get(event.pubkey) ?? {
        pubkey: event.pubkey,
        reasons: [],
        message: "",
        at: event.created_at,
      }

      held.reasons.push({reason: parsed.reason() ?? "other", report: event})
      held.message ||= event.content.trim()
      held.at = Math.max(held.at, event.created_at)
      byPerson.set(event.pubkey, held)
      byPost.set(id, byPerson)
    }

    return new Map(
      [...byPost].map(([id, byPerson]) => [id, [...byPerson.values()].sort((a, b) => b.at - a.at)]),
    )
  },
)

/** The flaggings on any of `ids`. */
export const flaggingsOn = (flaggings: Map<string, Flagging[]>, ids: string[]) =>
  ids.flatMap(id => flaggings.get(id) ?? [])

/** The flaggings that put a warning on a post: the user's own and trusted contacts', on posts the user has not looked past. */
export const counted: Readable<Map<string, Flagging[]>> = derived(
  [flaggings, session, untrusted, dismissed],
  ([$flaggings, $session, $untrusted, $dismissed]) =>
    new Map(
      [...$flaggings]
        .filter(([id]) => !$dismissed.includes(id))
        .map(([id, people]) => [
          id,
          people.filter(({pubkey}) => pubkey === $session.identity || !$untrusted.includes(pubkey)),
        ]),
    ),
)

/** Each reason given across some flaggings, with how many different people gave it, most given first. */
export const tallyReasons = (flaggings: Flagging[]): Tally[] => {
  const people = new Map<string, Set<string>>()

  for (const {pubkey, reasons: given} of flaggings) {
    for (const {reason} of given) people.set(reason, (people.get(reason) ?? new Set()).add(pubkey))
  }

  return [...people]
    .map(([reason, by]) => ({reason, count: by.size}))
    .sort((a, b) => b.count - a.count || a.reason.localeCompare(b.reason))
}

/** The user's own flagging of a post, if they flagged it. */
export const mineOn = (flaggings: Map<string, Flagging[]>, id: string, identity?: string) =>
  flaggings.get(id)?.find(({pubkey}) => pubkey === identity)

/** Flag a post for one reason, saying why if the user did, unless they already flagged it for that. */
export const flag = async (event: HashedEvent, reason: string, message = "") => {
  const mine = mineOn(get(flaggings), event.id, get(session).identity)

  if (mine?.reasons.some(given => given.reason === reason)) return

  const writer = report
    .writer()
    .setPubkey(event.pubkey)
    .setEventId(event.id)
    .setReason(reason)
    .setContent(message.trim())

  await publish(await writer.renderTemplate())
}

/** Take back the user's flag on a post for one reason, which retracts the report carrying it. */
export const unflag = (event: HashedEvent, reason: string) => {
  const mine = mineOn(get(flaggings), event.id, get(session).identity)
  const carrying = mine?.reasons.filter(given => given.reason === reason) ?? []

  return Promise.all(carrying.map(({report: held}) => trashId(held.id)))
}

/** Throw out one person's flag, which retracts it when it is the user's own. */
export const trashFlagging = (flagging: Flagging) =>
  Promise.all(flagging.reasons.map(({report: held}) => trashId(held.id)))

/** Look past the flags on some posts, which shows them as if nobody had flagged them. */
export const dismissFlags = (ids: string[]) =>
  dismissed.set(
    [...ids, ...get(dismissed).filter(id => !ids.includes(id))].slice(0, DISMISSALS_KEPT),
  )
