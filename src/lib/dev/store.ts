// The tables the simulated core answers out of, and the read the real one would have run.
//
// The shapes are the core's and come from `$lib/core`, which is where the
// bridge contract is written down: an event is a `HashedEvent`, a sighting is a
// row of `event_seen`, a share is a row of `event_shared`, and a blob is
// `model::blob::Blob` in snake_case, because what crosses the bridge is serde
// JSON of the Rust types. `docs/storage.md#the-schema`.

import {
  getIdOrAddress,
  isEphemeral,
  isReplaceable,
  matchFilter,
  type Filter,
  type HashedEvent,
} from "@welshman/util"
import type {Blob, EventDetail, Query} from "$lib/core"

const parse = (filter?: string): Filter => (filter ? (JSON.parse(filter) as Filter) : {})

/** An event's seen time is the earliest peer that handed it over. `storage.md`. */
const seenAt = (detail: EventDetail) =>
  Math.min(...detail.sightings.map(sighting => sighting.seen_at))

/** NIP-01's rule for which of two versions of one address wins. */
const supersedes = (event: HashedEvent, current: HashedEvent) =>
  event.created_at > current.created_at ||
  (event.created_at === current.created_at && event.id < current.id)

const withinProvenance = (detail: EventDetail, query: Query) => {
  const seen = seenAt(detail)
  const from = query.seenFrom

  return (
    (query.seenSince === undefined || seen >= query.seenSince) &&
    (query.seenUntil === undefined || seen <= query.seenUntil) &&
    (!from || detail.sightings.some(sighting => from.includes(sighting.pubkey)))
  )
}

/**
 * Newest first, ties broken by id, which is what both of the core's orderings do.
 *
 * A query that names no order gets `createdAt`, which is the default on the
 * uniffi record and what both shells send for an absent one.
 */
const byRecency = (order: Query["order"]) => (a: EventDetail, b: EventDetail) => {
  const key = order === "seenAt" ? seenAt(b) - seenAt(a) : b.event.created_at - a.event.created_at

  return key || a.event.id.localeCompare(b.event.id)
}

/** Everything the simulated core has, and the only place it is written. */
export class Store {
  private details = new Map<string, EventDetail>()
  private blobs = new Map<string, Blob>()
  private preferences = new Map<string, {value: string; updatedAt: number}>()
  /** Events the author's signature arrived with, which is what makes them forwardable. */

  /**
   * Store an event and a sighting of it, answering whether the event itself was new.
   *
   * Refused the same three ways the core refuses: an ephemeral kind is never
   * stored, and a replaceable one only when it beats whatever holds its address
   * — later, or equal and lower id. A simulator that keeps every version is more
   * forgiving than the core, and an edit the core silently drops then looks like
   * it landed. `core/dip/src/db/event/command.rs`.
   */
  record(event: HashedEvent, from: string, at: number, blobs: Blob[] = []) {
    const sighting = {event_id: event.id, pubkey: from, seen_at: at}
    const known = this.details.get(event.id)

    if (known) {
      // One sighting per peer, which is what `event_seen`'s primary key enforces.
      if (!known.sightings.some(seen => seen.pubkey === from)) known.sightings.push(sighting)

      return false
    }

    if (isEphemeral(event)) return false

    const current = isReplaceable(event) ? this.atAddress(event) : undefined

    if (current && !supersedes(event, current.event)) return false

    for (const blob of blobs) {
      this.blobs.set(blob.sha256, this.blobs.get(blob.sha256) ?? blob)
    }

    if (current) this.forget(current.event.id)

    this.details.set(event.id, {event, blobs, sightings: [sighting], shares: []})

    return true
  }

  /**
   * Record that an event was handed to a peer, answering whether that is new.
   *
   * One row per peer, the way `event_shared`'s primary key is: serving somebody
   * what they already have is reconciliation rather than another share.
   */
  share(id: string, pubkey: string, at: number) {
    const detail = this.details.get(id)

    if (!detail || detail.shares.some(share => share.pubkey === pubkey)) return false

    detail.shares.push({event_id: id, pubkey, shared_at: at})

    return true
  }

  /** Whatever currently holds an event's address, for a kind that has one. */
  private atAddress(event: HashedEvent) {
    const address = getIdOrAddress(event)

    return [...this.details.values()].find(detail => getIdOrAddress(detail.event) === address)
  }

  /**
   * The events matching a query, narrowed and ordered the way the core would.
   *
   * The NIP-01 half is `matchFilter`, so tags narrow here the way they narrow
   * on a device. Its `search` is a substring where the core's is NIP-50 over an
   * FTS index — the one constraint whose answer differs.
   */
  list(query: Query = {}): EventDetail[] {
    const filter = parse(query.filter)
    const found = [...this.details.values()]
      .filter(detail => matchFilter(filter, detail.event) && withinProvenance(detail, query))
      .sort(byRecency(query.order))

    return filter.limit === undefined ? found : found.slice(0, filter.limit)
  }

  get(id: string) {
    return this.details.get(id)
  }

  /** Drop an event and everything hanging off it, the way the core cascades. */
  forget(id: string) {
    const detail = this.details.get(id)

    if (!detail) return false

    this.details.delete(id)

    for (const blob of detail.blobs) {
      if (this.referencing(blob.sha256).length === 0) this.blobs.delete(blob.sha256)
    }

    return true
  }

  /** Every blob referenced by a stored event and not held in full. */
  wanted(limit?: number) {
    const found = [...this.blobs.values()].filter(blob => !blob.complete)

    return limit === undefined ? found : found.slice(0, limit)
  }

  blob(sha256: string) {
    return this.blobs.get(sha256) ?? null
  }

  /** Mark a blob whole, which is what publishing the bytes behind an `imeta` does. */
  hold(sha256: string, size: number) {
    const blob = this.blobs.get(sha256)

    if (blob) {
      this.blobs.set(sha256, {...blob, size, stored_bytes: size, complete: true})
    }
  }

  referencing(sha256: string) {
    return [...this.details.values()]
      .filter(detail => detail.blobs.some(blob => blob.sha256 === sha256))
      .map(detail => detail.event.id)
  }

  prefs() {
    return [...this.preferences].map(([key, {value, updatedAt}]) => ({key, value, updatedAt}))
  }

  pref(key: string) {
    return this.preferences.get(key)?.value ?? null
  }

  setPref(key: string, value: string, at: number) {
    this.preferences.set(key, {value, updatedAt: at})
  }

  clearPref(key: string) {
    return this.preferences.delete(key)
  }
}
