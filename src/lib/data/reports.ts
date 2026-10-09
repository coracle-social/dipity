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
import {policy} from "$lib/data/policy"
import {session} from "$lib/data/session"
import {trashId, trashed} from "$lib/data/trash"
import {report} from "$lib/kinds"

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

/** The contacts whose flags the user ignores, a preference the core also reads. */
export const ignoredFlaggers: Readable<string[]> = derived(
  policy,
  $policy => $policy?.ignored_flaggers ?? [],
)

/** The posts the user looked at past their flags, newest first. */
const dismissed = remembered<string[]>("display.dismissed_flags", [])

/** How many dismissals are remembered, which bounds a list that would otherwise only grow. */
const DISMISSALS_KEPT = 500

/** One reason a post was flagged for, and by how many people. */
export type Flag = {reason: string; count: number}

/** Every flag that counts, by the post it names. */
export type Flags = Map<string, Flag[]>

/** One person's flag on a post: every reason they gave, what they said, and the reports carrying it. */
export type Flagging = {
  pubkey: string
  reasons: string[]
  message: string
  at: number
  reports: HashedEvent[]
}

/** How many words a flag's message may run to. */
export const FLAG_WORDS = 100

export const wordsIn = (text: string) => text.trim().split(/\s+/).filter(Boolean).length

const stored = answering(
  storedEvents,
  () => eventsOf({filter: JSON.stringify({kinds: [REPORT]})}),
  [] as HashedEvent[],
)

/** The reports not in the trash, because a trashed flag does not count. */
const live = derived([stored, trashed], ([$stored, $trashed]) =>
  $stored.filter(event => !$trashed.has(event.id)),
)

/**
 * Every flag the user or a contact made, by the post it names, newest first.
 *
 * An ignored contact's flags are here too, so the details can offer to trust
 * them again. A stranger's never are.
 */
export const flaggings: Readable<Map<string, Flagging[]>> = derived(
  [live, social, session],
  ([$live, $social, $session]) => {
    const known = (pubkey: string) =>
      pubkey === $session.identity || Boolean($social.people.get(pubkey)?.petname)
    const byPost = new Map<string, Map<string, Flagging>>()

    for (const event of $live) {
      const parsed = report.reader(event).parse()
      const id = parsed.eventId()

      if (!id || !known(event.pubkey)) continue

      const byPerson = byPost.get(id) ?? new Map<string, Flagging>()
      const held = byPerson.get(event.pubkey) ?? {
        pubkey: event.pubkey,
        reasons: [],
        message: "",
        at: event.created_at,
        reports: [],
      }
      const reason = parsed.reason() ?? "other"

      if (!held.reasons.includes(reason)) held.reasons.push(reason)
      if (!held.message && event.content.trim()) held.message = event.content.trim()

      held.at = Math.max(held.at, event.created_at)
      held.reports.push(event)
      byPerson.set(event.pubkey, held)
      byPost.set(id, byPerson)
    }

    return new Map(
      [...byPost].map(([id, byPerson]) => [id, [...byPerson.values()].sort((a, b) => b.at - a.at)]),
    )
  },
)

/** The flaggings on any of `ids`, since a boost is drawn as what it passes on. */
export const flaggingsOn = (flaggings: Map<string, Flagging[]>, ids: (string | undefined)[]) =>
  ids.flatMap(id => (id && flaggings.get(id)) || [])

/** What the user has flagged, by post, so the dialog does not publish the same flag twice. */
export const flaggedByMe: Readable<Map<string, Set<string>>> = derived(
  [flaggings, session],
  ([$flaggings, $session]) =>
    new Map(
      [...$flaggings].flatMap(([id, people]) => {
        const mine = people.find(({pubkey}) => pubkey === $session.identity)

        return mine ? [[id, new Set(mine.reasons)] as const] : []
      }),
    ),
)

const sortFlags = (tally: Map<string, number>): Flag[] =>
  [...tally]
    .map(([reason, count]) => ({reason, count}))
    .sort((a, b) => b.count - a.count || a.reason.localeCompare(b.reason))

/** The flags that count: the user's own and every contact's they trust, less any the user dismissed. */
export const flags: Readable<Flags> = derived(
  [flaggings, session, ignoredFlaggers, dismissed],
  ([$flaggings, $session, $ignored, $dismissed]) => {
    const ignored = new Set($ignored)
    const skipped = new Set($dismissed)
    const counted = new Map<string, Flag[]>()

    for (const [id, people] of $flaggings) {
      if (skipped.has(id)) continue

      const tally = new Map<string, number>()

      for (const {pubkey, reasons: given} of people) {
        if (pubkey !== $session.identity && ignored.has(pubkey)) continue

        for (const reason of given) tally.set(reason, (tally.get(reason) ?? 0) + 1)
      }

      if (tally.size > 0) counted.set(id, sortFlags(tally))
    }

    return counted
  },
)

/** The flags on any of `ids`, merged, since a boost is drawn as what it passes on. */
export const flagsOn = (flags: Flags, ids: (string | undefined)[]): Flag[] => {
  const merged = new Map<string, number>()

  for (const id of ids) {
    for (const {reason, count} of (id && flags.get(id)) || []) {
      merged.set(reason, Math.max(merged.get(reason) ?? 0, count))
    }
  }

  return sortFlags(merged)
}

/** Flag a post for one reason, saying why if the user did, unless they already flagged it for that. */
export const flag = async (event: HashedEvent, reason: string, message = "") => {
  if (get(flaggedByMe).get(event.id)?.has(reason)) return

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
  const mine = get(flaggings)
    .get(event.id)
    ?.find(({pubkey}) => pubkey === get(session).identity)
  const carrying = (mine?.reports ?? []).filter(
    held => (report.reader(held).parse().reason() ?? "other") === reason,
  )

  return Promise.all(carrying.map(held => trashId(held.id)))
}

/** Throw out one person's flag, which retracts it when it is the user's own. */
export const trashFlagging = (flagging: Flagging) =>
  Promise.all(flagging.reports.map(event => trashId(event.id)))

/** Look past the flags on some posts, which shows them as if nobody had flagged them. */
export const dismissFlags = (ids: (string | undefined)[]) => {
  const kept = ids.filter((id): id is string => Boolean(id))

  return dismissed.set(
    [...kept, ...get(dismissed).filter(id => !kept.includes(id))].slice(0, DISMISSALS_KEPT),
  )
}
