//! The kinds this app defines: the people lists the trust graph is built
//! from, and the card that says what somebody is called.
//!
//! Every kind the core reads and does not define comes from `coracle-kinds` —
//! [`delete`](coracle_kinds::delete) for kind 5, [`profile`](coracle_kinds::profile)
//! for kind 0 — so this module holds only what is ours.
//!
//! Trust, block and mute are the same shape: `p` tags naming pubkeys,
//! distinguished only by the kind that gives them meaning. That is
//! `coracle-kinds`'s `relay_collection` arrangement — one reader and one
//! writer parameterized by kind, with an alias and a named constant per kind —
//! and it is followed here rather than reinvented.
//!
//! **None of the three is encrypted.** NIP-51 keeps private entries as
//! ciphertext in `content`, which would put a list beyond reach of the peers
//! who need it — these travel to trusted peers by design, so what governs who
//! sees them is [`Visibility`](crate::model::Visibility) rather than a key.
//! That is why no `Sealed` payload appears below, where the library's own list
//! kinds carry one. `docs/policy.md#social-graph`.

use coracle_lib::events::{HasContent, HasEvent, HasKind, HasTags};
use coracle_lib::keys::PublicKey;
use coracle_lib::readers::{Reader, ReaderError};
use coracle_lib::tags::{HasTagsMut, Tags};
use coracle_lib::writers::{ValidationError, Writer};

/// The people the user trusts.
///
/// Ours rather than NIP-51's, because this list is not a curation of people to
/// read — it decides who may be handed the author's signature, which is
/// permanent transferable attribution. A generic list editor in another client
/// must not be able to grant that without knowing it has.
///
/// Replaceable, so the address is `16017:<pubkey>:` and one lookup answers for
/// the whole list. See `docs/policy.md#social-graph`.
pub const TRUST: u16 = 16_017;

/// The people the user has blocked. The wire control, and separate from
/// [`MUTE`], which only filters what the user is shown.
pub const BLOCK: u16 = 16_018;

/// NIP-51's mute list. A display filter, and never a gate on propagation.
pub const MUTE: u16 = 10_000;

/// NIP-51's bookmark list, which the retention sweep reads.
///
/// The one list here the core does not otherwise care about: the view writes
/// it, and [`forget_seen_before`](crate::db::event::command::forget_seen_before)
/// spares whatever it names, because a bookmark is the only way a person says
/// to keep something somebody else wrote. `docs/storage.md#retention`.
pub const BOOKMARKS: u16 = 10_003;

/// One person, as somebody else calls them.
///
/// Ours, and addressable at the person it names. The core reads nothing out of
/// a card, but the retention sweep spares it the way it spares a replaceable
/// list. `docs/policy.md#social-graph`.
pub const CONTACT: u16 = 36_017;

/// The `p` pubkeys in a tag set, deduplicated and in tag order.
///
/// A value that is not a pubkey names nobody, so it contributes nothing and
/// the rest of the list reads normally.
fn pubkeys(tags: &Tags) -> Vec<PublicKey> {
    let mut pubkeys: Vec<PublicKey> = Vec::new();

    for value in tags.values("p") {
        if let Ok(pubkey) = PublicKey::from_hex(value)
            && !pubkeys.contains(&pubkey)
        {
            pubkeys.push(pubkey);
        }
    }

    pubkeys
}

/// A list of people, and the event it was read from.
pub struct PeopleListReader<'e, E, const KIND: u16> {
    event: &'e E,
    pubkeys: Vec<PublicKey>,
}

/// A trust list being read.
pub type TrustListReader<'e, E> = PeopleListReader<'e, E, TRUST>;
/// A block list being read.
pub type BlockListReader<'e, E> = PeopleListReader<'e, E, BLOCK>;
/// A mute list being read.
pub type MuteListReader<'e, E> = PeopleListReader<'e, E, MUTE>;

impl<'e, E, const KIND: u16> HasEvent for PeopleListReader<'e, E, KIND> {
    type Event = E;

    fn event(&self) -> &E {
        self.event
    }
}

impl<'e, E: HasKind + HasTags, const KIND: u16> Reader<'e> for PeopleListReader<'e, E, KIND> {
    const KIND: u16 = KIND;

    fn read_impl(event: &'e E) -> Result<Self, ReaderError> {
        Ok(PeopleListReader {
            pubkeys: pubkeys(event.tags()),
            event,
        })
    }
}

impl<'e, E, const KIND: u16> PeopleListReader<'e, E, KIND> {
    /// The pubkeys the list names.
    pub fn pubkeys(&self) -> &[PublicKey] {
        &self.pubkeys
    }

    /// Whether the list names a pubkey.
    pub fn contains(&self, pubkey: &PublicKey) -> bool {
        self.pubkeys.contains(pubkey)
    }
}

/// A list of people being assembled.
#[derive(Debug, Clone, Default)]
pub struct PeopleListWriter<const KIND: u16> {
    tags: Tags,
}

/// A trust list being assembled.
pub type TrustListWriter = PeopleListWriter<TRUST>;
/// A block list being assembled.
pub type BlockListWriter = PeopleListWriter<BLOCK>;
/// A mute list being assembled.
pub type MuteListWriter = PeopleListWriter<MUTE>;

impl<const KIND: u16> HasTags for PeopleListWriter<KIND> {
    fn tags(&self) -> &Tags {
        &self.tags
    }
}

