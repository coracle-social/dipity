//! When to tell the user something arrived, which is the core's to decide
//! because the view is suspended whenever it would matter.
//!
//! A notification goes out only while the app is in the background, and only
//! for what the user switched on: somebody nearby wanting to pair, or new
//! writing on the board. Writing is counted rather than announced one event at
//! a time, so a first sync that brings in a hundred things is one
//! notification, updated at most once a minute. `docs/storage.md#notifications`.

use std::collections::BTreeSet;

use crate::link::LinkId;
use crate::model::NotificationPrefs;
use coracle_lib::events::HashedEvent;
use coracle_lib::keys::PublicKey;

/// The kinds that count as new writing: notes, comments, polls, calendar
/// entries and articles. Reactions, boosts, deletions, lists and contact cards
/// do not.
pub const CONTENT_KINDS: [u16; 6] = [1, 1_111, 1_068, 31_922, 31_923, 30_023];

/// The shortest gap between two updates of the new-writing notification.
pub const CONTENT_INTERVAL_SECONDS: i64 = 60;

/// Something worth interrupting the user for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notification {
    /// Somebody the user has not named is in range and can be paired with.
    Pairing,
    /// This much new writing has arrived since the user last opened the app.
    Content(u32),
}

/// Who a pairing notification was about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Asker {
    Link(LinkId),
    Person(PublicKey),
}

/// What has been counted and announced since the user last looked.
#[derive(Debug, Default)]
pub struct Notifier {
    /// What the user switched on.
    pub prefs: NotificationPrefs,
    /// New writing since the app was last in the foreground.
    unseen: u32,
    /// How much of it the last notification announced.
    announced: u32,
    /// When the new-writing notification last went out.
    announced_at: Option<i64>,
    /// People, or held links naming nobody yet, already announced as wanting to pair.
    asked: BTreeSet<Asker>,
}

impl Notifier {
    /// The user opened the app, so everything so far has been seen.
    pub fn seen(&mut self) {
        self.unseen = 0;
        self.announced = 0;
        self.announced_at = None;
        self.asked.clear();
    }

    /// Count a newly stored event, if it is somebody else's writing the user
    /// has not muted.
    pub fn stored(
        &mut self,
        event: &HashedEvent,
        identity: &PublicKey,
        muted: &BTreeSet<PublicKey>,
    ) {
        if event.pubkey != *identity
            && CONTENT_KINDS.contains(&event.kind)
            && !muted.contains(&event.pubkey)
        {
            self.unseen = self.unseen.saturating_add(1);
        }
    }

    /// Somebody the user has not named is asking over `link`, announced once a
    /// person, or once a link while the gate holds them unnamed.
    pub fn pairing(
        &mut self,
        link: LinkId,
        pubkey: Option<PublicKey>,
        background: bool,
    ) -> Option<Notification> {
        let asker = pubkey.map_or(Asker::Link(link), Asker::Person);

        (background && self.prefs.pairing && self.asked.insert(asker))
            .then_some(Notification::Pairing)
    }

    /// The new-writing notification, if there is more to announce and the
    /// last one is at least an interval old.
    pub fn content(&mut self, now: i64, background: bool) -> Option<Notification> {
        let due = self
            .announced_at
            .is_none_or(|at| now - at >= CONTENT_INTERVAL_SECONDS);

        if !(background && self.prefs.content && due && self.unseen > self.announced) {
            return None;
        }

        self.announced = self.unseen;
        self.announced_at = Some(now);

        Some(Notification::Content(self.unseen))
    }
}

#[cfg(test)]
mod tests {
    use coracle_lib::tags::Tags;

    use super::*;
    use crate::fixtures::{author, event, note};

    fn on() -> Notifier {
        Notifier {
            prefs: NotificationPrefs {
                pairing: true,
                content: true,
            },
            ..Default::default()
        }
    }

    #[test]
    fn writing_is_counted_but_reactions_own_events_and_the_muted_are_not() {
        let mut notifier = on();
        let muted = BTreeSet::from([author(5)]);

        notifier.stored(
            &note(author(2), 1, "hello", Tags::new()),
            &author(1),
            &muted,
        );
        notifier.stored(&note(author(1), 1, "mine", Tags::new()), &author(1), &muted);
        notifier.stored(
            &note(author(5), 1, "muted", Tags::new()),
            &author(1),
            &muted,
        );
        notifier.stored(
            &event(author(2), 7, 1, "+", Tags::new()),
            &author(1),
            &muted,
        );

        assert_eq!(notifier.content(100, true), Some(Notification::Content(1)));
    }

    #[test]
    fn the_writing_notification_updates_at_most_once_an_interval() {
        let mut notifier = on();
        let stored = note(author(2), 1, "hello", Tags::new());

        notifier.stored(&stored, &author(1), &BTreeSet::new());
        assert_eq!(notifier.content(100, true), Some(Notification::Content(1)));

        notifier.stored(&stored, &author(1), &BTreeSet::new());
        assert_eq!(notifier.content(130, true), None);
        assert_eq!(
            notifier.content(100 + CONTENT_INTERVAL_SECONDS, true),
            Some(Notification::Content(2))
        );
    }

    #[test]
    fn nothing_is_announced_in_the_foreground_or_when_switched_off() {
        let mut notifier = on();
        notifier.stored(
            &note(author(2), 1, "hello", Tags::new()),
            &author(1),
            &BTreeSet::new(),
        );

        assert_eq!(notifier.content(100, false), None);
        assert_eq!(notifier.pairing(LinkId(1), None, false), None);

        notifier.prefs = NotificationPrefs::default();
        assert_eq!(notifier.content(100, true), None);
        assert_eq!(notifier.pairing(LinkId(1), None, true), None);
    }

    #[test]
    fn a_pairing_is_announced_once_a_person_until_the_user_looks() {
        let mut notifier = on();

        assert_eq!(
            notifier.pairing(LinkId(1), None, true),
            Some(Notification::Pairing)
        );
        assert_eq!(notifier.pairing(LinkId(1), None, true), None);
        assert_eq!(
            notifier.pairing(LinkId(2), Some(author(2)), true),
            Some(Notification::Pairing)
        );
        assert_eq!(notifier.pairing(LinkId(3), Some(author(2)), true), None);

        notifier.seen();
        assert_eq!(
            notifier.pairing(LinkId(4), Some(author(2)), true),
            Some(Notification::Pairing)
        );
    }
}
