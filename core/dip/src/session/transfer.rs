//! Login with device: handing the nostr identity to a second device over an
//! established session. `docs/keys.md#login-with-device`.
//!
//! This is deliberate key exfiltration, so nothing about it is implicit. Both
//! users act — the source starts the flow, the target answers a prompt, and the
//! source answers one more — and both compare the six digits
//! [`sas`](crate::session::sas) derives from the Noise transcript before the key
//! moves.
//!
//! The comparison is what authenticates the flow, and it is the only thing that
//! does. NIP-42 cannot help: the target authenticates as the identity it made
//! at first run, which says nothing about whether it is the user's own phone.
//!
//! [`IdentityTransfer`] is the whole flow: the state, the wire form, and the
//! two answers a user gives. It touches no session and no store — every method
//! answers with the control payload to send, and the session it hangs off does
//! the sending and decides whether a user is there to compare anything.

use anyhow::{Result, anyhow, bail};
use coracle_lib::keys::SecretKey;
use zeroize::Zeroizing;

use crate::session::sas::{LOGIN_LABEL, LOGIN_SPACE, sas};

/// Discriminants for the transfer exchange, one byte ahead of its payload,
/// inside the control channel's own `TRANSFER` frame.
mod message {
    /// The source's user started the flow. Carries nothing: both ends derive
    /// the comparison value from the transcript they already share.
    pub const OFFER: u8 = 0x01;
    /// The target's user compared the numbers and said yes.
    pub const ACCEPT: u8 = 0x02;
    /// The 32 secret key bytes, sealed by the session like every other frame.
    pub const KEY: u8 = 0x03;
    /// Either end refused, or could not run the flow at all.
    pub const DECLINE: u8 = 0x04;
}

/// One session's identity transfer, on whichever side is running it.
///
/// Only one is live at a time: the flow is a single exchange and a second offer
/// over the same link is refused rather than queued.
#[derive(Debug, Default)]
pub struct IdentityTransfer {
    /// Where the flow has got to, `None` when none is running.
    step: Option<Step>,
    /// A key taken off the wire, waiting for the shell to store it.
    received: Option<SecretKey>,
    /// How the flow ended, until whoever was waiting has been told.
    outcome: Option<Outcome>,
}

/// Where an identity transfer has got to.
///
/// Both users answer the same question — does the other device show this
/// number — and either may answer first, so the source carries whether its own
/// user has already said yes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Source: the offer has gone and the target has not accepted yet.
    Offered {
        /// Whether the shell has been asked to prompt yet.
        prompted: bool,
        /// Whether this device's user has already compared the numbers.
        confirmed: bool,
    },
    /// Source: the target accepted and this device's user has not answered.
    Comparing,
    /// Target: an offer arrived and this device's user has not answered.
    Invited {
        /// Whether the shell has been asked to prompt yet.
        prompted: bool,
    },
    /// Target: this device accepted and is waiting for the key.
    Accepted,
}

/// How a transfer ended, for whichever end was waiting on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The key arrived and is waiting for the shell to take it.
    Received,
    /// The key left this device, so the peer now holds this identity too.
    Sent,
    /// One of the two users said no, or the peer could not run the flow.
    Refused,
}

impl IdentityTransfer {
    /// Start the flow, which is the user tapping "log in another device".
    ///
    /// Whether this device is in a position to offer at all is the session's
    /// call, not this one's.
    pub fn offer(&mut self) -> Result<Vec<u8>> {
        if self.step.is_some() {
            bail!("an identity transfer is already running on this link");
        }

        self.step = Some(Step::Offered {
            prompted: false,
            confirmed: false,
        });

        Ok(vec![message::OFFER])
    }

    /// The user answered the prompt, which on both devices is the same
    /// question: does the other one show this number.
    ///
    /// The key leaves in here and in [`receive`](Self::receive), and nowhere
    /// else, once both users have said yes.
    pub fn answer(&mut self, confirmed: bool, identity: &SecretKey) -> Result<Option<Vec<u8>>> {
        let Some(step) = self.step else {
            bail!("no identity transfer is waiting on the user");
        };

        if !confirmed {
            self.step = None;

            return Ok(Some(vec![message::DECLINE]));
        }

        match step {
            Step::Invited { .. } => {
                self.step = Some(Step::Accepted);

                Ok(Some(vec![message::ACCEPT]))
            }
            Step::Offered { prompted, .. } => {
                self.step = Some(Step::Offered {
                    prompted,
                    confirmed: true,
                });

                Ok(None)
            }
            Step::Comparing => Ok(Some(self.send_key(identity))),
            Step::Accepted => bail!("this device has already accepted the identity transfer"),
        }
    }

