// A neighborhood, for `just dev`.
//
// The browser has no plugin behind the proxy, so every core call rejects and
// the home screen would only ever render its empty state. This is what it
// renders instead, and it is loaded by a dynamic import under
// `import.meta.env.DEV` so the built bundle the shells load never contains it.

import type {Arrival} from "$lib/data/arrivals"
import type {Query} from "$lib/core"

/** The identity `session` reports when the plugin is not there to answer. */
export const SAMPLE_IDENTITY = "9f2ba1d0c74e3b586a0d41c9e8b7523f4d16ae90c3b8f7215d6e0a4c9b3f8127"

const PEOPLE = [
  "3c1f8a0d5e2b74961c8d0fa3b52e7d419068fc2a5b3e91d7042f6c8a1b9d3e05",
  "7a4e2c9b18d360f5a72c4e9018bd35f6072a1c8e4d95b307f28e6a1d0c4b9375",
  "0d938f1a6c2e457b83f0a91d2c6b48e57930a2f1c8d64b095e37a1f8c02d5b64",
  "b62d0f37a918c4e50d73b28fa16c9e40358d7b12f0a96c48d5e2b70a3f184c9d",
]

type Seed = {hoursAgo: number; kind: number; person: number; content: string}

const SEEDS: Seed[] = [
  {
    hoursAgo: 0.6,
    kind: 30402,
    person: 0,
    content: "Bike trailer, needs a tyre\nFree, 14 Ellis, side gate",
  },
  {
    hoursAgo: 1.2,
    kind: 1,
    person: 1,
    content: "Two crates of plums, Saturday\nThe allotment, from nine",
  },
  {
    hoursAgo: 1.4,
    kind: 1,
    person: 2,
    content: "Anyone got a 10mm socket I can borrow for an hour?",
  },
  {
    hoursAgo: 3.5,
    kind: 1,
    person: 0,
    content: "Bins move to Thursday from next week, the whole of Ellis Street",
  },
  {
    hoursAgo: 4.1,
    kind: 30023,
    person: 3,
    content: "The Bight, in six pages\nWhat the tide leaves behind, and who walks it",
  },
  {
    hoursAgo: 8.5,
    kind: 1,
    person: 1,
    content: "Two of us at the market from eight if anyone wants a hand carrying",
  },
  {
    hoursAgo: 18 * 24,
    kind: 1,
    person: 2,
    content: "Reading group has moved to the laundromat, Sundays",
  },
  {
    hoursAgo: 28 * 24,
    kind: 1,
    person: 3,
    content: "Sourdough starter, spare jar, ask at the corner",
  },
]

const toArrival = (now: number, seed: Seed, index: number): Arrival => {
  const seenAt = Math.floor(now - seed.hoursAgo * 3600)

  return {
    id: `sample-${index}`,
    kind: seed.kind,
    pubkey: PEOPLE[seed.person],
    content: seed.content,
    seenAt,
    lastSeenAt: seenAt,
    from: [PEOPLE[(seed.person + 1) % PEOPLE.length]],
  }
}

/** The fixture, narrowed the way the core would have narrowed it. */
export const sampleArrivals = (now: number, query: Query): Arrival[] => {
  const {limit} = query.filter ? (JSON.parse(query.filter) as {limit?: number}) : {limit: undefined}
  const since = query.seenSince ?? 0

  return SEEDS.map((seed, index) => toArrival(now, seed, index))
    .filter(arrival => arrival.seenAt >= since)
    .slice(0, limit)
}
