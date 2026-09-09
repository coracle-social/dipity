// A core, in the browser, for `just dev`.
//
// `registerPlugin` answers a proxy with nothing behind it on the web, so
// without this the view can only ever render "the core is not running". This is
// the other half of that boundary: the store, and a clock walking a
// neighborhood past the device — gating, gossiping, and going quiet — so a
// screen is reachable in a browser and every call the view makes is answered.
//
// It is not a second implementation of anything the core decides. Sync, policy
// and proofs happen below the bridge and are not modelled here; what is
// modelled is their observable shape, which is what a screen is built against.
//
// It is also `window.dip`, because most of the surface has no screen yet: a
// consent gate, a key backup and an identity transfer are all answered from the
// console until something is built to answer them.
//
// Loaded by a dynamic import under `import.meta.env.DEV`, so neither this
// module nor faker reaches the bundle the shells load.

import {WebPlugin} from "@capacitor/core"
import {faker} from "@faker-js/faker"
import {blake3} from "@noble/hashes/blake3.js"
import {sha256} from "@noble/hashes/sha2.js"
import {bytesToHex} from "@noble/hashes/utils.js"
import {getPubkey, hash, makeSecret} from "@welshman/util"
import {nip19} from "nostr-tools"
import type {DipCore, Pref, Query} from "$lib/core"
import {identity, people, post, sas, type Person} from "$lib/dev/neighborhood"
import {Store, type Blob, type StoredEvent} from "$lib/dev/store"

const HOUR = 3600

const DAY = 86_400

/** How far back the seeded arrivals reach, matching the band the home screen draws. */
const BAND_HOURS = 12

/** How often somebody walks past, and how much that varies. */
const ENCOUNTER_EVERY = 9_000

const ENCOUNTER_JITTER = 12_000

/** How long a consent gate stays open before the peer has walked away. */
const GATE_OPEN = 45_000

/** How long a refusal is respected before that person is asked about again. */
const GATE_BACKOFF = 10 * 60_000

/** The core's own default, which the simulated device has never overridden. */
const RETENTION_DAYS = 30

const now = () => Math.floor(Date.now() / 1000)

const log = (message: string) => console.info(`[sim] ${message}`)

const short = (pubkey: string) => pubkey.slice(0, 8)

const bytes = (base64: string) => Uint8Array.from(atob(base64), letter => letter.charCodeAt(0))

/** The three `imeta` entries the core answers, computed the same way it does. */
const describe = (media: Uint8Array) => [
  `x ${bytesToHex(sha256(media))}`,
  `size ${media.length}`,
  `blake3 ${bytesToHex(blake3(media))}`,
]

/** A blob known only by what an `imeta` tag claims about it. */
const fromImeta = (tag: string[]): Blob | undefined => {
  const entries = tag.slice(1)
  const value = (name: string) =>
    entries.find(entry => entry.startsWith(`${name} `))?.slice(name.length + 1)
  const claimed = value("x")

  return claimed
    ? {
        sha256: claimed,
        role: "Original",
        url: value("url") ?? null,
        mime_type: value("m") ?? null,
        size: value("size") ? Number(value("size")) : null,
        dim: value("dim") ?? null,
        blurhash: value("blurhash") ?? null,
        alt: value("alt") ?? null,
        blake3: value("blake3") ?? null,
        imeta: entries,
        stored_bytes: 0,
        complete: false,
        accessed_at: null,
      }
    : undefined
}

/** The core as the browser gets to have one. */
export class Simulator extends WebPlugin implements DipCore {
  private store = new Store()
  private identity = identity
  private paired = new Set<string>()
  private refused = new Map<string, number>()
  private gates = new Map<number, Person>()
  private links = 1
  private clock: ReturnType<typeof setTimeout> | undefined

  constructor() {
    super()
    this.store.setPref("policy.retention_days", JSON.stringify(RETENTION_DAYS), now())

    for (const person of people.filter(person => person.known)) {
      this.paired.add(person.pubkey)
    }

    this.backlog()

    // Reachable as `dip` in the console, so a gate with no screen yet can still be answered.
    Object.assign(window, {dip: this})
  }