impl<const KIND: u16> HasTagsMut for PeopleListWriter<KIND> {
    fn tags_mut(&mut self) -> &mut Tags {
        &mut self.tags
    }
}

impl<const KIND: u16> Writer for PeopleListWriter<KIND> {
    const KIND: u16 = KIND;

    /// A writer carrying every tag the stored list held, so editing one from
    /// here does not destroy what another client wrote.
    fn read_impl<E>(event: &E) -> Result<Self, ReaderError>
    where
        E: HasKind + HasContent + HasTags,
    {
        Ok(PeopleListWriter {
            tags: event.tags().clone(),
        })
    }

    /// Empty. The list is public, so there is nothing sealed to carry.
    fn render_content(&self) -> Result<String, ValidationError> {
        Ok(String::new())
    }
}

impl<const KIND: u16> PeopleListWriter<KIND> {
    /// A list naming nobody yet.
    pub fn new() -> Self {
        PeopleListWriter::default()
    }

    /// The pubkeys the list names.
    pub fn pubkeys(&self) -> Vec<PublicKey> {
        pubkeys(&self.tags)
    }

    /// Name someone. Adding a pubkey already there changes nothing.
    pub fn add_pubkey(mut self, pubkey: PublicKey) -> Self {
        if !self.pubkeys().contains(&pubkey) {
            self.tags = self.tags.add("p", [pubkey.to_hex()]);
        }

        self
    }

    /// Stop naming someone.
    pub fn remove_pubkey(mut self, pubkey: &PublicKey) -> Self {
        let hex = pubkey.to_hex();

        self.tags
            .0
            .retain(|tag| tag.name() != "p" || tag.value() != hex);

        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::{author, event};

    /// A stored trust list for the reader to borrow.
    fn stored(tags: Tags) -> coracle_lib::events::HashedEvent {
        event(author(1), TRUST, 100, "", tags)
    }

    #[test]
    fn a_list_reads_its_p_tags() {
        let event = stored(
            Tags::new()
                .add("p", [author(2).to_hex()])
                .add("p", [author(3).to_hex()]),
        );
        let list = TrustListReader::read(&event).unwrap();

        assert_eq!(list.pubkeys(), [author(2), author(3)]);
        assert!(list.contains(&author(2)));
        assert!(!list.contains(&author(9)));
    }

    #[test]
    fn an_entry_that_is_not_a_pubkey_costs_nobody_their_place() {
        let event = stored(
            Tags::new()
                .add("p", ["nonsense"])
                .add("p", [author(2).to_hex()]),
        );

        assert_eq!(
            TrustListReader::read(&event).unwrap().pubkeys(),
            [author(2)]
        );
    }

    #[test]
    fn a_repeated_pubkey_is_named_once() {
        let event = stored(
            Tags::new()
                .add("p", [author(2).to_hex()])
                .add("p", [author(2).to_hex()]),
        );

        assert_eq!(
            TrustListReader::read(&event).unwrap().pubkeys(),
            [author(2)]
        );
    }

    #[test]
    fn a_reader_refuses_another_kind() {
        let blocks = event(author(1), BLOCK, 100, "", Tags::new());

        assert!(TrustListReader::read(&blocks).is_err());
        assert!(BlockListReader::read(&blocks).is_ok());
    }

    #[test]
    fn a_writer_keeps_what_this_build_does_not_model() {
        // A tag the writer has no field for survives: it holds the tag set, not a parse of it.
        let stored = event(
            author(1),
            TRUST,
            100,
            "",
            Tags::new()
                .add("p", [author(2).to_hex()])
                .add("title", ["people I have met"]),
        );

        let template = TrustListWriter::read(&stored)
            .unwrap()
            .add_pubkey(author(3))
            .render_template()
            .unwrap();

        assert_eq!(template.tags.value("title"), Some("people I have met"));
        assert_eq!(template.kind, TRUST);
        // Public, so nothing rides in content.
        assert!(template.content.is_empty());

        let rebuilt = event(author(1), TRUST, 100, "", template.tags);

        assert_eq!(
            TrustListReader::read(&rebuilt).unwrap().pubkeys(),
            [author(2), author(3)]
        );
    }

    #[test]
    fn adding_and_removing_name_and_unname() {
        let list = TrustListWriter::new()
            .add_pubkey(author(2))
            .add_pubkey(author(3))
            // Already named, so this is not a second tag.
            .add_pubkey(author(2));

        assert_eq!(list.pubkeys(), [author(2), author(3)]);
        assert_eq!(
            list.clone().remove_pubkey(&author(2)).pubkeys(),
            [author(3)]
        );
        // Removing someone not named changes nothing.
        assert_eq!(
            list.remove_pubkey(&author(9)).pubkeys(),
            [author(2), author(3)]
        );
    }

    #[test]
    fn the_kinds_are_replaceable_so_one_address_holds_each_list() {
        // An edit supersedes, and `by_address` finds the current list in one lookup.
        for kind in [TRUST, BLOCK, MUTE] {
            assert!(coracle_lib::kinds::is_replaceable(kind), "{kind} is not");
        }

        use coracle_lib::addresses::EventExtensionAddress;

        assert_eq!(
            event(author(1), TRUST, 100, "", Tags::new())
                .address()
                .unwrap()
                .to_string(),
            format!("{TRUST}:{}:", author(1).to_hex())
        );
    }
}
