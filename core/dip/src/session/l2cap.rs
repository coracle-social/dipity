//! The L2CAP bandwidth upgrade's half of the session: who asks, who publishes,
//! and how the PSM crosses. `docs/transport.md#the-l2cap-bandwidth-upgrade`.
//!
//! The PSM is assigned at publish time and has to travel over the GATT channel
//! already open. Which end publishes is decided by the platform APIs rather
//! than by who wants the bandwidth. The GATT peripheral publishes and the
//! central connects, which makes the receiver the end that publishes and the
//! dialer the end that opens, whichever of the two raised the need.
//!
//! [`Upgrade`] is the whole exchange. It holds no wire and no session. Each
//! entry point answers what it owes and nothing else: a control payload for
//! the caller to send the peer, a [`Step`] for the shell from
//! [`Upgrade::poll`], or nothing. What the session branches on it asks
//! separately, with [`Upgrade::is_open`] and [`Upgrade::is_live`]. What the
//! session adds is the sending, and moving the outbox onto the channel once
//! one is open.
//!
//! Everything here is best-effort. Every failure path lands in [`State::Off`]
//! rather than closing anything, because a link whose upgrade fails keeps
//! working at GATT speed, and a step this end cannot read is skipped or ends
//! the upgrade. Once off it stays off for the session, and owes the shell one
//! [`Step::Close`] for a channel it holds or is still making.
//! `docs/transport.md#giving-up-the-upgrade`.

use crate::link::Role;

/// Discriminants for the upgrade exchange, one byte ahead of its payload,
/// inside the control channel's own `L2CAP` frame.
pub mod message {
    /// The dialer wants bulk and cannot publish. Carries nothing.
    pub const REQUEST: u8 = 0x01;
    /// The receiver published a channel at this PSM, big-endian, two bytes,
    /// and anything a later version adds after them.
    pub const PUBLISHED: u8 = 0x02;
    /// This end cannot publish or could not open. Carries nothing.
    pub const UNAVAILABLE: u8 = 0x03;
}

/// Where the upgrade has got to on one link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Nothing has wanted bulk on this link yet.
    Idle,
    /// Dialer: the peer has been asked to publish.
    Requested,
    /// Receiver: a channel is wanted here.
    Publishing {
        /// Whether the shell has been asked to publish yet.
        asked: bool,
    },
    /// Receiver: published, waiting for the peer to connect.
    Published(u16),
    /// Dialer: a PSM arrived and the channel behind it is wanted.
    Opening {
        /// Where the peer published.
        psm: u16,
        /// Whether the shell has been asked to open it yet.
        asked: bool,
    },
    /// The channel is up and bulk rides it.
    Open,
    /// Tried, and this link stays on GATT.
    Off {
        /// Whether the shell holds a channel for this link, or is making one,
        /// that it has not been told to close.
        unclosed: bool,
    },
}

/// What the shell has to do next to move the upgrade along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Publish an L2CAP channel and answer with the PSM it was assigned.
    Publish,
    /// Open the channel the peer published, at this PSM.
    Open(u16),
    /// Close the channel, or withdraw it if it is only published, because
    /// the upgrade went off. The link stays up.
    Close,
}

/// The upgrade exchange on one link, from this end's side of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Upgrade {
    role: Role,
    state: State,
}

impl Upgrade {
    /// A link that has not wanted bulk yet.
    #[must_use]
    pub fn new(role: Role) -> Self {
        Self {
            role,
            state: State::Idle,
        }
    }

    /// Bulk is wanted on this link, which starts the exchange from whichever
    /// end this is. Answers what the peer has to be told, if anything.
    ///
    /// A dialer that wants bulk asks the peer for a PSM rather than making one,
    /// because the GATT peripheral publishes and the central connects. Wanting
    /// it twice changes nothing: only an idle link can start.
    pub fn wanted(&mut self) -> Option<Vec<u8>> {
        match (self.role, self.state) {
            (Role::Receiver, State::Idle) => {
                self.state = State::Publishing { asked: false };

                None
            }
            (Role::Dialer, State::Idle) => {
                self.state = State::Requested;

                Some(vec![message::REQUEST])
            }
            _ => None,
        }
    }

    /// What the shell has to do next, once. Asking again answers `None` until
    /// the shell reports back and the state moves on.
    pub fn poll(&mut self) -> Option<Step> {
        match &mut self.state {
            State::Publishing { asked } if !*asked => {
                *asked = true;

                Some(Step::Publish)
            }
            State::Opening { psm, asked } if !*asked => {
                *asked = true;

                Some(Step::Open(*psm))
            }
            State::Off { unclosed } if *unclosed => {
                *unclosed = false;

                Some(Step::Close)
            }
            _ => None,
        }
    }