  // ------------------------------------------------------ the neighborhood

  /** What the device had already collected before this session opened. */
  private backlog() {
    const at = now()
    const busiest = faker.number.int({min: 2, max: BAND_HOURS - 2})
    const known = people.filter(person => this.paired.has(person.pubkey))

    for (let hour = 0; hour < BAND_HOURS; hour++) {
      const many = hour === busiest
      const count = faker.number.int(many ? {min: 4, max: 6} : {min: 0, max: 2})

      for (let index = 0; index < count; index++) {
        this.arrive(faker.helpers.arrayElement(known), at - hour * HOUR - faker.number.int(HOUR))
      }
    }

    for (let index = 0; index < 4; index++) {
      this.arrive(
        faker.helpers.arrayElement(known),
        at - faker.number.int({min: 2 * DAY, max: 26 * DAY}),
      )
    }
  }

  /** One event handed over by a person, authored some time before they carried it here. */
  private arrive(person: Person, at: number) {
    const {kind, content, tags, blobs} = post()
    const written = at - faker.number.int({min: 0, max: 6 * HOUR})
    const event = hash({kind, content, tags, created_at: written, pubkey: person.pubkey})

    this.store.record(event as StoredEvent, person.pubkey, at, blobs)

    return event
  }

  /** The same event reaching the device again from somebody else, which is what cools slowest. */
  private resight(person: Person, at: number) {
    const seen = this.store
      .list({order: "seenAt", filter: JSON.stringify({limit: 30})})
      .filter(detail => !detail.sightings.some(sighting => sighting.pubkey === person.pubkey))
    const detail = seen.length ? faker.helpers.arrayElement(seen) : undefined

    if (detail) {
      this.store.record(detail.event, person.pubkey, at)
    }

    return detail
  }

  /** Everything one encounter produces: a gate if they are a stranger, then a burst. */
  private encounter() {
    const person = faker.helpers.arrayElement(people)
    const link = this.links++

    if (this.paired.has(person.pubkey)) {
      this.gossip(person, faker.number.int({min: 1, max: person.talkative}))
    } else {
      this.gate(person, link)
      this.gossip(person, 1)
    }

    this.tick()
  }

  /** A stranger asking, which is a question for the user and not an event. */
  private gate(person: Person, link: number) {
    const asked = this.refused.get(person.pubkey) ?? 0

    if (Date.now() - asked > GATE_BACKOFF) {
      this.gates.set(link, person)
      this.notifyListeners("requestApproval", {link})
      log(
        `${short(person.pubkey)} is asking to pair — dip.approve({link: ${link}, approved: true})`,
      )

      setTimeout(() => this.walkAway(link), GATE_OPEN)
    }
  }

  /** An unanswered gate is a peer who has gone, not a peer who was refused. */
  private walkAway(link: number) {
    const person = this.gates.get(link)

    if (person) {
      this.gates.delete(link)
      log(`${short(person.pubkey)} walked away with link ${link} unanswered`)
    }
  }

  /** A burst of events off one peer, some of them things the device already had. */
  private gossip(person: Person, count: number) {
    const at = now()
    let fresh = 0

    for (let index = 0; index < count; index++) {
      const again = faker.datatype.boolean({probability: 0.3}) && this.resight(person, at)

      if (!again) {
        this.arrive(person, at)
        fresh++
      }
    }

    this.notifyListeners("storeChanged", {group: "events"})
    log(`${short(person.pubkey)} handed over ${count} (${fresh} new)`)
  }

  /** One clock, however many times `start` is called, or the street doubles in size. */
  private tick() {
    clearTimeout(this.clock)

    this.clock = setTimeout(
      () => this.encounter(),
      ENCOUNTER_EVERY + faker.number.int(ENCOUNTER_JITTER),
    )
  }

  // --------------------------------------------------------------- identity

  async coreVersion() {
    return {version: "simulated"}
  }

  async hasIdentity() {
    return {exists: Boolean(this.identity)}
  }

  async createIdentity() {
    this.identity = getPubkey(makeSecret())

    return {npub: nip19.npubEncode(this.identity)}
  }

