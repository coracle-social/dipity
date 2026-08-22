//! Mutual NIP-42 over the secured channel.
//!
//! Each side challenges the other and answers with a signed kind 22242 event
//! whose relay tag names the channel by its Noise static key, so a response
//! cannot be replayed onto another link. The auth event is the only signed
//! nostr event in the app, and it is portable evidence — which is why the
//! channel key it binds to must not be durable. `docs/nips/p2p-auth.md`,
//! `docs/privacy.md#the-auth-event-is-portable-evidence`.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};
use coracle_lib::events::{Event, EventContent};
use coracle_lib::keys::{PublicKey, SecretKey};
use coracle_lib::tags::Tags;

use crate::clock;

/// One session's mutual `AUTH` exchange, and everything the peer has proved
/// over it.
#[derive(Debug, Default)]
pub struct AuthExchange {
    /// The challenge this device sent, to match against the peer's response.
    sent_challenge: Option<String>,
    /// The peer's challenge, answered when this device chooses to disclose.
    pub peer_challenge: Option<String>,
    /// Whether this device has disclosed its identity by answering.
    pub disclosed: bool,
    /// Every pubkey the peer has proved, accumulated across responses: a peer
    /// may authenticate as several identities over one channel.
    pub proved: BTreeSet<PublicKey>,
}

impl AuthExchange {
    /// Record pubkeys the peer has proved, accumulating rather than replacing.
    pub fn prove(&mut self, pubkeys: impl IntoIterator<Item = PublicKey>) {
        self.proved.extend(pubkeys);
    }

    /// Mint the challenge this device sends, remembering it for verification.
    pub fn make_challenge(&mut self) -> Result<String> {
        let mut bytes = [0u8; 16];
        getrandom::getrandom(&mut bytes).context("generating an AUTH challenge")?;

        let challenge = hex::encode(bytes);
        self.sent_challenge = Some(challenge.clone());

        Ok(challenge)
    }

    /// Record the challenge the peer sent, to answer when this device
    /// discloses.
    pub fn receive_challenge(&mut self, challenge: String) {
        self.peer_challenge = Some(challenge);
    }

    /// Answer the peer's challenge once, disclosing this device's identity.
    ///
    /// `None` when there is no challenge to answer or it has been answered
    /// already; otherwise the signed kind 22242 event, which the caller sends
    /// as NIP-42's own `["AUTH", <event>]`. The relay tag names the peer's
    /// static key — the party that challenged us — binding the response to the
    /// channel their key established.
    pub fn answer(&mut self, identity: &SecretKey, peer_static: [u8; 32]) -> Result<Option<Event>> {
        if self.disclosed {
            return Ok(None);
        }

        let Some(challenge) = self.peer_challenge.clone() else {
            return Ok(None);
        };

        let relay = format!("noise://{}", hex::encode(peer_static));
        let hashed = EventContent::new()
            .with_content("")
            .with_tags(
                Tags::new()
                    .add("challenge", [challenge.as_str()])
                    .add("relay", [relay.as_str()]),
            )
            .with_kind(22_242)
            .with_created_at(clock::now())
            .with_pubkey(identity.public_key())
            .with_id();

        let id = hashed.id;
        let event = hashed.with_sig(identity.sign(id.as_bytes()));

        self.disclosed = true;

        Ok(Some(event))
    }

