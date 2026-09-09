// The street `just dev` runs on: who keeps turning up, and what they hand over.
//
// Everything is faker output under a fixed seed, so a reload is the same
// neighborhood rather than a new one and two screenshots are comparable.
// Nothing here knows about time or about the store — the simulator decides when
// a person walks past and what happens when they do.

import {faker} from "@faker-js/faker"
import {CLASSIFIED, LONG_FORM, NOTE} from "@welshman/util"
import type {Blob} from "$lib/dev/store"

/** What makes the same reload the same street. */
const SEED = 20_214

/** How many people are within radio range of this device over a session. */
const POPULATION = 7

faker.seed(SEED)

/** A key-shaped 32 bytes. Nothing below the bridge checks it, and no key here signs. */
const key = () => faker.string.hexadecimal({length: 64, casing: "lower", prefix: ""})

/** Somebody the device keeps running into. */
export type Person = {
  pubkey: string
  /** The most events they hand over in one burst, once they are paired. */
  talkative: number
  /** Whether this device had already paired with them before the session started. */
  known: boolean
}

/** Something one of them wrote. */
export type Post = {kind: number; content: string; tags: string[][]; blobs: Blob[]}

/** The device's own identity, which the simulated shell reports as already stored. */
export const identity = key()

/** Everyone in range, the first few already paired so the screen opens with a history. */
export const people: Person[] = Array.from({length: POPULATION}, (_, index) => ({
  pubkey: key(),
  talkative: faker.number.int({min: 1, max: 5}),
  known: index < 3,
}))

const street = () => `${faker.location.buildingNumber()} ${faker.location.street()}`

const notes = [
  () => `${faker.food.dish()}, far too much of it. Spare portions at ${street()}.`,
  () => `Anyone got a ${faker.number.int({min: 6, max: 19})}mm socket I can borrow for an hour?`,
  () =>
    `Bins move to ${faker.date.weekday()} from next week, the whole of ${faker.location.street()}.`,
  () => `${faker.person.firstName()} is out looking for their ${faker.animal.dog()} again.`,
  () =>
    `Two of us at the market from ${faker.number.int({min: 7, max: 10})} if anyone wants a hand carrying.`,
  () => `Reading group has moved to the ${faker.company.buzzNoun()}, ${faker.date.weekday()}s.`,
  () => `Power was out on ${faker.location.street()} for an hour. Anyone else?`,
]

const picture = (): Blob => {
  const size = faker.number.int({min: 40_000, max: 900_000})
  const sha256 = faker.string.hexadecimal({length: 64, casing: "lower", prefix: ""})

  return {
    sha256,
    role: "Original",
    url: null,
    mime_type: faker.helpers.arrayElement(["image/jpeg", "image/png", "image/webp"]),
    size,
    dim: `${faker.number.int({min: 600, max: 2400})}x${faker.number.int({min: 600, max: 2400})}`,
    blurhash: null,
    alt: faker.lorem.words({min: 2, max: 6}),
    blake3: faker.string.hexadecimal({length: 64, casing: "lower", prefix: ""}),
    imeta: [`x ${sha256}`, `size ${size}`],
    stored_bytes: 0,
    complete: false,
    accessed_at: null,
  }
}

/** A `imeta` tag naming a blob, which is the only way media reaches an event. */
const imeta = (blob: Blob) => ["imeta", ...blob.imeta]

const classified = (): Post => ({
  kind: CLASSIFIED,
  content: `${faker.commerce.productName()}\n${faker.commerce.price({min: 0, max: 80, symbol: "£"})}, ${street()}`,
  tags: [["title", faker.commerce.productName()]],
  blobs: [],
})

const article = (): Post => ({
  kind: LONG_FORM,
  content: `${faker.book.title()}\n${faker.lorem.sentence()}`,
  tags: [
    ["d", faker.lorem.slug()],
    ["title", faker.book.title()],
  ],
  blobs: [],
})

const note = (): Post => {
  const blobs = faker.datatype.boolean({probability: 0.25}) ? [picture()] : []

  return {
    kind: NOTE,
    content: faker.helpers.arrayElement(notes)(),
    tags: blobs.map(imeta),
    blobs,
  }
}

// Only kinds the view can render: a kind 0 gossips too, and today it draws as its own JSON.
/** One thing somebody hands over, weighted the way a street is: mostly talk. */
export const post = (): Post =>
  faker.helpers.weightedArrayElement([
    {weight: 8, value: note},
    {weight: 2, value: classified},
    {weight: 1, value: article},
  ])()

/** The six digits both devices show during a login-with-device. `docs/keys.md`. */
export const sas = () => faker.number.int({min: 0, max: 999_999})