    /// The shell published a channel, whose PSM goes to the peer, unless the
    /// upgrade went off while the shell was publishing it, in which case the
    /// shell is told to withdraw it.
    pub fn published(&mut self, psm: u16) -> Option<Vec<u8>> {
        if self.is_off() {
            self.state = State::Off { unclosed: true };

            return None;
        }

        self.state = State::Published(psm);

        Some([&[message::PUBLISHED], &psm.to_be_bytes()[..]].concat())
    }

    /// The channel is up, and bulk rides it from here unless the upgrade went
    /// off while the shell was opening it, in which case the shell is told to
    /// close it. [`is_open`](Self::is_open) says which.
    pub fn opened(&mut self) {
        self.state = if self.is_off() {
            State::Off { unclosed: true }
        } else {
            State::Open
        };
    }

    /// Whether bulk rides an open channel on this link.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.state == State::Open
    }

    /// Whether the upgrade has been asked for and not given up: anywhere
    /// between a request and a channel, where either end may have moved
    /// blob traffic off GATT.
    #[must_use]
    pub fn is_live(&self) -> bool {
        !matches!(self.state, State::Idle | State::Off { .. })
    }

    /// Whether the upgrade has been given up for the rest of the session.
    fn is_off(&self) -> bool {
        matches!(self.state, State::Off { .. })
    }

    /// Whether the shell holds or is making a channel for this link that it
    /// has not been told to close.
    fn shell_holds_channel(&self) -> bool {
        matches!(
            self.state,
            State::Publishing { asked: true }
                | State::Published(_)
                | State::Opening { asked: true, .. }
                | State::Open
                | State::Off { unclosed: true }
        )
    }

    /// Give the upgrade up for the rest of the session, owing the shell a
    /// [`Step::Close`] if it holds a channel or is making one.
    fn go_off(&mut self) {
        self.state = State::Off {
            unclosed: self.shell_holds_channel(),
        };
    }

    /// The upgrade will not happen, or the channel that had it went away.
    ///
    /// The peer is told whenever it may be counting on the channel: waiting on a
    /// PSM, opening one, or writing to one. A channel can die at one end while
    /// the other's streams go quiet without ever reporting it.
    pub fn unavailable(&mut self) -> Option<Vec<u8>> {
        let owed = matches!(
            self.state,
            State::Publishing { .. } | State::Published(_) | State::Opening { .. } | State::Open
        );

        self.go_off();

        owed.then(|| vec![message::UNAVAILABLE])
    }

    /// One step of the exchange, off the control channel. Answers what goes
    /// back, if anything.
    pub fn receive(&mut self, payload: &[u8]) -> Option<Vec<u8>> {
        let (&step, body) = payload.split_first()?;

        match step {
            message::REQUEST => self.on_request(),
            message::PUBLISHED => self.on_published(body),
            message::UNAVAILABLE => {
                self.go_off();

                None
            }
            other => {
                log::debug!("ignoring L2CAP step {other:#04x}");

                None
            }
        }
    }

    /// The peer wants bulk and cannot publish it itself.
    ///
    /// A dialer cannot publish either. It says as much and stays on GATT,
    /// because neither end gives up a link over an optimization. A receiver
    /// whose upgrade went off says the same, and one whose upgrade is under
    /// way has nothing to add.
    fn on_request(&mut self) -> Option<Vec<u8>> {
        match (self.role, self.state) {
            (Role::Receiver, State::Idle) => {
                self.state = State::Publishing { asked: false };

                None
            }
            (Role::Receiver, State::Off { .. }) => Some(vec![message::UNAVAILABLE]),
            (Role::Receiver, _) => None,
            (Role::Dialer, _) => {
                self.go_off();

                Some(vec![message::UNAVAILABLE])
            }
        }
    }

    /// The peer published a PSM, and the channel behind it is wanted here.
    ///
    /// This arrives with no request behind it as often as not, because a
    /// receiver that wants bulk publishes unprompted. One too short to read
    /// ends the upgrade, and once it is off any PSM is answered `unavailable`.
    fn on_published(&mut self, body: &[u8]) -> Option<Vec<u8>> {
        let Some(psm) = psm(body) else {
            self.go_off();

            return Some(vec![message::UNAVAILABLE]);
        };

        match (self.role, self.state) {
            (_, State::Off { .. }) => Some(vec![message::UNAVAILABLE]),
            (Role::Dialer, State::Idle | State::Requested) => {
                self.state = State::Opening { psm, asked: false };

                None
            }
            _ => None,
        }
    }
}

