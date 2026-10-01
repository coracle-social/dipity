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
// It is also `window.dip`, because part of the surface has no screen to start
// it from: a peer offering this device its identity is `dip.receiveOffer(link)`,
// since nothing in a neighborhood should volunteer a key on a timer.
//
// Loaded by a dynamic import under `import.meta.env.DEV`, so neither this
// module nor faker reaches the bundle the shells load.

import {WebPlugin} from "@capacitor/core"
import {faker} from "@faker-js/faker"
import {blake3} from "@noble/hashes/blake3.js"
import {sha256} from "@noble/hashes/sha2.js"
import {bytesToHex} from "@noble/hashes/utils.js"
import {COMMENT, NOTE, REACTION, REPOST, getPubkey, hash, makeSecret} from "@welshman/util"
import type {HashedEvent} from "@welshman/util"
import {nip19} from "nostr-tools"
import type {Blob, DipCore, Pref, Query} from "$lib/core"
import {boost, card, comment, genericBoost, reaction} from "$lib/dev/kinds"
import {compiled} from "$lib/dev/policy"
import {
  emoji,
  identity,
  pairingCode,
  people,
  post,
  remark,
  shortCode,
  type Person,
} from "$lib/dev/neighborhood"
import {Store} from "$lib/dev/store"

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

/**
 * A boost, which names what it passes on and does not embed it.
 *
 * NIP-18 puts a copy of the boosted event in the content and dip carries the
 * reference alone, the same way the view writes one. `docs/sync.md#event-sync`.
 */
const boostOf = (about: HashedEvent) =>
  (about.kind === NOTE ? boost : genericBoost).writer().setEvent(about).setContent("")

/** An identity transfer running on one link, from whichever side started it. */
type Transfer = {
  link: number
  /** Whether this device is the one giving its key away. */
  source: boolean
  /** Whether this device's user has compared the digits yet. */
  confirmed: boolean
  /** Whether the other device's user has. */
  accepted: boolean
}

/** The core as the browser gets to have one. */
export class Simulator extends WebPlugin implements DipCore {
  private store = new Store()
  private identity = identity
  private paired = new Set<string>()
  private refused = new Map<string, number>()
  private gates = new Map<number, Person>()
  private live = new Map<number, Person>()
  private transfer: Transfer | undefined
  private nextLink = 1
  private clock: ReturnType<typeof setTimeout> | undefined

  constructor() {
    super()

    for (const person of people.filter(person => person.known)) {
      this.paired.add(person.pubkey)
    }

    // Reachable as `dip` in the console, so a gate with no screen yet can still be answered.
    Object.assign(window, {dip: this})
  }

  // ------------------------------------------------------ the neighborhood

  /** What the device had already collected before this session opened. */
  private async backlog() {
    const at = now()
    const busiest = faker.number.int({min: 2, max: BAND_HOURS - 2})
    const known = people.filter(person => this.paired.has(person.pubkey))

    this.cards(at - DAY)

    for (let hour = 0; hour < BAND_HOURS; hour++) {
      const many = hour === busiest
      const count = faker.number.int(many ? {min: 4, max: 6} : {min: 0, max: 2})

      for (let index = 0; index < count; index++) {
        await this.arrive(
          faker.helpers.arrayElement(known),
          at - hour * HOUR - faker.number.int(HOUR),
        )
      }
    }

    for (let index = 0; index < 4; index++) {
      await this.arrive(
        faker.helpers.arrayElement(known),
        at - faker.number.int({min: 2 * DAY, max: 26 * DAY}),
      )
    }

    for (const person of known) {
      await this.respond(person, at - faker.number.int(6 * HOUR))
      this.handOn(person, 6, at - faker.number.int(6 * HOUR))
    }
  }

  /**
   * Who calls whom what, one card per person.
   *
   * The device's own cards are what it learned at pairing; a neighbour's card
   * for somebody is how a person it never met arrives with a name on them.
   */
  private cards(at: number) {
    const known = people.filter(person => person.known)

    for (const person of known) {
      this.store.record(card(this.identity, person.pubkey, person.petname, at), identity, at)
    }

    for (const person of known) {
      const theirs = faker.helpers.arrayElements(
        people.filter(other => other.pubkey !== person.pubkey),
        3,
      )

      for (const other of theirs) {
        this.store.record(card(person.pubkey, other.pubkey, other.petname, at), person.pubkey, at)
      }
    }
  }