    /// One step of the exchange off the control channel, answering with
    /// whatever this end sends back.
    ///
    /// `attended` is whether a user is in front of this screen, and every step
    /// needs one, not just the first: a flow arriving at a device where nobody
    /// is is declined rather than held, and neither end hands a key over or
    /// takes one in the background.
    pub fn receive(
        &mut self,
        payload: &[u8],
        identity: &SecretKey,
        attended: bool,
    ) -> Result<Option<Vec<u8>>> {
        let Some((&message, body)) = payload.split_first() else {
            bail!("an empty identity transfer frame arrived");
        };

        if !attended && message != message::DECLINE {
            return Ok(Some(self.decline()));
        }

        match (message, self.step) {
            (message::OFFER, None) => {
                self.step = Some(Step::Invited { prompted: false });

                Ok(None)
            }
            // Either user may answer first, so an accept lands on a source already confident.
            (
                message::ACCEPT,
                Some(Step::Offered {
                    confirmed: true, ..
                }),
            ) => Ok(Some(self.send_key(identity))),
            (message::ACCEPT, Some(Step::Offered { .. })) => {
                self.step = Some(Step::Comparing);

                Ok(None)
            }
            (message::KEY, Some(Step::Accepted)) => {
                self.received = Some(read_key(body)?);
                self.finish(Outcome::Received);

                Ok(None)
            }
            (message::DECLINE, Some(_)) => {
                self.finish(Outcome::Refused);

                Ok(None)
            }
            (message, step) => {
                bail!("identity transfer message {message} arrived with the flow at {step:?}")
            }
        }
    }

    /// The comparison value to put in front of the user, once per prompt.
    pub fn take_prompt(&mut self, handshake_hash: &[u8; 32]) -> Option<u32> {
        match &mut self.step {
            Some(Step::Offered { prompted, .. } | Step::Invited { prompted }) if !*prompted => {
                *prompted = true;

                Some(sas(LOGIN_LABEL, handshake_hash, LOGIN_SPACE))
            }
            _ => None,
        }
    }

    /// How the transfer ended, handed over once.
    pub fn take_outcome(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }

    /// A key this exchange took off the wire, handed over once.
    ///
    /// Nothing here adopts it: the shell writes it to secure storage and
    /// reopens the node under it, which is the same custody path a key
    /// generated on device takes. `docs/keys.md#key-custody`.
    pub fn take_identity(&mut self) -> Option<SecretKey> {
        self.received.take()
    }

    /// Whether a transfer is running on this session.
    #[must_use]
    pub fn running(&self) -> bool {
        self.step.is_some()
    }

    /// Give up whatever is running, because nobody is in front of the screen
    /// any more, answering with the `DECLINE` the peer is owed.
    pub fn cancel(&mut self) -> Option<Vec<u8>> {
        self.running().then(|| self.decline())
    }

    /// End a running flow as refused and answer `DECLINE`. With nothing
    /// running, the peer is told no and nobody here was waiting.
    fn decline(&mut self) -> Vec<u8> {
        if self.running() {
            self.finish(Outcome::Refused);
        }

        vec![message::DECLINE]
    }

    /// Hand the identity key over, which both users have now agreed to.
    fn send_key(&mut self, identity: &SecretKey) -> Vec<u8> {
        self.finish(Outcome::Sent);

        key_frame(identity)
    }

    /// End the flow, with whoever was waiting to be told how.
    fn finish(&mut self, outcome: Outcome) {
        self.step = None;
        self.outcome = Some(outcome);
    }
}

/// A `KEY` frame: the discriminant and the 32 secret key bytes.
///
/// `coracle-lib` opens one door out of a `SecretKey` and it is hex, so the
/// round trip lives here rather than at the call site.
fn key_frame(identity: &SecretKey) -> Vec<u8> {
    let hex = Zeroizing::new(identity.to_hex());
    let bytes = Zeroizing::new(hex::decode(&*hex).expect("a key is 32 bytes of hex"));

    let mut payload = vec![message::KEY];
    payload.extend_from_slice(&bytes);

    payload
}

