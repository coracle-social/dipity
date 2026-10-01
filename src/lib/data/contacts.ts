// Who the device knows, and the name each one is known by.
//
// Nobody publishes a profile, so every name here is one somebody gave somebody
// else — the user's own card for a person, or a card that arrived from a
// neighbour. A name therefore always has a claimant, and the screen says whose
// it is.
//
// Trust, block and mute are the user's own lists and the core derives the trust
// graph from them, so writing one is publishing a replaceable event rather than
// setting a preference. `docs/policy.md#social-graph`.

import {derived, get, type Readable} from "svelte/store"
import {spec} from "@welshman/lib"
import type {ConfiguredKind, MuteListQuery, MuteListReader, MuteListWriter} from "@welshman/domain"
import {MUTES, type EventTemplate, type HashedEvent} from "@welshman/util"
import {answering, eventsOf, storedEvents} from "$lib/data/query"
import {publish} from "$lib/data/publish"
import {session} from "$lib/data/session"
import {block, contactCard, mute, trust} from "$lib/kinds"
import {CONTACT} from "$lib/kinds/contact"
import {BLOCK, TRUST} from "$lib/kinds/people"

/** Somebody the device knows about. */
export type Contact = {
  pubkey: string
  /** What the user calls them, which only somebody they paired with has. */
  petname?: string
  /** What other people call them, newest card first. */
  aliases: {by: string; petname: string}[]
  trusted: boolean
  muted: boolean
  blocked: boolean
}

/** What the user wrote, kept so an edit supersedes rather than replaces. */
export type Own = {
  /** The user's own card for each person they have named. */
  cards: Map<string, HashedEvent>
  trust?: HashedEvent
  block?: HashedEvent
  mute?: HashedEvent
}

/** Everybody, and what the user would be editing. */
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

/** One name a card carries, and who gave it. A card saying nothing names nobody. */
type Naming = {by: string; about: string; petname: string; event: HashedEvent}

const namings = (events: HashedEvent[]): Naming[] =>
  events
    .filter(event => event.kind === CONTACT)
    .sort((a, b) => b.created_at - a.created_at)
    .flatMap(event => {
      const card = contactCard.reader(event).parse()
      const about = card.subject()
      const petname = card.petname()

      return about && petname ? [{by: event.pubkey, about, petname, event}] : []
    })

const collate = async (events: HashedEvent[], identity?: string): Promise<Social> => {
  const named = namings(events)
  const own = {
    cards: new Map(named.filter(({by}) => by === identity).map(({about, event}) => [about, event])),
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

  for (const {by, about, petname} of named) {
    if (by === identity) {
      at(about).petname = petname
    } else {
      at(about).aliases.push({by, petname})
    }
  }

  for (const pubkey of [...trusted, ...blocked, ...muted]) at(pubkey)

  return {people, own}
}

const read = ([identity]: [string | undefined, number]) =>
  eventsOf({filter: JSON.stringify({kinds: [CONTACT, MUTES, TRUST, BLOCK]})}).then(events =>
    collate(events, identity),
  )

/** Everybody the device knows about, re-collated whenever a card or a list arrives. */
export const social: Readable<Social> = answering(
  derived([session, storedEvents], ([$session, revision]): [string | undefined, number] => [
    $session.identity,
    revision,
  ]),
  read,
  {people: new Map(), own: {cards: new Map()}},
)

/** Everybody the user muted, whose things the screens leave out. Mute never reaches the wire. */
export const muted: Readable<Set<string>> = derived(
  social,
  ({people}) =>
    new Set([...people.values()].filter(contact => contact.muted).map(({pubkey}) => pubkey)),
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

/** The last version of each of the user's own events written from here, by address. */
const written = new Map<string, HashedEvent>()

/** Edits run one at a time, so each reads what the one before it wrote. */
let edits: Promise<unknown> = Promise.resolve()

/**
 * Supersede the user's event at `address`.
 *
 * The store answers a write only after a round trip, so an edit reading it
 * straight after another would build on the version that one replaced and undo
 * it. Each builds on whichever is newer: what the store holds, or what the edit
 * before it wrote.
 */
const edit = (
  address: string,
  stored: HashedEvent | undefined,
  change: (current?: HashedEvent) => Promise<EventTemplate>,
) => {
  const run = edits
    .catch(() => undefined)
    .then(async () => {
      const mine = written.get(address)
      const current = mine && (!stored || mine.created_at >= stored.created_at) ? mine : stored

      written.set(address, await publish(await change(current), current))
    })

  edits = run

  return run
}

/** Name somebody, or rename them, which is a card addressed to them either way. */
export const name = (pubkey: string, petname: string) =>
  edit(`${CONTACT}:${pubkey}`, get(social).own.cards.get(pubkey), current =>
    contactCard
      .writer(current && contactCard.reader(current).parse())
      .name(pubkey, petname)
      .renderTemplate(),
  )

const amend = (kind: PeopleList, list: keyof Omit<Own, "cards">, pubkey: string, onList: boolean) =>
  edit(list, get(social).own[list], async current => {
    const writer = kind.writer(current && (await kind.reader(current).parse()))
    const amended = onList ? writer.addPublic(["p", pubkey]) : writer.dropTags(spec(["p", pubkey]))

    return amended.renderTemplate()
  })

export const setTrusted = (pubkey: string, trusted: boolean) =>
  amend(trust, "trust", pubkey, trusted)

/** Block somebody, which takes them off the trust list too: a block is a veto over trust. */
export const setBlocked = async (pubkey: string, blocked: boolean) => {
  const current = get(social)

  if (blocked && current.people.get(pubkey)?.trusted) {
    await amend(trust, "trust", pubkey, false)
  }

  await amend(block, "block", pubkey, blocked)
}

export const setMuted = (pubkey: string, muted: boolean) => amend(mute, "mute", pubkey, muted)
