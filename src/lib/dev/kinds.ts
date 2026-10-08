// The kinds a simulated peer writes, bound the way the view binds its own.
//
// Not `$lib/kinds`: a stand-in for the core must not reach for the app that
// calls it, or driving the view through the simulator would only prove the view
// agrees with itself. What both sides do share is `@welshman/domain`, which owns
// every tag layout here but the contact card's. That is how the two agree on
// the wire without either importing the other.
//
// The resolver answers no relays for the same reason the view's does: a
// simulated peer hands events over in person and routes nothing.

import {
  Article,
  Comment,
  DateEvent,
  Delete,
  GenericRepost,
  Note,
  Poll,
  Reaction,
  Repost,
  TimeEvent,
  type KindContext,
} from "@welshman/domain"
import {Resolver, hash} from "@welshman/util"

const context: KindContext = {resolver: new Resolver(() => [])}

export const note = Note.configure(context)

export const poll = Poll.configure(context)

export const article = Article.configure(context)

export const dateEvent = DateEvent.configure(context)

export const timeEvent = TimeEvent.configure(context)

export const comment = Comment.configure(context)

export const removal = Delete.configure(context)

export const reaction = Reaction.configure(context)

export const boost = Repost.configure(context)

export const genericBoost = GenericRepost.configure(context)

/** A card naming one person, written by hand because `@welshman/domain` has no class for it. */
export const card = (author: string, about: string, petname: string, at: number) =>
  hash({
    kind: 36_017,
    created_at: at,
    pubkey: author,
    tags: [["d", about]],
    content: petname,
  })