  async importIdentity({nsec}: {nsec: string}) {
    this.identity = getPubkey(bytesToHex(nip19.decode(nsec).data as Uint8Array))

    return {npub: nip19.npubEncode(this.identity)}
  }

  async deleteIdentity() {
    const existed = Boolean(this.identity)

    this.identity = ""
    clearTimeout(this.clock)

    return {existed}
  }

  async start() {
    log(`started as ${short(this.identity)}, ${people.length} people in range`)
    this.tick()

    return {identity: this.identity}
  }

  // ------------------------------------------------------------- the store

  async mediaTags({media}: {media: string}) {
    return {entries: describe(bytes(media))}
  }

  async publish({event, media = []}: {event: string; media?: string[]}) {
    const published = JSON.parse(event) as StoredEvent
    const blobs = published.tags
      .filter(tag => tag[0] === "imeta")
      .map(fromImeta)
      .filter(blob => blob !== undefined)

    this.store.record(published, this.identity, now(), blobs)

    for (const attached of media.map(bytes)) {
      this.store.hold(bytesToHex(sha256(attached)), attached.length)
    }

    this.notifyListeners("storeChanged", {group: "events"})
    log(`published kind ${published.kind}, ${blobs.length} attached`)
  }

  async approve({link, approved}: {link: number; approved: boolean}) {
    const person = this.gates.get(link)

    this.gates.delete(link)

    if (person && approved) {
      this.paired.add(person.pubkey)
      log(`paired with ${short(person.pubkey)}`)
      this.gossip(person, person.talkative)
    } else if (person) {
      this.refused.set(person.pubkey, Date.now())
      log(`refused ${short(person.pubkey)}`)
    }
  }

  async listEvents(query?: Query) {
    return {events: this.store.list(query).map(detail => JSON.stringify(detail.event))}
  }

  async listDetails(query?: Query) {
    return {details: this.store.list(query).map(detail => JSON.stringify(detail))}
  }

  async getEvent({id}: {id: string}) {
    const detail = this.store.get(id)

    return {event: detail ? JSON.stringify(detail.event) : null}
  }

  async wantedBlobs(options?: {limit?: number}) {
    return {blobs: this.store.wanted(options?.limit).map(blob => JSON.stringify(blob))}
  }

  async getBlob({sha256: wanted}: {sha256: string}) {
    const blob = this.store.blob(wanted)

    return {blob: blob && JSON.stringify(blob)}
  }

  async eventsReferencingBlob({sha256: wanted}: {sha256: string}) {
    return {ids: this.store.referencing(wanted)}
  }

  // -------------------------------------------------------------- settings

  async preferences(): Promise<{preferences: Pref[]}> {
    return {preferences: this.store.prefs()}
  }

  async preference({key}: {key: string}) {
    return {value: this.store.pref(key)}
  }

  async setPreference({key, value}: {key: string; value: string}) {
    this.store.setPref(key, value, now())
    this.notifyListeners("storeChanged", {group: "preferences"})
  }

  async clearPreference({key}: {key: string}) {
    const existed = this.store.clearPref(key)

    this.notifyListeners("storeChanged", {group: "preferences"})

    return {existed}
  }

  // ---------------------------------------------------- key and its transfer

  async exportKey() {
    log("wrote a key backup and put it in front of the user")
    this.notifyListeners("keyBackupShared", {shared: true})
  }

  async offerIdentity({link}: {link: number}) {
    const code = sas()

    log(`offering this identity on link ${link}, code ${code}`)
    this.notifyListeners("confirmIdentityTransfer", {link, code})
  }

  async answerIdentityTransfer({link, confirmed}: {link: number; confirmed: boolean}) {
    this.notifyListeners("identityTransfer", {link, received: false})
    log(`identity transfer on link ${link} was ${confirmed ? "confirmed" : "refused"}`)
  }

  async takeTransferredIdentity({link}: {link: number}) {
    log(`adopted the identity offered on link ${link}`)

    return {identity: this.identity}
  }
}

/** One simulated core per page, which is what `registerPlugin` asks the web for. */
export const simulated = () => new Simulator()
