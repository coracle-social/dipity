// Flags: what the user and the contacts they trust said is wrong with a post.
//
// A flag is a NIP-56 report, kind 1984, naming the post and its author with a
// reason and no message. It travels like a reaction does. Like a mute, it is a
// display filter and never gates propagation: a flagged post is still kept and
// still passed on. A stranger's flag counts for nothing, because anybody in
// range could otherwise hide anything. `docs/policy.md#social-graph`.

import {derived, get, type Readable} from "svelte/store"
import {REPORT, type HashedEvent} from "@welshman/util"
import {social} from "$lib/data/contacts"
import {publish} from "$lib/data/publish"
import {answering, eventsOf, remembered, storedEvents} from "$lib/data/query"
import {session} from "$lib/data/session"
import {report} from "$lib/kinds"

/** The reasons NIP-56 names, in the order the menu offers them. */
export const reasons = [
  {id: "spam", label: "Spam"},
  {id: "nudity", label: "Nudity"},
  {id: "profanity", label: "Profanity"},
  {id: "illegal", label: "Illegal"},
  {id: "malware", label: "Malware"},
  {id: "impersonation", label: "Impersonation"},
  {id: "other", label: "Other"},
]

export const reasonLabel = (id: string) => reasons.find(reason => reason.id === id)?.label ?? id

/** Whether a flagged post leaves the board altogether rather than drawing as a warning. */
export const hideFlagged = remembered("display.hide_flagged", false)

/** The contacts whose flags the user ignores. Every other contact's count. */
export const ignoredFlaggers = remembered<string[]>("display.ignored_flaggers", [])

/** The posts the user looked at past their flags, newest first. */
const dismissed = remembered<string[]>("display.dismissed_flags", [])

/** How many dismissals are remembered, which bounds a list that would otherwise only grow. */
const DISMISSALS_KEPT = 500

/** One reason a post was flagged for, and by how many people. */
export type Flag = {reason: string; count: number}

/** Every flag that counts, by the post it names. */
export type Flags = Map<string, Flag[]>

const stored = answering(
  storedEvents,
  () => eventsOf({filter: JSON.stringify({kinds: [REPORT]})}),
  [] as HashedEvent[],
)

/** What the user has flagged, by post, so the menu does not publish the same flag twice. */
export const flaggedByMe: Readable<Map<string, Set<string>>> = derived(
  [stored, session],
  ([$stored, $session]) => {
    const mine = new Map<string, Set<string>>()

    for (const event of $stored) {
      const parsed = report.reader(event).parse()
      const id = parsed.eventId()

      if (event.pubkey !== $session.identity || !id) continue

      mine.set(id, (mine.get(id) ?? new Set()).add(parsed.reason() ?? "other"))
    }

    return mine
  },
)

/** The flags that count: the user's own and every contact's they trust, less any the user dismissed. */
export const flags: Readable<Flags> = derived(
  [stored, social, session, ignoredFlaggers, dismissed],
  ([$stored, $social, $session, $ignored, $dismissed]) => {
    const ignored = new Set($ignored)
    const skipped = new Set($dismissed)
    const counts = (pubkey: string) =>
      pubkey === $session.identity ||
      (Boolean($social.people.get(pubkey)?.petname) && !ignored.has(pubkey))
    const by = new Map<string, Map<string, Set<string>>>()

    for (const event of $stored) {
      const parsed = report.reader(event).parse()
      const id = parsed.eventId()

      if (!id || skipped.has(id) || !counts(event.pubkey)) continue

      const reasonsFor = by.get(id) ?? new Map<string, Set<string>>()
      const reason = parsed.reason() ?? "other"

      reasonsFor.set(reason, (reasonsFor.get(reason) ?? new Set()).add(event.pubkey))
      by.set(id, reasonsFor)
    }

    return new Map(
      [...by].map(([id, reasonsFor]) => [
        id,
        [...reasonsFor]
          .map(([reason, people]) => ({reason, count: people.size}))
          .sort((a, b) => b.count - a.count || a.reason.localeCompare(b.reason)),
      ]),
    )
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

  return [...merged]
    .map(([reason, count]) => ({reason, count}))
    .sort((a, b) => b.count - a.count || a.reason.localeCompare(b.reason))
}

/** Flag a post for one reason, unless the user already flagged it for that. */
export const flag = async (event: HashedEvent, reason: string) => {
  if (get(flaggedByMe).get(event.id)?.has(reason)) return

  const writer = report.writer().setPubkey(event.pubkey).setEventId(event.id).setReason(reason)

  await publish(await writer.renderTemplate())
}

/** Look past the flags on some posts, which shows them as if nobody had flagged them. */
export const dismissFlags = (ids: (string | undefined)[]) => {
  const kept = ids.filter((id): id is string => Boolean(id))

  return dismissed.set(
    [...kept, ...get(dismissed).filter(id => !kept.includes(id))].slice(0, DISMISSALS_KEPT),
  )
}

/** Count a contact's flags, or ignore them. */
export const setFlagsTrusted = (pubkey: string, trusted: boolean) => {
  const rest = get(ignoredFlaggers).filter(ignored => ignored !== pubkey)

  return ignoredFlaggers.set(trusted ? rest : [...rest, pubkey])
}
