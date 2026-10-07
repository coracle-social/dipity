// Who the device knows, and the name each one is known by.
//
// Nobody publishes a profile, so every name here is one somebody gave somebody
// else — the user's own card for a person, or a card that arrived from a
// neighbour. A name therefore always has a claimant, and the screen says whose
// it is.
//
// The user's cards, block and mute are things they publish, and the core
// derives the contact graph from them, so writing one is publishing an event
// rather than setting a preference. `docs/policy.md#social-graph`.
//
// The mute list names topics as well as people, so a thing that is not a person
// is edited here too. One event has one editor. Two of them amending it would
// each build on the version the other replaced.

import {derived, get, type Readable} from "svelte/store"
import {spec} from "@welshman/lib"
import type {ConfiguredKind, MuteListQuery, MuteListReader, MuteListWriter} from "@welshman/domain"
import {MUTES, type EventTemplate, type HashedEvent} from "@welshman/util"
import {Dip} from "$lib/core"
import {links} from "$lib/data/links"
import {answering, eventsOf, storedEvents} from "$lib/data/query"
import {publish} from "$lib/data/publish"
import {session} from "$lib/data/session"
import {block, contactCard, mute, topicsIn} from "$lib/kinds"
import {CONTACT} from "$lib/kinds/contact"
import {BLOCK} from "$lib/kinds/people"

/** Somebody the device knows about. */
export type Contact = {
  pubkey: string
  /** What the user calls them, which only somebody they paired with has. */
  petname?: string
  /** What other people call them, newest card first. */
  aliases: {by: string; petname: string}[]
  muted: boolean
  blocked: boolean
}