  /**
   * A reaction, a comment or a boost on something the device already holds.
   *
   * A comment can be commented on in its turn, which is how a conversation more
   * than one deep exists to be read.
   */
  private async respond(person: Person, at: number) {
    const held = this.store
      .list({order: "seenAt", filter: JSON.stringify({kinds: [NOTE, COMMENT], limit: 20})})
      .filter(detail => detail.event.pubkey !== person.pubkey)
    const about = held.length ? faker.helpers.arrayElement(held).event : undefined

    if (!about) return

    const shape = faker.helpers.weightedArrayElement([
      {weight: 5, value: REACTION},
      {weight: 3, value: COMMENT},
      {weight: 2, value: REPOST},
    ])

    const writer =
      shape === REACTION
        ? reaction.writer().setEvent(about).setContent(emoji())
        : shape === COMMENT
          ? comment.writer().replyTo(about).setContent(remark())
          : boostOf(about)

    const written = hash({
      ...(await writer.renderTemplate()),
      created_at: at,
      pubkey: person.pubkey,
    })

    this.store.record(written, person.pubkey, at)
  }

  /** One event handed over by a person, authored some time before they carried it here. */
  private async arrive(person: Person, at: number) {
    const template = await post()
    const written = at - faker.number.int({min: 0, max: 6 * HOUR})
    const event = hash({...template, created_at: written, pubkey: person.pubkey})

    this.store.record(event, person.pubkey, at)

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

  /**
   * What this device hands the peer: whatever it holds that they did not bring.
   *
   * The core serves a peer's `REQ` out of the store and records who each event
   * went to, which is what a detail screen reads. None of the negotiation is
   * modelled — the observable shape is that somebody on an open link leaves
   * with things.
   */
  private handOn(person: Person, count: number, at: number) {
    const unheld = this.store
      .list({order: "seenAt", filter: JSON.stringify({limit: 30})})
      .filter(detail => !detail.sightings.some(sighting => sighting.pubkey === person.pubkey))

    for (const detail of faker.helpers.arrayElements(unheld, {min: 0, max: count})) {
      this.store.share(detail.event.id, person.pubkey, at)
    }
  }

  /** Everything one encounter produces: a gate if they are a stranger, then a burst. */
  private async encounter() {
    const person = faker.helpers.arrayElement(people)
    const link = this.nextLink++

    if (this.paired.has(person.pubkey)) {
      this.open(link, person)
      await this.gossip(person, faker.number.int({min: 1, max: person.talkative}))
    } else {
      this.gate(person, link)
      await this.gossip(person, 1)
    }

    this.tick()
  }

  /**
   * A session both ends authenticated, which is the only thing a transfer runs
   * over.
   *
   * Announced the way the core announces it, and closed ninety seconds later
   * when they have walked far enough — the view is told both, so a screen
   * naming a link knows when there is nothing behind it.
   */
  private open(link: number, person: Person) {
    this.live.set(link, person)
    this.notifyListeners("peerIdentified", {link, pubkey: person.pubkey})
    log(
      `link ${link} is up with ${short(person.pubkey)} — dip.receiveOffer(${link}) to be offered their key`,
    )

    setTimeout(() => this.close(link), 90_000)
  }

  /** They walked out of range, which ends anything running over the link. */
  private close(link: number) {
    if (this.live.delete(link)) {
      if (this.transfer?.link === link) this.finish(link, "refused")

      this.notifyListeners("linkClosed", {link})
      log(`link ${link} went down`)
    }
  }

  /** A stranger asking, which is a question for the user and not an event. */
  private gate(person: Person, link: number) {
    const asked = this.refused.get(person.pubkey) ?? 0

    if (Date.now() - asked > GATE_BACKOFF) {
      this.gates.set(link, person)
      this.notifyListeners("requestApproval", {link, code: pairingCode()})
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
  private async gossip(person: Person, count: number) {
    const at = now()
    let fresh = 0

    for (let index = 0; index < count; index++) {
      const again = faker.datatype.boolean({probability: 0.3}) && this.resight(person, at)

      if (!again) {
        await this.arrive(person, at)
        fresh++
      }
    }

    if (faker.datatype.boolean({probability: 0.5})) await this.respond(person, at)

    this.handOn(person, 3, at)
    this.notifyListeners("storeChanged", {group: "events"})
    log(`${short(person.pubkey)} handed over ${count} (${fresh} new)`)
  }

  /** One clock, however many times `start` is called, or the street doubles in size. */
  private tick() {
    clearTimeout(this.clock)

    this.clock = setTimeout(
      () => void this.encounter(),
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
    await this.backlog()
    log(`started as ${short(this.identity)}, ${people.length} people in range`)
    this.tick()

    return {identity: this.identity}
  }

  // ------------------------------------------------------------- the store

  async mediaTags({media}: {media: string}) {
    return {entries: describe(bytes(media))}
  }

  async publish({event, media = []}: {event: string; media?: string[]}) {
    const published = JSON.parse(event) as HashedEvent
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
      this.open(link, person)
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

  async forgetEvent({id}: {id: string}) {
    const existed = this.store.forget(id)

    if (existed) {
      this.notifyListeners("storeChanged", {group: "events"})
      log(`dropped ${id.slice(0, 8)} from this device`)
    }

    return {existed}
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

  async policy() {
    return {policy: JSON.stringify(compiled(this.store))}
  }

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

  async exportKey({password}: {password?: string} = {}) {
    if (password !== undefined && password.length < 12) {
      throw new Error("a backup password has to be at least 12 characters")
    }

    log("wrote a key backup and put it in front of the user")
    this.notifyListeners("keyBackupShared", {shared: true})
  }

  async offerIdentity({link}: {link: number}) {
    if (!this.live.has(link)) throw new Error(`link ${link} has no session to transfer over`)
    if (this.transfer) throw new Error("an identity transfer is already running")

    this.invite(link, true)

    // The other device's user compares the digits while this one is still looking.
    setTimeout(() => this.peerCompares(link), 2_500)
  }

  /**
   * The peer on a live link offers this device its identity, which is the half
   * of the flow no screen can start.
   *
   * Console-only on purpose: a paired phone volunteering its key unprompted is
   * not something the neighborhood should do on a timer.
   */
  receiveOffer(link: number) {
    if (!this.live.has(link)) throw new Error(`link ${link} has no session to transfer over`)
    if (this.transfer) throw new Error("an identity transfer is already running")

    this.invite(link, false)
  }

  async answerIdentityTransfer({link, confirmed}: {link: number; confirmed: boolean}) {
    const running = this.transfer

    if (running?.link !== link) throw new Error(`no identity transfer is waiting on link ${link}`)

    if (!confirmed) {
      this.finish(link, "refused")
    } else if (running.source) {
      running.confirmed = true

      if (running.accepted) this.finish(link, "sent")
    } else {
      this.finish(link, "received")
    }
  }

  async takeTransferredIdentity({link}: {link: number}) {
    log(`adopted the identity offered on link ${link}`)

    return {identity: this.identity}
  }

  /**
   * Nothing: a browser has no back button to claim.
   *
   * `notifyListeners("backPressed", {})` from the console is how the trail is
   * driven here, the same way a pairing request is.
   */
  async setCanGoBack() {}

  /** Put the same six digits in front of both users. */
  private invite(link: number, source: boolean) {
    const code = shortCode()

    this.transfer = {link, source, confirmed: false, accepted: false}
    this.notifyListeners("confirmIdentityTransfer", {link, code})
    log(`identity transfer on link ${link}, code ${code}`)
  }

  /** The target's user said yes, which releases the key if this one already had. */
  private peerCompares(link: number) {
    const running = this.transfer

    if (running?.link === link && running.source) {
      running.accepted = true

      if (running.confirmed) this.finish(link, "sent")
    }
  }

  /** How it ended, told once, the way the core tells whoever was waiting. */
  private finish(link: number, outcome: "received" | "sent" | "refused") {
    this.transfer = undefined
    this.notifyListeners("identityTransfer", {link, outcome})
    log(`identity transfer on link ${link} was ${outcome}`)
  }
}

/**
 * One simulated core per page, which is what `registerPlugin` asks the web for.
 *
 * It loads the web implementation behind an await and caches it afterwards, so
 * two calls made before the first one lands each build their own. Two stores
 * means half the app reading an empty one.
 */
let running: Simulator | undefined

export const simulated = () => (running ??= new Simulator())
