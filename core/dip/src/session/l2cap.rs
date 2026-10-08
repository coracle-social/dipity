//! The L2CAP bandwidth upgrade's half of the session: who asks, who publishes,
//! and how the PSM crosses. `docs/transport.md#the-l2cap-bandwidth-upgrade`.
//!
//! The PSM is assigned at publish time and has to travel over the GATT channel
//! already open. Which end publishes is decided by the platform APIs rather
//! than by who wants the bandwidth. The GATT peripheral publishes and the
//! central connects, which makes the receiver the end that publishes and the
//! dialer the end that opens, whichever of the two raised the need.
//!
//! [`Upgrade`] is the whole exchange. It holds no wire and no session: every
//! answer it owes the peer comes back as a control payload for the caller to
//! send, and everything it wants of the shell comes back as a [`Step`]. What
//! the session adds is the sending, and moving the outbox onto the channel once
//! one is open.
//!
//! Everything here is best-effort. Every failure path lands in [`State::Off`]
//! rather than closing anything, because a link whose upgrade fails keeps
//! working at GATT speed.

use anyhow::{Result, bail};

use crate::link::Role;

/// Discriminants for the upgrade exchange, one byte ahead of its payload,
/// inside the control channel's own `L2CAP` frame.
pub mod message {
    /// The dialer wants bulk and cannot publish. Carries nothing.
    pub const REQUEST: u8 = 0x01;
    /// The receiver published a channel at this PSM, big-endian, two bytes.
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
    Off,
}

/// What the shell has to do next to move the upgrade along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Publish an L2CAP channel and answer with the PSM it was assigned.
    Publish,
    /// Open the channel the peer published, at this PSM.
    Open(u16),
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
            _ => None,
        }
    }

    /// The shell published a channel, whose PSM goes to the peer.
    pub fn published(&mut self, psm: u16) -> Vec<u8> {
        self.state = State::Published(psm);

        [&[message::PUBLISHED], &psm.to_be_bytes()[..]].concat()
    }

    /// The channel is up and bulk rides it from here.
    pub fn opened(&mut self) {
        self.state = State::Open;
    }

    /// The upgrade will not happen, or the channel that had it went away.
    ///
    /// A peer waiting on a PSM is told, since a publisher that goes quiet
    /// leaves it waiting on a channel that is never coming.
    pub fn unavailable(&mut self) -> Option<Vec<u8>> {
        let owed = matches!(self.state, State::Publishing { .. } | State::Published(_));

        self.state = State::Off;

        owed.then(|| vec![message::UNAVAILABLE])
    }

    /// One step of the exchange, off the control channel. Answers what goes
    /// back, if anything.
    pub fn receive(&mut self, payload: &[u8]) -> Result<Option<Vec<u8>>> {
        let Some((&discriminant, body)) = payload.split_first() else {
            bail!("an empty L2CAP frame arrived");
        };

        match discriminant {
            message::REQUEST => Ok(self.on_request()),
            message::PUBLISHED => self.on_published(body).map(|()| None),
            message::UNAVAILABLE => {
                self.state = State::Off;

                Ok(None)
            }
            other => bail!("an unknown L2CAP frame {other} arrived"),
        }
    }

    /// The peer wants bulk and cannot publish it itself.
    ///
    /// A dialer cannot publish either. It says as much and stays on GATT,
    /// because neither end gives up a link over an optimization.
    fn on_request(&mut self) -> Option<Vec<u8>> {
        match (self.role, self.state) {
            (Role::Receiver, State::Idle) => {
                self.state = State::Publishing { asked: false };

                None
            }
            (Role::Receiver, _) => None,
            (Role::Dialer, _) => {
                self.state = State::Off;

                Some(vec![message::UNAVAILABLE])
            }
        }
    }

    /// The peer published a PSM, and the channel behind it is wanted here.
    ///
    /// This arrives with no request behind it as often as not, because a
    /// receiver that wants bulk publishes unprompted.
    fn on_published(&mut self, body: &[u8]) -> Result<()> {
        let Some(psm) = psm(body) else {
            bail!("an L2CAP PSM arrived as {} bytes, not 2", body.len());
        };

        if self.role == Role::Dialer && matches!(self.state, State::Idle | State::Requested) {
            self.state = State::Opening { psm, asked: false };
        }

        Ok(())
    }
}