/** What the user wrote, kept so an edit supersedes rather than replaces. */
export type Own = {
  /** The user's own card for each person they have named. */
  cards: Map<string, HashedEvent>
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

/** The user's newest card for each person, emptied ones included, since emptying one is forgetting them. */
const ownCards = (events: HashedEvent[], identity?: string) => {
  const cards = new Map<string, HashedEvent>()

  for (const event of events) {
    if (event.kind !== CONTACT || event.pubkey !== identity) continue

    const about = contactCard.reader(event).parse().subject()
    const held = about && cards.get(about)

    if (about && (!held || event.created_at > held.created_at)) cards.set(about, event)
  }

  return cards
}

const collate = async (events: HashedEvent[], identity?: string): Promise<Social> => {
  const named = namings(events)
  const own = {
    cards: ownCards(events, identity),
    block: newest(events, BLOCK, identity),
    mute: newest(events, MUTES, identity),
  }

  const blocked = await listed(own.block, block)
  const muted = await listed(own.mute, mute)
  const people = new Map<string, Contact>()
  const at = (pubkey: string) => {
    const contact = people.get(pubkey) ?? {
      pubkey,
      aliases: [],
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

  for (const pubkey of [...blocked, ...muted]) at(pubkey)

  return {people, own}
}

const read = ([identity]: [string | undefined, number]) =>
  eventsOf({filter: JSON.stringify({kinds: [CONTACT, MUTES, BLOCK]})}).then(events =>
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

/** Every topic the user muted, whose posts the board leaves out. */
export const mutedTopics: Readable<Set<string>> = derived(
  social,
  ({own}) => new Set(own.mute ? topicsIn(own.mute) : []),
)

/** Somebody on the people list, and whether a link to them is up right now. */
export type Listed = Contact & {connected: boolean}

/**
 * The people list: whoever is connected right now first, then the ones the user
 * named. A connected peer the device knows nothing else about is listed too,
 * since being in the room is reason enough. The user is not among them.
 */
export const contacts: Readable<Listed[]> = derived(
  [social, session, links],
  ([{people}, $session, $links]) => {
    const here = new Set($links.map(({pubkey}) => pubkey))
    const listed = new Map<string, Listed>()

    for (const contact of people.values()) {
      listed.set(contact.pubkey, {...contact, connected: here.has(contact.pubkey)})
    }

    for (const pubkey of here) {
      if (!listed.has(pubkey)) {
        listed.set(pubkey, {
          pubkey,
          aliases: [],
          muted: false,
          blocked: false,
          connected: true,
        })
      }
    }

    const label = (contact: Listed) =>
      contact.petname ?? contact.aliases[0]?.petname ?? contact.pubkey

    return [...listed.values()]
      .filter(contact => contact.pubkey !== $session.identity)
      .sort(
        (a, b) =>
          Number(b.connected) - Number(a.connected) ||
          Number(Boolean(b.petname)) - Number(Boolean(a.petname)) ||
          label(a).localeCompare(label(b)),
      )
  },
)

/** One name other people know the user by, however each of them spelled it, and who gave it. */
export type Alias = {slug: string; spellings: string[]; by: string[]}

/** A name reduced to what two people typing it the same way would agree on. */
export const slugify = (name: string) =>
  name
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, "-")
    .replace(/^-+|-+$/g, "") || name.trim().toLowerCase()

/**
 * The names other people have given the user, one entry per name however it
 * was spelled, most given first. Each comes from a card somebody published
 * about the user, so each has a claimant.
 */
export const aliases: Readable<Alias[]> = derived([social, session], ([{people}, $session]) => {
  const given = $session.identity ? (people.get($session.identity)?.aliases ?? []) : []
  const bySlug = new Map<string, Alias>()

  for (const {by, petname} of given) {
    const slug = slugify(petname)
    const alias = bySlug.get(slug) ?? {slug, spellings: [], by: []}

    if (!alias.spellings.includes(petname)) alias.spellings.push(petname)
    if (!alias.by.includes(by)) alias.by.push(by)

    bySlug.set(slug, alias)
  }

  return [...bySlug.values()].sort(
    (a, b) => b.by.length - a.by.length || a.slug.localeCompare(b.slug),
  )
})

/** What to call somebody, and whose name it is when it is not the user's. */
export type Named = {name: string; according?: string}

export const nameOf = ({people}: Social, pubkey: string): Named => {
  const contact = people.get(pubkey)

  if (contact?.petname) return {name: contact.petname}

  const alias = contact?.aliases[0]

  if (!alias) return {name: short(pubkey)}

  return {name: alias.petname, according: people.get(alias.by)?.petname ?? short(alias.by)}
}

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

const amend = (kind: PeopleList, list: keyof Omit<Own, "cards">, tag: string[], onList: boolean) =>
  edit(list, get(social).own[list], async current => {
    const writer = kind.writer(current && (await kind.reader(current).parse()))
    const amended = onList ? writer.addPublic(tag) : writer.dropTags(spec(tag))

    return amended.renderTemplate()
  })

export const setBlocked = (pubkey: string, blocked: boolean) =>
  amend(block, "block", ["p", pubkey], blocked)

export const setMuted = (pubkey: string, muted: boolean) =>
  amend(mute, "mute", ["p", pubkey], muted)

/** Mute a topic, which NIP-51 puts on the same list as a muted person. */
export const setTopicMuted = (topic: string, muted: boolean) =>
  amend(mute, "mute", ["t", topic], muted)

/**
 * Forget somebody: an empty card supersedes the user's name for them, they come
 * off the mute list, the names other people gave them are dropped
 * from this phone, and their device is met as a stranger until the two next
 * sync. A block stays, since it is the one thing guarding the wire.
 */
export const forget = async (pubkey: string) => {
  const contact = get(social).people.get(pubkey)

  if (contact?.petname) await name(pubkey, "")
  if (contact?.muted) await setMuted(pubkey, false)

  // Dropped rather than refused, so a card that arrives again names them again.
  const theirs = await eventsOf({filter: JSON.stringify({kinds: [CONTACT], "#d": [pubkey]})})

  for (const card of theirs) {
    if (card.pubkey !== get(session).identity) await Dip.forgetEvent({id: card.id})
  }

  await Dip.forgetPairing({pubkey})
}
