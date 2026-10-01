// One person, as somebody calls them.
//
// A card is addressed to the person it names and carries the name as its
// content, so its only tag is the `d` its address is made of.
// `docs/policy.md#social-graph`.

import {EventQuery, EventReader, EventWriter, KindFactory} from "@welshman/domain"

/** A contact card. Ours rather than NIP-51's, and addressable at one person. */
export const CONTACT = 36_017

export class ContactCardReader extends EventReader {
  /** Who the card is about. */
  subject() {
    return this.identifier()
  }

  /** What the author calls them. */
  petname() {
    return this.content().trim()
  }
}

export class ContactCardWriter extends EventWriter<ContactCardReader> {
  /** Name somebody, by addressing the card to them. */
  name(pubkey: string, petname: string) {
    return this.setIdentifier(pubkey).setContent(petname)
  }
}

export class ContactCardQuery extends EventQuery {
  /** There are no relays here, so a query routes nowhere. */
  protected renderRoutes() {
    return []
  }
}

export const ContactCard = new KindFactory({
  kind: CONTACT,
  reader: ContactCardReader,
  writer: ContactCardWriter,
  query: ContactCardQuery,
})