/// The key a `KEY` frame carried.
fn read_key(payload: &[u8]) -> Result<SecretKey> {
    if payload.len() != 32 {
        bail!(
            "an identity transfer carried {} bytes, not 32",
            payload.len()
        );
    }

    SecretKey::from_hex(&Zeroizing::new(hex::encode(payload)))
        .map_err(|error| anyhow!("an identity transfer carried an unusable key: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::secret;

    /// The transcript both ends of a test share.
    const TRANSCRIPT: [u8; 32] = [7; 32];

    /// The key being moved. Only the source holds one that matters.
    fn source_key() -> SecretKey {
        secret(1)
    }

    /// Hand `payload` to `into`, which holds `identity`, and answer with what
    /// it says back.
    fn deliver(
        into: &mut IdentityTransfer,
        identity: &SecretKey,
        payload: Option<Vec<u8>>,
    ) -> Option<Vec<u8>> {
        into.receive(&payload?, identity, true).unwrap()
    }

    /// A source that has offered and a target that has been asked.
    fn offered() -> (IdentityTransfer, IdentityTransfer) {
        let mut source = IdentityTransfer::default();
        let mut target = IdentityTransfer::default();

        let offer = source.offer().unwrap();

        assert_eq!(deliver(&mut target, &secret(2), Some(offer)), None);

        (source, target)
    }

    #[test]
    fn an_identity_moves_once_both_users_have_compared_the_same_number() {
        let (mut source, mut target) = offered();

        // Both devices put the same number on screen, and each is asked once.
        let shown = source.take_prompt(&TRANSCRIPT).unwrap();
        assert_eq!(target.take_prompt(&TRANSCRIPT), Some(shown));
        assert_eq!(target.take_prompt(&TRANSCRIPT), None);

        let accept = target.answer(true, &secret(2)).unwrap();
        assert_eq!(deliver(&mut source, &source_key(), accept), None);

        let key = source.answer(true, &source_key()).unwrap();
        assert_eq!(deliver(&mut target, &secret(2), key), None);

        assert_eq!(
            target.take_identity().map(|key| key.to_hex()),
            Some(source_key().to_hex())
        );
        assert_eq!(target.take_outcome(), Some(Outcome::Received));
        assert!(!source.running());
        assert_eq!(source.take_outcome(), Some(Outcome::Sent));
    }

    #[test]
    fn a_source_nobody_is_looking_at_declines_rather_than_sends() {
        let (mut source, mut target) = offered();

        assert_eq!(source.answer(true, &source_key()).unwrap(), None);

        let accept = target.answer(true, &secret(2)).unwrap().unwrap();
        let answer = source.receive(&accept, &source_key(), false).unwrap();

        assert_eq!(answer, Some(vec![message::DECLINE]));
        assert_eq!(source.take_outcome(), Some(Outcome::Refused));
    }

    #[test]
    fn a_target_nobody_is_looking_at_does_not_take_the_key() {
        let (mut source, mut target) = offered();

        let accept = target.answer(true, &secret(2)).unwrap();
        deliver(&mut source, &source_key(), accept);
        let key = source.answer(true, &source_key()).unwrap().unwrap();

        assert_eq!(
            target.receive(&key, &secret(2), false).unwrap(),
            Some(vec![message::DECLINE])
        );
        assert!(target.take_identity().is_none());
        assert_eq!(target.take_outcome(), Some(Outcome::Refused));
    }

    #[test]
    fn the_source_may_compare_before_the_target_answers() {
        let (mut source, mut target) = offered();

        // Confirming first only records it; the key waits on the accept.
        assert_eq!(source.answer(true, &source_key()).unwrap(), None);

        let accept = target.answer(true, &secret(2)).unwrap();
        let key = deliver(&mut source, &source_key(), accept)
            .expect("the key, since the source already said yes");

        assert_eq!(deliver(&mut target, &secret(2), Some(key)), None);
        assert_eq!(
            target.take_identity().map(|key| key.to_hex()),
            Some(source_key().to_hex())
        );
        assert_eq!(source.take_outcome(), Some(Outcome::Sent));
    }

    #[test]
    fn a_target_that_says_no_is_left_with_nothing() {
        let (mut source, mut target) = offered();

        let decline = target.answer(false, &secret(2)).unwrap();
        assert_eq!(deliver(&mut source, &source_key(), decline), None);

        assert!(!source.running());
        assert_eq!(source.take_outcome(), Some(Outcome::Refused));
        assert!(target.take_identity().is_none());

        // The source has confirmed nothing and its key stays put.
        assert!(source.answer(true, &source_key()).is_err());
    }

    #[test]
    fn an_offer_arriving_where_nobody_is_looking_is_declined_without_a_prompt() {
        let mut source = IdentityTransfer::default();
        let mut target = IdentityTransfer::default();

        let offer = source.offer().unwrap();
        let decline = target.receive(&offer, &secret(2), false).unwrap();

        assert!(!target.running());
        assert_eq!(target.take_prompt(&TRANSCRIPT), None);
        assert_eq!(deliver(&mut source, &source_key(), decline), None);
        assert_eq!(source.take_outcome(), Some(Outcome::Refused));
    }

    #[test]
    fn a_second_offer_over_the_same_link_is_refused() {
        let mut source = IdentityTransfer::default();

        source.offer().unwrap();

        assert!(source.offer().is_err());
    }

    #[test]
    fn a_step_arriving_out_of_order_is_refused() {
        let mut target = IdentityTransfer::default();

        assert!(target.receive(&[], &secret(2), true).is_err());
        assert!(
            target
                .receive(&[message::ACCEPT], &secret(2), true)
                .is_err()
        );
        assert!(target.answer(true, &secret(2)).is_err());
    }

    #[test]
    fn a_key_round_trips_through_the_wire_form() {
        let identity = SecretKey::generate();

        assert_eq!(
            read_key(&key_frame(&identity)[1..]).unwrap().to_hex(),
            identity.to_hex()
        );
    }

    #[test]
    fn a_key_payload_is_exactly_thirty_two_bytes() {
        assert!(read_key(&[0; 31]).is_err());
        assert!(read_key(&[0; 33]).is_err());
    }
}
