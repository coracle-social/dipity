// Who the device knows, and the name each one is known by.
//
// Nobody publishes a profile, so every name here is a pet name somebody gave
// somebody else — the user's own, or one that arrived with a neighbour's roster.
// A name therefore always has a claimant, and a card says whose it is.
//
// Trust, block and mute are the user's own lists and the core derives the trust
// graph from them, so writing one is publishing a replaceable event rather than
// setting a preference. `docs/policy.md#social-graph`.

import {derived, get, type Readable} from "svelte/store"
import {spec} from "@welshman/lib"
import type {ConfiguredKind, MuteListQuery, MuteListReader, MuteListWriter} from "@welshman/domain"
import {FOLLOWS, MUTES, type HashedEvent} from "@welshman/util"
import {answering, eventsOf, storedEvents} from "$lib/data/query"
import {publish} from "$lib/data/publish"
import {session} from "$lib/data/session"
import {block, mute, roster, trust} from "$lib/kinds"
import {BLOCK, TRUST} from "$lib/kinds/people"

/** Somebody the device knows about. */
export type Contact = {
  pubkey: string
  /** What the user calls them, which only somebody they paired with has. */
  petname?: string
  /** What other people call them, newest roster first. */
  aliases: {by: string; petname: string}[]
  trusted: boolean
  muted: boolean
  blocked: boolean
}

/** The user's own lists, kept so an edit supersedes rather than replaces. */
export type Own = {
  roster?: HashedEvent
  trust?: HashedEvent
  block?: HashedEvent
  mute?: HashedEvent
}

/** Everybody, and the lists the user would be editing. */
export type Social = {people: Map<string, Contact>; own: Own}

/** Eight characters of a key, which is what a person with no name is called. */
export const short = (pubkey: string) => pubkey.slice(0, 8)

const newest = (events: HashedEvent[], kind: number, author?: string) =>
  events
    .filter(event => event.kind === kind && (!author || event.pubkey === author))
    .sort((a, b) => b.created_at - a.created_at)[0]

/** One of the three pubkey lists, all of which read as NIP-51's mute list does. */
type PeopleList = ConfiguredKind<MuteListReader, MuteListWriter, MuteListQuery>

const listed = async (event: HashedEvent | undefined, kind: PeopleList) =>
  new Set(event ? (await kind.reader(event).parse()).pubkeys() : [])

const collate = async (events: HashedEvent[], identity?: string): Promise<Social> => {
  const own = {
    roster: newest(events, FOLLOWS, identity),
    trust: newest(events, TRUST, identity),
    block: newest(events, BLOCK, identity),
    mute: newest(events, MUTES, identity),
  }

  const trusted = await listed(own.trust, trust)
  const blocked = await listed(own.block, block)
  const muted = await listed(own.mute, mute)
  const people = new Map<string, Contact>()
  const at = (pubkey: string) => {
    const contact = people.get(pubkey) ?? {
      pubkey,
      aliases: [],
      trusted: trusted.has(pubkey),
      muted: muted.has(pubkey),
      blocked: blocked.has(pubkey),
    }

    people.set(pubkey, contact)

    return contact
  }

  const rosters = events
    .filter(event => event.kind === FOLLOWS)
    .sort((a, b) => b.created_at - a.created_at)

  for (const event of rosters) {
    for (const {pubkey, petname} of roster.reader(event).parse().namings()) {
      if (event.pubkey === identity) {
        at(pubkey).petname = petname
      } else {
        at(pubkey).aliases.push({by: event.pubkey, petname})
      }
    }
  }

  for (const pubkey of [...trusted, ...blocked, ...muted]) at(pubkey)

  return {people, own}
}

const read = ([identity]: [string | undefined, number]) =>
  eventsOf({filter: JSON.stringify({kinds: [FOLLOWS, MUTES, TRUST, BLOCK]})}).then(events =>
    collate(events, identity),
  )

/** Everybody the device knows about, re-collated whenever a list arrives. */
export const social: Readable<Social> = answering(
  derived([session, storedEvents], ([$session, revision]): [string | undefined, number] => [
    $session.identity,
    revision,
  ]),
  read,
  {people: new Map(), own: {}},
)

/** The people list, the ones the user named first. */
export const contacts: Readable<Contact[]> = derived(social, ({people}) =>
  [...people.values()].sort((a, b) => {
    const named = Number(Boolean(b.petname)) - Number(Boolean(a.petname))

    return (
      named ||
      (a.petname ?? a.aliases[0]?.petname ?? a.pubkey).localeCompare(
        b.petname ?? b.aliases[0]?.petname ?? b.pubkey,
      )
    )
  }),
)

/** What to call somebody, and whose name it is when it is not the user's. */
export type Named = {name: string; according?: string}

export const nameOf = ({people}: Social, pubkey: string): Named => {
  const contact = people.get(pubkey)

  if (contact?.petname) return {name: contact.petname}

  const alias = contact?.aliases[0]

  if (!alias) return {name: short(pubkey)}

  return {name: alias.petname, according: people.get(alias.by)?.petname ?? short(alias.by)}
}

/** Whether the user paired with somebody themselves, which is what their own name means. */
export const isKnown = (social: Social, pubkey: string) =>
  Boolean(social.people.get(pubkey)?.petname)

/** Name somebody, or rename them. */
export const name = async (pubkey: string, petname: string) => {
  const {own} = get(social)
  const seed = own.roster && roster.reader(own.roster).parse()

  await publish(await roster.writer(seed).name(pubkey, petname).renderTemplate(), own.roster)
}

const amend = async (
  kind: PeopleList,
  current: HashedEvent | undefined,
  pubkey: string,
  onList: boolean,
) => {
  const writer = kind.writer(current && (await kind.reader(current).parse()))
  const amended = onList ? writer.addPublic(["p", pubkey]) : writer.dropTags(spec(["p", pubkey]))

  await publish(await amended.renderTemplate(), current)
}

export const setTrusted = (pubkey: string, trusted: boolean) =>
  amend(trust, get(social).own.trust, pubkey, trusted)

export const setBlocked = (pubkey: string, blocked: boolean) =>
  amend(block, get(social).own.block, pubkey, blocked)

export const setMuted = (pubkey: string, muted: boolean) =>
  amend(mute, get(social).own.mute, pubkey, muted)
