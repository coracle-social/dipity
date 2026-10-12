// The street `just dev` stages at `?scene` for store screenshots, written by hand and the same on every load.
//
// The faker neighborhood reads as lorem ipsum in a store listing. This one has
// the same shape, with people and posts a reader would believe, and nobody
// walks past once it is set out.
//
// The photographs are from Unsplash, under its license, by André Spieker,
// Matthew Wiebe, veeterzy, Webvilla, Brian Jimenez, Carli Jean and Olenka
// Kotyk. They load inline and only here, so none of them reaches a build.

import {sha256} from "@noble/hashes/sha2.js"
import {bytesToHex} from "@noble/hashes/utils.js"
import {hash, type EventTemplate, type HashedEvent} from "@welshman/util"
import bakes from "$lib/dev/scene/bakes.jpg?inline"
import coffee from "$lib/dev/scene/coffee.jpg?inline"
import oranges from "$lib/dev/scene/oranges.jpg?inline"
import pug from "$lib/dev/scene/pug.jpg?inline"
import puppy from "$lib/dev/scene/puppy.jpg?inline"
import strawberries from "$lib/dev/scene/strawberries.jpg?inline"
import vegetables from "$lib/dev/scene/vegetables.jpg?inline"
import {
  article,
  boost,
  card,
  comment,
  genericBoost,
  note,
  picture,
  poll,
  pollResponse,
  reaction,
  timeEvent,
} from "$lib/dev/kinds"
import {bytes, describe} from "$lib/dev/media"
import type {Person} from "$lib/dev/neighborhood"

const HOUR = 3600

const DAY = 86_400

/** Whether this load is the staged street. */
export const staged = new URLSearchParams(location.search).has("scene")

/** A stable key for a name, so the same person is the same pubkey on every load. */
const keyOf = (name: string) => bytesToHex(sha256(new TextEncoder().encode(`dip scene ${name}`)))

const person = (petname: string, known: boolean): Person => ({
  pubkey: keyOf(petname),
  petname,
  talkative: 3,
  known,
})

const maya = person("Maya", true)
const rosa = person("Rosa", true)
const theo = person("Theo", true)
const priya = person("Priya", true)
const sam = person("Sam", true)
const june = person("June", true)

/** Strangers the user's contacts have named, who reach the board through them. */
const lena = person("Lena", false)
const felix = person("Felix", false)
const omar = person("Omar", false)

export const cast = [maya, rosa, theo, priya, sam, june, lena, felix, omar]

/** The user, staged as somebody who has paired and posted. */
export const identity = keyOf("you")

/** One event as the device came to hold it, and the photograph behind it if it is a picture. */
export type Staged = {event: HashedEvent; from: string; at: number; media?: string}

/** A photograph's bytes as base64, and what the `imeta` tag says about it. */
const photo = (inline: string, dim: string) => {
  const base64 = inline.slice(inline.indexOf(",") + 1)

  return {
    base64,
    imeta: {
      url: "",
      hash: describe(bytes(base64))[0].slice(2),
      mimeType: "image/jpeg",
      dim,
      extra: describe(bytes(base64)).slice(1),
    },
  }
}