    /// Verify the peer's response against our challenge and this channel,
    /// returning the pubkey it proved.
    /// Consumes the challenge: `p2p-auth.md` requires one be "accepted once",
    /// so a captured response cannot be replayed against it for the life of
    /// the session.
    ///
    /// The same doc allows a peer to authenticate as several pubkeys over a
    /// sequence of responses. That needs a fresh challenge per response, and
    /// the exchange has no step that issues one mid-session — so today a
    /// second identity is refused rather than accepted unchallenged. Single
    /// use is the security property; the allowance is the thing left unbuilt.
    pub fn verify(&mut self, event: &Event, local_static: [u8; 32]) -> Result<PublicKey> {
        if event.kind != 22_242 {
            bail!("the AUTH response is not kind 22242");
        }

        if event.verify().is_err() {
            bail!("the AUTH response has an invalid signature");
        }

        let challenge = self.sent_challenge.as_ref().ok_or_else(|| {
            anyhow::anyhow!("an AUTH response arrived before a challenge was sent")
        })?;

        let tag_challenge = event
            .tags
            .value("challenge")
            .ok_or_else(|| anyhow::anyhow!("the AUTH response has no challenge tag"))?;
        if tag_challenge != challenge.as_str() {
            bail!("the AUTH response carries a different challenge");
        }

        // We issued the challenge, so the response must name our channel.
        let relay = format!("noise://{}", hex::encode(local_static));
        let tag_relay = event
            .tags
            .value("relay")
            .ok_or_else(|| anyhow::anyhow!("the AUTH response has no relay tag"))?;
        if tag_relay != relay.as_str() {
            bail!("the AUTH response names a different channel");
        }

        // Spent, whatever happens next: a second response has to answer a
        // challenge of its own.
        self.sent_challenge = None;

        Ok(event.pubkey)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures::secret;

    /// The dialer's static key in these tests; the receiver's is `[8; 32]`.
    const DIALER_STATIC: [u8; 32] = [7; 32];
    const RECEIVER_STATIC: [u8; 32] = [8; 32];

    #[test]
    fn a_challenge_round_trips_to_the_proved_pubkey() {
        let mut challenger = AuthExchange::default();
        let mut responder = AuthExchange::default();
        let identity = secret(1);

        let challenge = challenger.make_challenge().unwrap();
        responder.receive_challenge(challenge.clone());

        // The responder names the challenger's channel; the challenger checks
        // its own static key against the relay tag.
        let payload = responder
            .answer(&identity, DIALER_STATIC)
            .unwrap()
            .expect("an answer to the stored challenge");

        assert!(responder.disclosed);
        assert_eq!(
            challenger.verify(&payload, DIALER_STATIC).unwrap(),
            identity.public_key()
        );
    }

    #[test]
    fn an_answer_is_one_shot() {
        let mut responder = AuthExchange::default();
        responder.receive_challenge("abc123".to_string());

        assert!(
            responder
                .answer(&secret(1), DIALER_STATIC)
                .unwrap()
                .is_some()
        );
        assert!(
            responder
                .answer(&secret(1), DIALER_STATIC)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn nothing_is_answered_before_a_challenge_arrives() {
        let mut responder = AuthExchange::default();

        assert!(
            responder
                .answer(&secret(1), DIALER_STATIC)
                .unwrap()
                .is_none()
        );
        assert!(!responder.disclosed);
    }

    #[test]
    fn a_challenge_is_accepted_once() {
        // `p2p-auth.md`: a challenge "is accepted once". Without that, a peer
        // could replay one captured response against the same challenge for
        // the life of the session.
        let mut challenger = AuthExchange::default();
        let mut responder = AuthExchange::default();

        let challenge = challenger.make_challenge().unwrap();
        responder.receive_challenge(challenge.clone());
        let payload = responder
            .answer(&secret(1), DIALER_STATIC)
            .unwrap()
            .unwrap();

        assert!(challenger.verify(&payload, DIALER_STATIC).is_ok());
        assert!(
            challenger.verify(&payload, DIALER_STATIC).is_err(),
            "the same response was accepted twice"
        );
    }

    #[test]
    fn a_response_to_a_different_challenge_is_refused() {
        let mut challenger = AuthExchange::default();
        let mut responder = AuthExchange::default();

        challenger.make_challenge().unwrap();
        responder.receive_challenge("somebody-elses".to_string());

        let payload = responder
            .answer(&secret(1), DIALER_STATIC)
            .unwrap()
            .unwrap();

        assert!(challenger.verify(&payload, DIALER_STATIC).is_err());
    }

    #[test]
    fn a_response_naming_another_channel_is_refused() {
        let mut challenger = AuthExchange::default();
        let mut responder = AuthExchange::default();

        let challenge = challenger.make_challenge().unwrap();
        responder.receive_challenge(challenge.clone());

        // The answer binds to the receiver's channel, not the challenger's, as
        // in a replay onto another link.
        let payload = responder
            .answer(&secret(1), RECEIVER_STATIC)
            .unwrap()
            .unwrap();

        assert!(challenger.verify(&payload, DIALER_STATIC).is_err());
    }

    #[test]
    fn a_response_before_any_challenge_was_sent_is_refused() {
        let mut challenger = AuthExchange::default();
        let mut responder = AuthExchange::default();
        responder.receive_challenge("abc123".to_string());

        let payload = responder
            .answer(&secret(1), DIALER_STATIC)
            .unwrap()
            .unwrap();

        assert!(challenger.verify(&payload, DIALER_STATIC).is_err());
    }
}