/// Read the PSM off a `PUBLISHED` payload.
fn psm(payload: &[u8]) -> Option<u16> {
    match payload {
        &[high, low] => Some(u16::from_be_bytes([high, low])),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn published(psm: u16) -> Vec<u8> {
        Upgrade::new(Role::Receiver).published(psm)
    }

    #[test]
    fn a_psm_is_exactly_two_bytes() {
        assert_eq!(psm(&0x1234u16.to_be_bytes()), Some(0x1234));
        assert_eq!(psm(&[0x12]), None);
        assert_eq!(psm(&[0x12, 0x34, 0x56]), None);
    }

    #[test]
    fn a_dialer_asks_for_a_psm_and_a_receiver_publishes_one() {
        let mut dialer = Upgrade::new(Role::Dialer);
        assert_eq!(dialer.wanted(), Some(vec![message::REQUEST]));
        assert_eq!(dialer.poll(), None);

        let mut receiver = Upgrade::new(Role::Receiver);
        assert_eq!(receiver.receive(&[message::REQUEST]).unwrap(), None);
        assert_eq!(receiver.poll(), Some(Step::Publish));

        dialer.receive(&receiver.published(0x0080)).unwrap();
        assert_eq!(dialer.poll(), Some(Step::Open(0x0080)));
    }

    #[test]
    fn a_receiver_that_wants_bulk_publishes_unprompted() {
        let mut receiver = Upgrade::new(Role::Receiver);
        assert_eq!(receiver.wanted(), None);
        assert_eq!(receiver.poll(), Some(Step::Publish));

        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.receive(&published(0x0081)).unwrap();

        assert_eq!(dialer.poll(), Some(Step::Open(0x0081)));
    }

    #[test]
    fn the_shell_is_asked_for_each_channel_once() {
        let mut receiver = Upgrade::new(Role::Receiver);
        receiver.wanted();
        assert_eq!(receiver.poll(), Some(Step::Publish));
        assert_eq!(receiver.poll(), None);

        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.receive(&published(0x0082)).unwrap();
        assert_eq!(dialer.poll(), Some(Step::Open(0x0082)));
        assert_eq!(dialer.poll(), None);
    }

    #[test]
    fn a_dialer_asked_to_publish_says_it_cannot() {
        let mut dialer = Upgrade::new(Role::Dialer);

        assert_eq!(
            dialer.receive(&[message::REQUEST]).unwrap(),
            Some(vec![message::UNAVAILABLE])
        );
        assert_eq!(dialer.state, State::Off);
        assert_eq!(dialer.poll(), None);
    }

    #[test]
    fn an_upgrade_that_went_off_never_starts_again() {
        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.receive(&[message::UNAVAILABLE]).unwrap();

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
    fn a_malformed_psm_is_an_error_rather_than_a_guess() {
        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.wanted();

        assert!(dialer.receive(&[message::PUBLISHED, 0x80]).is_err());
        assert!(dialer.receive(&[]).is_err());
        assert!(dialer.receive(&[0xff]).is_err());
    }

    #[test]
    fn a_psm_arriving_after_the_channel_is_up_does_not_reopen_it() {
        let mut dialer = Upgrade::new(Role::Dialer);
        dialer.wanted();
        dialer.receive(&published(0x0084)).unwrap();
        dialer.poll();
        dialer.opened();

        dialer.receive(&published(0x0085)).unwrap();

        assert_eq!(dialer.state, State::Open);
        assert_eq!(dialer.poll(), None);
    }
}
