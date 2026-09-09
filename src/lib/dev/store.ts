// The tables the simulated core answers out of, and the read the real one would have run.
//
// The shapes are the core's rather than the view's: an event is NIP-01, a
// sighting is a row of `event_seen`, and a blob is `model::blob::Blob` in
// snake_case, because what crosses the bridge is serde JSON of the Rust types.
// `docs/storage.md#the-schema`.

import type {Query} from "$lib/core"

/** One event as the core stores it — hashed, and never signed. */
export type StoredEvent = {
  id: string
  pubkey: string
  created_at: number
  kind: number
  tags: string[][]
  content: string
}

/** One row of `event_seen`: which peer handed an event over, and when. */
export type Sighting = {event_id: string; pubkey: string; seen_at: number}

/** A blob an event references, whether or not the bytes are held. */
export type Blob = {
  sha256: string
  role: "Preview" | "Original"
  url: string | null
  mime_type: string | null
  size: number | null
  dim: string | null
  blurhash: string | null
  alt: string | null
  blake3: string | null
  imeta: string[]
  stored_bytes: number
  complete: boolean
  accessed_at: number | null
}

/** What `listDetails` answers per event: the thing, its media, and how it got here. */
export type Detail = {event: StoredEvent; blobs: Blob[]; sightings: Sighting[]}

/** The NIP-01 fields the core's query builder narrows on. */
type Filter = {
  ids?: string[]
  authors?: string[]
  kinds?: number[]
  since?: number
  until?: number
  limit?: number
}

const parse = (filter?: string): Filter => (filter ? (JSON.parse(filter) as Filter) : {})

/** An event's seen time is the earliest peer that handed it over. `storage.md`. */
const seenAt = (detail: Detail) => Math.min(...detail.sightings.map(sighting => sighting.seen_at))

const withinFilter = (event: StoredEvent, filter: Filter) =>
  (!filter.ids || filter.ids.includes(event.id)) &&
  (!filter.authors || filter.authors.includes(event.pubkey)) &&
  (!filter.kinds || filter.kinds.includes(event.kind)) &&
  (filter.since === undefined || event.created_at >= filter.since) &&
  (filter.until === undefined || event.created_at <= filter.until)

const withinProvenance = (detail: Detail, query: Query) => {
  const seen = seenAt(detail)
  const from = query.seenFrom

  return (
    (query.seenSince === undefined || seen >= query.seenSince) &&
    (query.seenUntil === undefined || seen <= query.seenUntil) &&
    (!from || detail.sightings.some(sighting => from.includes(sighting.pubkey)))
  )
}

/** Newest first, ties broken by id, which is what both of the core's orderings do. */
const byRecency = (order: Query["order"]) => (a: Detail, b: Detail) => {
  const key =
    order === "createdAt" ? b.event.created_at - a.event.created_at : seenAt(b) - seenAt(a)

  return key || a.event.id.localeCompare(b.event.id)
}

/** Everything the simulated core has, and the only place it is written. */
export class Store {
  private details = new Map<string, Detail>()
  private blobs = new Map<string, Blob>()
  private preferences = new Map<string, {value: string; updatedAt: number}>()

  /** Store an event and a sighting of it, answering whether the event itself was new. */
  record(event: StoredEvent, from: string, at: number, blobs: Blob[] = []) {
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

  /** The events matching a query, narrowed and ordered the way the core would. */
  list(query: Query = {}): Detail[] {
    const filter = parse(query.filter)
    const found = [...this.details.values()]
      .filter(detail => withinFilter(detail.event, filter) && withinProvenance(detail, query))
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
