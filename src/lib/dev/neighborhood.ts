// The street `just dev` runs on: who keeps turning up, and what they hand over.
//
// Everything is faker output under a fixed seed, so a reload is the same
// neighborhood rather than a new one and two screenshots are comparable.
// Nothing here knows about time or about the store — the simulator decides when
// a person walks past and what happens when they do.
//
// The first version of the app carries text, so nothing here attaches a file.
//
// A topic is a plain `t` tag, so the slugs below are spelled again rather than
// taken from `$lib/kinds`. A peer agrees with the app about the wire and knows
// nothing else about it.

import {faker} from "@faker-js/faker"
import type {EventTemplate} from "@welshman/util"
import {article, dateEvent, note, poll, timeEvent} from "$lib/dev/kinds"

faker.seed(20_214)

/** A key-shaped 32 bytes. Nothing below the bridge checks it, and no key here signs. */
const key = () => faker.string.hexadecimal({length: 64, casing: "lower", prefix: ""})

/** Somebody the device keeps running into. */
export type Person = {
  pubkey: string
  /** What people who have paired with them call them. */
  petname: string
  /** The most events they hand over in one burst, once they are paired. */
  talkative: number
  /** Whether this device had already paired with them before the session started. */
  known: boolean
}

/** The device's own identity, which the simulated shell reports as already stored. */
export const identity = key()

/** Everyone in range, the first few already paired so the screen opens with a history. */
export const people: Person[] = Array.from({length: 7}, (_, index) => ({
  pubkey: key(),
  petname: faker.person.firstName(),
  talkative: faker.number.int({min: 1, max: 5}),
  known: index < 3,
}))

const street = () => `${faker.location.buildingNumber()} ${faker.location.street()}`

const notes = [
  {
    topic: "spare",
    say: () => `${faker.food.dish()}, far too much of it. Spare portions at ${street()}.`,
  },
  {
    topic: "help",
    say: () =>
      `Anyone got a ${faker.number.int({min: 6, max: 19})}mm socket I can borrow for an hour?`,
  },
  {
    topic: "notices",
    say: () =>
      `Bins move to ${faker.date.weekday()} from next week, the whole of ${faker.location.street()}.`,
  },
  {
    topic: "lost-and-found",
    say: () => `${faker.person.firstName()} is out looking for their ${faker.animal.dog()} again.`,
  },
  {
    topic: "help",
    say: () =>
      `Two of us at the market from ${faker.number.int({min: 7, max: 10})} if anyone wants a hand carrying.`,
  },
  {
    topic: "meetups",
    say: () =>
      `Reading group has moved to the ${faker.company.buzzNoun()}, ${faker.date.weekday()}s.`,
  },
  {
    topic: "notices",
    say: () => `Power was out on ${faker.location.street()} for an hour. Anyone else?`,
  },
  {
    topic: "for-sale",
    say: () => `${faker.animal.type()} hutch going cheap, barely used. ${street()}.`,
  },
  {
    topic: "recommendations",
    say: () => `Anyone used a decent plumber round ${faker.location.street()}?`,
  },
]

const said = () => {
  const {topic, say} = faker.helpers.arrayElement(notes)

  return note.writer().setContent(say()).addTags(["t", topic]).renderTemplate()
}

const asked = () => {
  const writer = poll
    .writer()
    .setTitle(`${faker.date.weekday()} or the weekend for the ${faker.company.buzzNoun()}?`)
    .addTags(["t", "meetups"])

  for (const label of [faker.date.weekday(), "Saturday", "Either suits me"]) {
    writer.addOption(label)
  }

  return writer.renderTemplate()
}

/** A gathering, either on a day or at a time, which is the difference between the two kinds. */
const gathering = () => {
  const starts = faker.date.soon({days: 9})
  const title = `${faker.company.buzzNoun()} at ${street()}`

  if (faker.datatype.boolean()) {
    return dateEvent
      .writer()
      .setIdentifier()
      .setTitle(title)
      .setLocation(street())
      .setStart(starts.toISOString().slice(0, 10))
      .setContent(faker.lorem.sentence())
      .addTags(["t", "meetups"])
      .renderTemplate()
  }

  return timeEvent
    .writer()
    .setIdentifier()
    .setTitle(title)
    .setLocation(street())
    .setStart(Math.floor(starts.getTime() / 1000))
    .setContent(faker.lorem.sentence())
    .addTags(["t", "meetups"])
    .renderTemplate()
}

const written = () =>
  article
    .writer()
    .setIdentifier()
    .setTitle(faker.book.title())
    .setSummary(faker.lorem.sentence())
    .setContent(faker.lorem.paragraphs(2))
    .renderTemplate()

/** One thing somebody hands over, weighted the way a street is: mostly talk. */
export const post = (): Promise<EventTemplate> =>
  faker.helpers.weightedArrayElement([
    {weight: 8, value: said},
    {weight: 2, value: asked},
    {weight: 2, value: gathering},
    {weight: 1, value: written},
  ])()

/** What somebody says about another person's thing, addressed to their own neighbours. */
export const remark = () =>
  faker.helpers.arrayElement([
    "Saw this too, still there as of an hour ago.",
    "I can bring a van if that helps.",
    faker.lorem.sentence(),
    "Ours went the same way last winter.",
  ])

export const emoji = () => faker.helpers.arrayElement(["👍", "❤️", "😂", "🙏", "👀"])

/**
 * The code both phones show while pairing, and the six digits a login-with-device
 * shows. `docs/keys.md`, `docs/discovery.md#the-consent-gate`.
 */
/** The six digits a login-with-device prompt compares. */
export const shortCode = () => faker.number.int({min: 0, max: 999_999})

/**
 * What the pairing gate compares: five shapes of eight in three tints, which is
 * `session::sas::PAIRING_SPACE` in the core.
 */
export const pairingCode = () => faker.number.int({min: 0, max: 24 ** 5 - 1})