/** The street, written as of `now`. */
export const stage = async (now: number): Promise<Staged[]> => {
  const staged: Staged[] = []
  const ago = (hours: number) => now - Math.round(hours * HOUR)

  const add = async (
    template: EventTemplate | Promise<EventTemplate>,
    author: Person | string,
    written: number,
    from: Person | string = author,
    seen = written,
    media?: string,
  ) => {
    const pubkey = typeof author === "string" ? author : author.pubkey
    const event = hash({...(await template), created_at: written, pubkey})

    staged.push({event, from: typeof from === "string" ? from : from.pubkey, at: seen, media})

    return event
  }

  const pictured = async (
    author: Person,
    written: number,
    from: Person,
    seen: number,
    inline: string,
    dim: string,
    caption: string,
    topic: string,
  ) => {
    const {base64, imeta} = photo(inline, dim)
    const template = picture.writer().setContent(caption).addImeta(imeta).addTags(["t", topic])

    return add(template.renderTemplate(), author, written, from, seen, base64)
  }

  // A response was seen long ago, so the board leads with posts rather than the talk under them.
  const respond = (template: Promise<EventTemplate>, by: Person, hours: number) =>
    add(template, by, ago(hours), by, ago(hours + 3 * 24))

  const react = (about: HashedEvent, by: Person, content: string, hours: number) =>
    respond(reaction.writer().setEvent(about).setContent(content).renderTemplate(), by, hours)

  const reply = (
    about: HashedEvent,
    by: Person,
    content: string,
    hours: number,
    topic?: string,
  ) => {
    const writer = comment.writer().replyTo(about).setContent(content)

    if (topic) writer.addTags(["t", topic])

    return respond(writer.renderTemplate(), by, hours)
  }

  for (const contact of cast.filter(contact => contact.known)) {
    staged.push({
      event: card(identity, contact.pubkey, contact.petname, now - 20 * DAY),
      from: identity,
      at: now - 20 * DAY,
    })
  }

  for (const [by, about] of [
    [maya, lena],
    [maya, rosa],
    [sam, felix],
    [sam, omar],
    [theo, omar],
    [theo, june],
    [priya, felix],
    [june, maya],
    [rosa, priya],
  ] as const) {
    staged.push({
      event: card(by.pubkey, about.pubkey, about.petname, now - 15 * DAY),
      from: by.pubkey,
      at: now - 15 * DAY,
    })
  }

  const plums = await add(
    note
      .writer()
      .setContent(
        "The plum tree by the bakery is dropping fruit faster than anyone can eat it. Bring a bag and help yourself 🍑",
      )
      .addTags(["t", "spare"])
      .renderTemplate(),
    maya,
    ago(1.2),
    maya,
    ago(0.4),
  )

  await react(plums, rosa, "❤️", 0.9)
  await react(plums, theo, "❤️", 0.8)
  await react(plums, priya, "🙌", 0.6)
  await react(plums, june, "🍑", 0.5)

  await reply(
    plums,
    theo,
    "Went by at lunch and there are still loads. The ones on the south side are the ripest.",
    0.7,
    "spare",
  )

  const jam = await reply(
    plums,
    june,
    "Making jam this weekend. Bring me a bag and you get a jar back",
    0.5,
    "spare",
  )

  await reply(jam, maya, "Deal. I'll drop some round tomorrow.", 0.3, "spare")

  const pepper = await pictured(
    rosa,
    ago(2.5),
    rosa,
    ago(1.1),
    puppy,
    "1200x1500",
    "Meet Pepper, the newest resident of Alder Street. She has already introduced herself to every dog on the block.",
    "pets",
  )

  await react(pepper, maya, "❤️", 2)
  await react(pepper, june, "❤️", 1.8)
  await react(pepper, sam, "😍", 1.5)
  await reply(pepper, sam, "She's adorable. Bring her to movie night!", 1.4, "pets")

  const saturday = new Date((now + DAY) * 1000)

  saturday.setDate(saturday.getDate() + ((6 - saturday.getDay() + 7) % 7))
  saturday.setHours(10, 0, 0, 0)

  const workday = await add(
    timeEvent
      .writer()
      .setIdentifier("garden-workday")
      .setTitle("Community garden workday")
      .setLocation("Elm Street garden")
      .setStart(Math.floor(saturday.getTime() / 1000))
      .setContent(
        "Bring gloves if you have them. We'll have tools, tea and a great deal of compost to turn.",
      )
      .addTags(["t", "meetups"])
      .renderTemplate(),
    priya,
    ago(5),
    priya,
    ago(2),
  )

  await react(workday, theo, "👍", 1.9)
  await react(workday, maya, "👍", 1.7)

  const vote = poll
    .writer()
    .setTitle("Movie night on the green: what should we watch?")
    .addTags(["t", "meetups"])

  for (const [id, label] of [
    ["bride", "The Princess Bride"],
    ["paddington", "Paddington 2"],
    ["spirited", "Spirited Away"],
  ]) {
    vote.addOption(label, id)
  }

  const movies = await add(vote.renderTemplate(), sam, ago(4), sam, ago(3))

  for (const [by, choice] of [
    [maya, "paddington"],
    [rosa, "paddington"],
    [theo, "bride"],
    [june, "spirited"],
    [priya, "paddington"],
  ] as const) {
    await add(
      pollResponse.writer().setPollId(movies.id).addSelection(choice).renderTemplate(),
      by,
      ago(2.5),
      by,
      ago(3 * 24),
    )
  }

  const ladder = await add(
    note
      .writer()
      .setContent(
        "Anyone have a long ladder I could borrow Saturday morning? The gutters are overflowing again. I'll return it with a loaf of bread.",
      )
      .addTags(["t", "help"])
      .renderTemplate(),
    theo,
    ago(6),
    theo,
    ago(3.5),
  )

  await reply(ladder, priya, "We have one in the shed. Come by any time after nine.", 3.2, "help")

  const harvest = await pictured(
    priya,
    ago(8),
    priya,
    ago(4.5),
    vegetables,
    "1200x900",
    "First proper harvest from the allotment. Far too much chard, so come and take some.",
    "spare",
  )

  await react(harvest, rosa, "🥬", 4)
  await react(harvest, theo, "❤️", 3.8)
  await reply(harvest, june, "Save me some? I'll swap you a dozen eggs.", 3.9, "spare")

  const keys = await add(
    note
      .writer()
      .setContent(
        "Found a set of keys on a blue lanyard by the bus stop on Alder Street. I've left them with the corner shop.",
      )
      .addTags(["t", "lost-and-found"])
      .renderTemplate(),
    lena,
    ago(9),
    maya,
    ago(5.5),
  )

  await add(boost.writer().setEvent(keys).setContent("").renderTemplate(), maya, ago(5.6))

  const farm = await pictured(
    felix,
    ago(11),
    sam,
    ago(6.5),
    strawberries,
    "1200x900",
    "Pick-your-own is open at the farm past the bridge until Sunday.",
    "notices",
  )

  await add(genericBoost.writer().setEvent(farm).setContent("").renderTemplate(), sam, ago(6.6))

  await add(
    article
      .writer()
      .setIdentifier("repair-cafe")
      .setTitle("Notes from our first repair café")
      .setContent(
        [
          "Thirty-one things came through the library doors on Sunday, and twenty-four went home working: two toasters, a lamp that had been dark since 2019, a stack of trousers short of a button, and one very loved teddy bear with a new ear.",
          "Thank you to everyone who brought a screwdriver, a sewing kit or just patience. We'll run the next one on the first Sunday of next month. If you can solder, Theo would like a word.",
        ].join("\n\n"),
      )
      .addTags(["t", "meetups"])
      .renderTemplate(),
    june,
    ago(10),
    june,
    ago(7),
  )

  await add(
    note
      .writer()
      .setContent(
        "Huge thanks to whoever recommended the cobbler on Mill Lane. My boots look brand new.",
      )
      .addTags(["t", "recommendations"])
      .renderTemplate(),
    rosa,
    ago(13),
    rosa,
    ago(8),
  )

  const roaster = await pictured(
    sam,
    ago(14),
    sam,
    ago(9),
    coffee,
    "1200x900",
    "The corner café has a new roaster and it's worth the detour.",
    "recommendations",
  )

  await react(roaster, june, "☕", 8.5)

  const shelf = await add(
    note
      .writer()
      .setContent("The book swap shelf is back up outside number 12. Take one, leave one.")
      .addTags(["t", "notices"])
      .renderTemplate(),
    identity,
    ago(20),
    identity,
    ago(20),
  )

  await react(shelf, maya, "📚", 18)
  await react(shelf, june, "❤️", 17)

  await pictured(
    maya,
    ago(2 * 24 + 3),
    maya,
    ago(2 * 24),
    pug,
    "1200x1500",
    "Biscuit has opinions about the cold.",
    "pets",
  )

  await pictured(
    theo,
    ago(3 * 24 + 5),
    theo,
    ago(3 * 24),
    oranges,
    "1200x1500",
    "Our orange tree finally decided to fruit. Knock if you want a bag.",
    "spare",
  )

  await pictured(
    june,
    ago(4 * 24 + 2),
    june,
    ago(4 * 24),
    bakes,
    "1200x1200",
    "Bake sale leftovers are at the library until four. Lemon biscuits, mostly.",
    "spare",
  )

  return staged
}

/** The posts the user bookmarked, by what they say. */
export const bookmarkedFrom = (staged: Staged[]) =>
  staged
    .map(({event}) => event)
    .filter(event =>
      ["Community garden workday", "Pick-your-own", "Notes from our first repair café"].some(
        words => event.content.includes(words) || event.tags.some(tag => tag[1]?.includes(words)),
      ),
    )
