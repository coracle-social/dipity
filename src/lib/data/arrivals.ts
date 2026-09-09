// What came in, and what the day it came in on looked like.
//
// The home screen is ordered by arrival rather than by the author's claimed
// time, so every query here sets `seenAt` and every shape derived below is
// about when a thing reached this device. `docs/storage.md#the-schema`.

import {readable, type Readable} from "svelte/store"
import type {PluginListenerHandle} from "@capacitor/core"
import {Dip, type Query} from "$lib/core"
import type {Session} from "$lib/data/session"

const HOUR = 3600

const DAY = 86_400

/** How far back the band looks. One wider than a phone is unreadable. */
export const WINDOW_HOURS = 12

/** One stored event, reduced to what an arrival is: a thing, and how it got here. */
export type Arrival = {
  id: string
  kind: number
  pubkey: string
  content: string
  /** When it first reached this device, which is what the screen is ordered by. */
  seenAt: number
  /** The most recent sighting, which is what the retention sweep measures. */
  lastSeenAt: number
  /** The peers it was seen from, earliest first. Never leaves the device. */
  from: string[]
}

/** How far through its retention window an arrival is. */
export type Warmth = "warm" | "fading" | "cold" | "kept"

/** One hour of the band: when it started, and how much landed in it. */
export type Hour = {at: number; count: number}

/** The day, in the words the screen says it in. */
export type Day = {headline: string; detail: string; quiet: boolean}

type Sighting = {seen_at: number; pubkey: string}

type Detail = {
  event: {id: string; kind: number; pubkey: string; content: string; created_at: number}
  sightings: Sighting[]
}

const toArrival = ({event, sightings}: Detail): Arrival => ({
  id: event.id,
  kind: event.kind,
  pubkey: event.pubkey,
  content: event.content,
  seenAt: Math.min(...sightings.map(sighting => sighting.seen_at)),
  lastSeenAt: Math.max(...sightings.map(sighting => sighting.seen_at)),
  from: sightings.map(sighting => sighting.pubkey),
})

/** A read that failed is an empty screen, since the query is the only thing that knows better. */
const unread = (error: unknown): Arrival[] => {
  console.error("the store could not be read", error)

  return []
}

/**
 * One query, re-read whenever the store moves.
 *
 * Re-read rather than patched: the core announces the group that changed and
 * not the rows, so the query is the only thing that knows what belongs on
 * screen. The query is a thunk because a window slides with the clock.
 */
const arrivalStore = (asked: () => Query): Readable<Arrival[]> =>
  readable<Arrival[]>([], set => {
    let live = true
    let handle: PluginListenerHandle | undefined

    const read = async () => {
      const found = await Dip.listDetails(asked())
        .then(({details}) => details.map(detail => toArrival(JSON.parse(detail) as Detail)))
        .catch(unread)

      if (live) set(found)
    }

    Dip.addListener("storeChanged", ({group}) => {
      if (group === "events") read()
    })
      .then(listener => {
        handle = listener

        if (!live) listener.remove()
      })
      .catch(() => undefined)

    read()

    return () => {
      live = false
      handle?.remove()
    }
  })

/** The start of the band's oldest hour, which is what `today` and `hoursOf` share. */
const bandStart = (now: number) => Math.floor(now / HOUR) * HOUR - (WINDOW_HOURS - 1) * HOUR

/** Everything that arrived inside the band's window, newest first. */
export const today = arrivalStore(() => ({
  order: "seenAt",
  seenSince: bandStart(Date.now() / 1000),
}))

/**
 * The last few arrivals, however long ago they came.
 *
 * Not the window above: a quiet day would show an empty screen, and the older
 * end of this list is where an arrival is visibly cooling.
 */
export const recent = arrivalStore(() => ({
  order: "seenAt",
  filter: JSON.stringify({limit: 8}),
}))

/**
 * Whether the retention sweep will ever take an arrival.
 *
 * The core's rule, restated rather than taken from `isReplaceableKind` in
 * `@welshman/util`: that one counts addressable kinds as replaceable and the
 * sweep does not, so a classified would read as permanent when it is not.
 * `core/dip/src/db/event/command.rs`.
 */
const spared = (arrival: Arrival, identity?: string) =>
  arrival.pubkey === identity ||
  arrival.kind === 0 ||
  arrival.kind === 3 ||
  (arrival.kind >= 10_000 && arrival.kind < 20_000)

/** When the sweep takes an arrival, or undefined for one it never will. */
export const sweptAt = (arrival: Arrival, session: Session) =>
  spared(arrival, session.identity) ? undefined : arrival.lastSeenAt + session.retentionDays * DAY

/** How much of an arrival's life is left, as the one word the screen ever says. */
export const warmthOf = (arrival: Arrival, swept: number | undefined, now: number): Warmth => {
  if (swept) {
    const spent = (now - arrival.lastSeenAt) / (swept - arrival.lastSeenAt)

    return spent < 0.5 ? "warm" : spent < 0.9 ? "fading" : "cold"
  } else {
    return "kept"
  }
}

/** The window as hourly buckets, oldest first, including the empty ones. */
export const hoursOf = (found: Arrival[], now: number): Hour[] => {
  const start = bandStart(now)
  const hours: Hour[] = []

  for (let index = 0; index < WINDOW_HOURS; index++) {
    hours.push({at: start + index * HOUR, count: 0})
  }

  for (const arrival of found) {
    const index = Math.floor((arrival.seenAt - hours[0].at) / HOUR)

    if (index >= 0 && index < hours.length) {
      hours[index].count++
    }
  }

  return hours
}

const volume = (count: number) => {
  if (count > 4) {
    return "A lot came in"
  } else {
    return count > 1 ? "A few things came in" : "One thing came in"
  }
}

const things = (count: number) => (count === 1 ? "One thing" : `${count} things`)

const people = (count: number) => (count === 1 ? "one person" : `${count} people`)

/**
 * What the day is, said out loud.
 *
 * The band is the shape and this is the caption on it, so the two read the
 * same window: a busiest hour, a count, and whether it has gone quiet since.
 */
export const describeDay = (found: Arrival[], hours: Hour[], now: number): Day => {
  if (found.length) {
    const busiest = hours.reduce((most, hour) => (hour.count > most.count ? hour : most))
    const latest = Math.max(...found.map(arrival => arrival.seenAt))
    const senders = new Set(found.map(arrival => arrival.pubkey)).size
    const quiet = now - latest > HOUR
    const count = `${things(found.length)}, from ${people(senders)}.`
    const hour = new Date(busiest.at * 1000).toLocaleTimeString(undefined, {hour: "numeric"})
    const since = new Date(latest * 1000).toLocaleTimeString(undefined, {timeStyle: "short"})

    return {
      headline: `${volume(found.length)} around ${hour}.`,
      detail: quiet ? `Then quiet since ${since}. ${count}` : count,
      quiet,
    }
  } else {
    return {
      headline: "Nothing has come in.",
      detail: `Quiet for ${WINDOW_HOURS} hours.`,
      quiet: true,
    }
  }
}
