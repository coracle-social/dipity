// The tables the simulated core answers out of, and the read the real one would have run.
//
// The shapes are the core's and come from `$lib/core`, which is where the
// bridge contract is written down: an event is a `HashedEvent`, a sighting is a
// row of `event_seen`, and a blob is `model::blob::Blob` in snake_case, because
// what crosses the bridge is serde JSON of the Rust types.
// `docs/storage.md#the-schema`.

import {matchFilter, type Filter, type HashedEvent} from "@welshman/util"
import type {Blob, EventDetail, Query} from "$lib/core"

const parse = (filter?: string): Filter => (filter ? (JSON.parse(filter) as Filter) : {})

/** An event's seen time is the earliest peer that handed it over. `storage.md`. */
const seenAt = (detail: EventDetail) =>
  Math.min(...detail.sightings.map(sighting => sighting.seen_at))

const withinProvenance = (detail: EventDetail, query: Query) => {
  const seen = seenAt(detail)
  const from = query.seenFrom

  return (
    (query.seenSince === undefined || seen >= query.seenSince) &&
    (query.seenUntil === undefined || seen <= query.seenUntil) &&
    (!from || detail.sightings.some(sighting => from.includes(sighting.pubkey)))
  )
}

/** Newest first, ties broken by id, which is what both of the core's orderings do. */
const byRecency = (order: Query["order"]) => (a: EventDetail, b: EventDetail) => {
  const key =
    order === "createdAt" ? b.event.created_at - a.event.created_at : seenAt(b) - seenAt(a)

  return key || a.event.id.localeCompare(b.event.id)
}

/** Everything the simulated core has, and the only place it is written. */
export class Store {
  private details = new Map<string, EventDetail>()
  private blobs = new Map<string, Blob>()
  private preferences = new Map<string, {value: string; updatedAt: number}>()

  /** Store an event and a sighting of it, answering whether the event itself was new. */
  record(event: HashedEvent, from: string, at: number, blobs: Blob[] = []) {
    const sighting = {event_id: event.id, pubkey: from, seen_at: at}
    const known = this.details.get(event.id)

    for (const blob of blobs) {
      this.blobs.set(blob.sha256, this.blobs.get(blob.sha256) ?? blob)
    }

    if (known) {
      known.sightings.push(sighting)
    } else {
      this.details.set(event.id, {event, blobs, sightings: [sighting]})
    }

    return !known
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