/// Read the PSM off a `PUBLISHED` payload's first two bytes.
fn psm(payload: &[u8]) -> Option<u16> {
    match payload {
        &[high, low, ..] => Some(u16::from_be_bytes([high, low])),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn published(psm: u16) -> Vec<u8> {
        Upgrade::new(Role::Receiver).published(psm).unwrap()
    }

    #[test]
    fn a_psm_is_the_first_two_bytes_and_whatever_follows_is_ignored() {
        assert_eq!(psm(&0x1234u16.to_be_bytes()), Some(0x1234));
        assert_eq!(psm(&[0x12, 0x34, 0x56]), Some(0x1234));
        assert_eq!(psm(&[0x12]), None);
        assert_eq!(psm(&[]), None);
    }

    #[test]
    fn a_dialer_asks_for_a_psm_and_a_receiver_publishes_one() {
        let mut dialer = Upgrade::new(Role::Dialer);
        assert_eq!(dialer.wanted(), Some(vec![message::REQUEST]));
        assert_eq!(dialer.poll(), None);

        let mut receiver = Upgrade::new(Role::Receiver);
        assert_eq!(receiver.receive(&[message::REQUEST]), None);
        assert_eq!(receiver.poll(), Some(Step::Publish));

        dialer.receive(&receiver.published(0x0080).unwrap());
        assert_eq!(dialer.poll(), Some(Step::Open(0x0080)));
    }

    #[test]
    fn a_receiver_that_wants_bulk_publishes_unprompted() {
        let mut receiver = Upgrade::new(Role::Receiver);
        assert_eq!(receiver.wanted(), None);
        assert_eq!(receiver.poll(), Some(Step::Publish));

        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.receive(&published(0x0081));

        assert_eq!(dialer.poll(), Some(Step::Open(0x0081)));
    }

    #[test]
    fn the_shell_is_asked_for_each_channel_once() {
        let mut receiver = Upgrade::new(Role::Receiver);
        receiver.wanted();
        assert_eq!(receiver.poll(), Some(Step::Publish));
        assert_eq!(receiver.poll(), None);

        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.receive(&published(0x0082));
        assert_eq!(dialer.poll(), Some(Step::Open(0x0082)));
        assert_eq!(dialer.poll(), None);
    }

    #[test]
    fn a_dialer_asked_to_publish_says_it_cannot() {
        let mut dialer = Upgrade::new(Role::Dialer);

        assert_eq!(
            dialer.receive(&[message::REQUEST]),
            Some(vec![message::UNAVAILABLE])
        );
        assert_eq!(dialer.state, State::Off { unclosed: false });
        assert_eq!(dialer.poll(), None);
    }

    #[test]
    fn an_upgrade_that_went_off_never_starts_again() {
        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.receive(&[message::UNAVAILABLE]);

        assert_eq!(dialer.wanted(), None);
        assert_eq!(dialer.poll(), None);
    }

    #[test]
    fn only_an_end_the_peer_is_waiting_on_owes_it_an_answer() {
        let mut receiver = Upgrade::new(Role::Receiver);
        receiver.wanted();
        assert_eq!(
            receiver.unavailable(),
            Some(vec![message::UNAVAILABLE]),
            "a peer that asked for a PSM is waiting on one"
        );

        let mut published = Upgrade::new(Role::Receiver);
        published.published(0x0083);
        assert_eq!(published.unavailable(), Some(vec![message::UNAVAILABLE]));

        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.wanted();
        assert_eq!(
            dialer.unavailable(),
            None,
            "the end that asked owes the peer nothing by giving up"
        );
    }

    #[test]
    fn a_psm_too_short_to_read_ends_the_upgrade_and_is_answered_unavailable() {
        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.wanted();

        assert_eq!(
            dialer.receive(&[message::PUBLISHED, 0x80]),
            Some(vec![message::UNAVAILABLE])
        );
        assert_eq!(dialer.state, State::Off { unclosed: false });
        assert_eq!(dialer.poll(), None);
    }

    #[test]
    fn an_empty_or_unknown_step_is_ignored() {
        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.wanted();

        assert_eq!(dialer.receive(&[]), None);
        assert_eq!(dialer.receive(&[0xff, 1, 2, 3]), None);
        assert_eq!(dialer.state, State::Requested);
    }

    #[test]
    fn a_receiver_whose_upgrade_went_off_answers_a_request_with_unavailable() {
        let mut receiver = Upgrade::new(Role::Receiver);
        receiver.receive(&[message::UNAVAILABLE]);

        assert_eq!(
            receiver.receive(&[message::REQUEST]),
            Some(vec![message::UNAVAILABLE])
        );
        assert_eq!(receiver.state, State::Off { unclosed: false });
        assert_eq!(receiver.poll(), None);
    }

    #[test]
    fn a_dialer_whose_upgrade_went_off_answers_a_psm_with_unavailable() {
        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.receive(&[message::UNAVAILABLE]);

        assert_eq!(
            dialer.receive(&published(0x0087)),
            Some(vec![message::UNAVAILABLE])
        );
        assert_eq!(dialer.state, State::Off { unclosed: false });
        assert_eq!(dialer.poll(), None);
    }

    #[test]
    fn a_request_while_the_upgrade_is_under_way_is_ignored() {
        let mut receiver = Upgrade::new(Role::Receiver);
        receiver.wanted();
        receiver.poll();

        assert_eq!(receiver.receive(&[message::REQUEST]), None);
        assert_eq!(receiver.poll(), None);

        receiver.published(0x0088);
        assert_eq!(receiver.receive(&[message::REQUEST]), None);
        assert_eq!(receiver.state, State::Published(0x0088));
    }

    #[test]
    fn giving_up_a_channel_the_shell_was_asked_for_tells_it_to_close_once() {
        let mut receiver = Upgrade::new(Role::Receiver);
        receiver.wanted();
        receiver.poll();
        receiver.receive(&[message::UNAVAILABLE]);

        assert_eq!(receiver.poll(), Some(Step::Close));
        assert_eq!(receiver.poll(), None);

        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.receive(&published(0x008a));
        dialer.poll();
        dialer.opened();
        dialer.unavailable();

        assert_eq!(dialer.poll(), Some(Step::Close));
        assert_eq!(dialer.poll(), None);
    }

    #[test]
    fn giving_up_before_the_shell_was_asked_for_a_channel_tells_it_nothing() {
        let mut receiver = Upgrade::new(Role::Receiver);
        receiver.wanted();
        receiver.receive(&[message::UNAVAILABLE]);
        assert_eq!(receiver.poll(), None);

        let mut requested = Upgrade::new(Role::Dialer);
        requested.wanted();
        requested.unavailable();
        assert_eq!(
            requested.poll(),
            None,
            "asking the peer asks the shell nothing"
        );

        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.receive(&published(0x008b));
        dialer.receive(&[message::UNAVAILABLE]);
        assert_eq!(dialer.poll(), None);
    }

    #[test]
    fn a_channel_the_shell_reports_after_the_upgrade_went_off_is_closed_too() {
        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.receive(&published(0x008c));
        dialer.poll();
        dialer.receive(&[message::UNAVAILABLE]);
        assert_eq!(dialer.poll(), Some(Step::Close), "the open still under way");

        dialer.opened();
        assert_eq!(dialer.poll(), Some(Step::Close), "the channel it delivered");
        assert_eq!(dialer.poll(), None);

        let mut receiver = Upgrade::new(Role::Receiver);
        receiver.wanted();
        receiver.poll();
        receiver.receive(&[message::UNAVAILABLE]);
        assert_eq!(receiver.poll(), Some(Step::Close));

        receiver.published(0x008c);
        assert_eq!(receiver.poll(), Some(Step::Close));
        assert_eq!(receiver.poll(), None);
    }

    #[test]
    fn a_channel_given_up_at_both_ends_at_once_is_closed_once() {
        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.receive(&published(0x008d));
        dialer.poll();
        dialer.opened();

        // The shell loses the channel and the peer gives it up as well, before the shell is told anything.
        dialer.unavailable();
        dialer.receive(&[message::UNAVAILABLE]);

        assert_eq!(dialer.poll(), Some(Step::Close));
        assert_eq!(dialer.poll(), None);

        dialer.receive(&[message::UNAVAILABLE]);
        dialer.receive(&[message::REQUEST]);
        assert_eq!(dialer.poll(), None);
    }

    #[test]
    fn a_psm_arriving_after_the_channel_is_up_does_not_reopen_it() {
        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.wanted();
        dialer.receive(&published(0x0084));
        dialer.poll();
        dialer.opened();

        assert_eq!(dialer.receive(&published(0x0085)), None);

        assert_eq!(dialer.state, State::Open);
        assert_eq!(dialer.poll(), None);
    }
}
