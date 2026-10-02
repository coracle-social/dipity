// What a search matches, wherever the screen searching is.
//
// The core searches what people wrote, word by word, each word matching as the
// start of one (`core/dip/src/db/event/query.rs`). People's names are not in
// the store's text index, since nobody publishes a profile and a name is a card
// somebody wrote about somebody else, so they are matched here the same way.

import type {Social} from "$lib/data/contacts"

/** The words of a search, lower-cased, none empty. */
export const wordsOf = (query: string) => query.toLowerCase().split(/\s+/).filter(Boolean)

/** Whether every word of the search starts some word of `text`, which is how the core matches. */
export const matches = (words: string[], text: string) => {
  const have = text
    .toLowerCase()
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean)

  return words.every(word => have.some(found => found.startsWith(word)))
}

/**
 * Whether somebody is called something the search matches: the user's name for
 * them, any name somebody else gave them, or the start of their key.
 */
export const nameMatches = (social: Social, pubkey: string, words: string[]) => {
  const contact = social.people.get(pubkey)
  const names = [contact?.petname, ...(contact?.aliases.map(({petname}) => petname) ?? [])]

  return (
    names.some(name => name !== undefined && matches(words, name)) ||
    (words.length === 1 && pubkey.startsWith(words[0]))
  )
}

/** Everybody the device knows whose name the search matches. */
export const peopleNamed = (social: Social, words: string[]) =>
  [...social.people.keys()].filter(pubkey => nameMatches(social, pubkey, words))
