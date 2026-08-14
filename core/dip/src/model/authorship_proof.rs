//! Authorship proofs: what carries an event its second hop.
//!
//! Content events are unsigned, so authorship at the first hop is the
//! authenticated session and nothing else. At the second hop there is no
//! session with the author, so the forwarder proves in zero knowledge —
//! designated to exactly one verifier — that it holds the author's signature
//! over this event and its own pubkey. `docs/proofs.md` carries the reasoning;
//! this is the arithmetic.
//!
//! # Who calls what
//!
//! | Who | Call | Reads a secret key |
//! | --- | --- | --- |
//! | A, handing an event to B | [`RecipientSignature::sign`] | its identity key |
//! | B, on ingest | [`RecipientSignature::verifies`] | no |
//! | B, forwarding to C | [`AuthorshipProof::prove`] | **no** |
//! | C, on ingest | [`AuthorshipProof::verifies`] | no |
//!
//! Forwarding proves knowledge of A's signature scalar, which B already holds,
//! so it never opens secure storage and cannot be steered into signing as B.
//! The only key B reads in an encounter is for its own auth event.
//!
//! [`AuthorshipProof::simulate`] is the fifth call and is on no path: it is how
//! C fabricates a transcript indistinguishable from one B produced, which is
//! what the deniability claim means and the only way to test it.
//!
//! # The construction
//!
//! A's signature is BIP-340 `(R, s)` under key `A`, verifying as
//! `s·G = R + e·A`. Every term but `s` is public, so the verifier computes
//! `S = R + e·A` itself and the forwarder proves knowledge of `dlog(S)`,
//! OR-composed with knowledge of `dlog(C)` — a Cramer–Damgård–Schoenmakers
//! OR-proof made non-interactive with Fiat–Shamir. The prover runs the branch
//! it can and simulates the other; the verifier cannot tell which, and knows
//! it did not produce the proof itself, so it must be the first.
//!
//! The two branches are ordered by role, never by which one is real, so the
//! transcript is byte-shaped identically whichever side built it.
//!
//! ## The two hashes
//!
//! ```text
//! m = tagged_hash("dip/authorship-signature", event_id ‖ recipient_pubkey)
//! e = tagged_hash("BIP0340/challenge", R.x ‖ A.x ‖ m)
//! ```
//!
//! `e` is the one value here that has to agree byte for byte with the binding:
//! get it wrong and `S` is not the point the signature is over, no proof this
//! device makes ever verifies, and nothing local notices.

use anyhow::{Context, Result, anyhow, bail};
use coracle_lib::keys::{PublicKey, SecretKey};
use k256::elliptic_curve::ops::Reduce;
use k256::elliptic_curve::point::{AffineCoordinates, DecompressPoint};
use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::elliptic_curve::subtle::{Choice, ConditionallySelectable};
use k256::elliptic_curve::zeroize::Zeroizing;
use k256::elliptic_curve::{Group, PrimeField};
use k256::{AffinePoint, FieldBytes, ProjectivePoint, Scalar, U256};
use sha2::{Digest, Sha256};

use crate::model::RecipientSignature;

/// The message A signs. Commits to the event and the recipient, and to nothing
/// else — two recipients of one event get two signatures, and neither carries
/// to the other.
const SIGNATURE_TAG: &[u8] = b"dip/authorship-signature";

/// BIP-340's own challenge tag. Fixed by the spec: `e` has to be the value
/// libsecp256k1 computed, or `S` is not the point the signature is over and
/// every proof fails at the peer.
const BIP340_CHALLENGE_TAG: &[u8] = b"BIP0340/challenge";

/// Masks the witness before it feeds the nonce derivation.
const AUX_TAG: &[u8] = b"dip/authorship-proof/aux";

/// Binds the nonce seed to the witness and the statement.
const NONCE_TAG: &[u8] = b"dip/authorship-proof/nonce";

/// Splits the seed into the three values one proof needs.
const DERIVE_TAG: &[u8] = b"dip/authorship-proof/derive";

/// The Fiat–Shamir challenge.
const CHALLENGE_TAG: &[u8] = b"dip/authorship-proof/challenge";

/// The transcript ahead of the commitments: the event, the three parties, the
/// signature's nonce point, and the point they determine.
const STATEMENT_BYTES: usize = 32 + 32 + 32 + 32 + 32 + 33;

/// What an authorship proof is about.
///
/// Both sides assemble this from what they already have — the event, the
/// pubkeys the peer authenticated as, and their own identity — so a proof is
/// meaningless detached from one and carries none of it on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorshipClaim {
    /// The event the proof is about.
    pub event_id: [u8; 32],
    /// The pubkey the event names as its author. `A`.
    pub author: PublicKey,
    /// The peer forwarding the event, which holds the author's signature. `B`.
    pub holder: PublicKey,
    /// The peer the proof is designated to, and the only one it convinces. `C`.
    pub verifier: PublicKey,
}

impl AuthorshipClaim {
    /// The claim a stored signature supports, forwarding to `verifier`.
    ///
    /// Everything but the verifier comes off the signature: the holder is the
    /// recipient it names, since a signature naming anyone else is one this
    /// device cannot prove anything with, and the author is the one it is by.
    pub fn from_signature(signature: &RecipientSignature, verifier: PublicKey) -> Result<Self> {
        Ok(Self {
            event_id: event_id_bytes(&signature.event_id)?,
            author: signature.author_pubkey,
            holder: signature.recipient_pubkey,
            verifier,
        })
    }
}

/// A designated-verifier proof that its holder has the author's signature over
/// an event, convincing only to the verifier it was built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorshipProof {
    /// The author's signature nonce point, x-only. Half of A's signature, and
    /// inert without the other half — the verifier needs it to reconstruct the
    /// point the proof is about, and it establishes nothing on its own.
    pub nonce_point: [u8; 32],
    /// The challenge share of the branch asserting the author's signature.
    pub author_challenge: [u8; 32],
    /// The response on that branch.
    pub author_response: [u8; 32],
    /// The challenge share of the branch asserting the verifier's own key.
    pub verifier_challenge: [u8; 32],
    /// The response on that branch.
    pub verifier_response: [u8; 32],
}

impl AuthorshipProof {
    /// The proof's length on the wire.
    pub const BYTES: usize = 160;

    /// Prove to `verifier` that this device holds the author's signature over
    /// the event the signature names.
    ///
    /// Reads no secret key. The witness is the signature's own scalar, which
    /// the caller already holds, so this cannot act as a signing oracle for
    /// anyone.
    ///
    /// Fails if the signature is not one over that event, by that author,
    /// naming this device — the check is the proof's own statement asked of the
    /// arithmetic it is about to run on, so a proof that would be rejected by
    /// the peer is refused here instead.
    pub fn prove(signature: &RecipientSignature, verifier: PublicKey) -> Result<Self> {
        let claim = AuthorshipClaim::from_signature(signature, verifier)?;

        // R.x is public; the scalar beside it is the whole capability, so it is
        // wrapped rather than dropped by hand — every path out of here from the
        // next line on is an early return.
        let mut nonce_point = [0u8; 32];
        nonce_point.copy_from_slice(&signature.sig[..32]);

        let mut response = Zeroizing::new([0u8; 32]);
        response.copy_from_slice(&signature.sig[32..]);

        let witness = Zeroizing::new(
            scalar_from_bytes(&response)
                .context("the signature's scalar is not in range for the curve")?,
        );

        let statement = Statement::assemble(&claim, &nonce_point)?;

        if ProjectivePoint::GENERATOR * *witness != statement.signature_point {
            bail!("the signature is not over this event, recipient and author");
        }

        // Which branch is real is not a secret — B is forwarding, so B proves
        // the signature branch and simulates the verifier's. Only the transcript
        // has to hide it, and it does: the branches are ordered by role.
        statement.assemble_proof(Branch::Author, &witness, nonce_point)
    }

    /// Fabricate a proof for a claim designating this device, from its own key.
    ///
    /// On no path in the app. This is the other half of the deniability claim —
    /// what C could have produced for itself, drawn from the same distribution
    /// as what B produced — and the only way to test the claim rather than
    /// assert it.
    ///
    /// `nonce_point` may be any valid x-only point: the verifier can no more
    /// check it than it can check the rest of the signature, which it never
    /// sees.
    pub fn simulate(
        claim: &AuthorshipClaim,
        nonce_point: &[u8; 32],
        verifier: &SecretKey,
    ) -> Result<Self> {
        if verifier.public_key() != claim.verifier {
            bail!("that key is not the verifier the claim designates");
        }

        let statement = Statement::assemble(claim, nonce_point)?;
        let witness = verifier_witness(verifier)?;

        statement.assemble_proof(Branch::Verifier, &witness, *nonce_point)
    }

    /// Whether this proof establishes `claim` to the device the claim
    /// designates.
    ///
    /// Malformed input — a scalar out of range, a nonce point off the curve, a
    /// commitment at infinity — is rejected before any arithmetic runs, and is
    /// indistinguishable from a proof that simply does not verify. Both are the
    /// same outcome: the event is dropped.
    #[must_use]
    pub fn verifies(&self, claim: &AuthorshipClaim) -> bool {
        self.check(claim).is_ok()
    }

    fn check(&self, claim: &AuthorshipClaim) -> Result<()> {
        let author_challenge = scalar_from_bytes(&self.author_challenge)
            .context("the author challenge is out of range")?;
        let author_response = scalar_from_bytes(&self.author_response)
            .context("the author response is out of range")?;
        let verifier_challenge = scalar_from_bytes(&self.verifier_challenge)
            .context("the verifier challenge is out of range")?;
        let verifier_response = scalar_from_bytes(&self.verifier_response)
            .context("the verifier response is out of range")?;

        let statement = Statement::assemble(claim, &self.nonce_point)?;

        let author_commitment = ProjectivePoint::GENERATOR * author_response
            - statement.signature_point * author_challenge;
        let verifier_commitment = ProjectivePoint::GENERATOR * verifier_response
            - statement.verifier_point * verifier_challenge;

        let challenge = statement.challenge(&author_commitment, &verifier_commitment)?;

        if challenge != author_challenge + verifier_challenge {
            bail!("the proof's challenge shares do not add up");
        }

        Ok(())
    }

    /// The proof as it goes on the wire.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; Self::BYTES] {
        let mut bytes = [0u8; Self::BYTES];

        bytes[..32].copy_from_slice(&self.nonce_point);
        bytes[32..64].copy_from_slice(&self.author_challenge);
        bytes[64..96].copy_from_slice(&self.author_response);
        bytes[96..128].copy_from_slice(&self.verifier_challenge);
        bytes[128..].copy_from_slice(&self.verifier_response);

        bytes
    }

    /// Read a proof off the wire.
    ///
    /// Nothing is checked here; the values are five opaque field-widths until
    /// [`verifies`](Self::verifies) looks at them.
    #[must_use]
    pub fn from_bytes(bytes: &[u8; Self::BYTES]) -> Self {
        let field = |start: usize| {
            let mut value = [0u8; 32];
            value.copy_from_slice(&bytes[start..start + 32]);
            value
        };

        Self {
            nonce_point: field(0),
            author_challenge: field(32),
            author_response: field(64),
            verifier_challenge: field(96),
            verifier_response: field(128),
        }
    }
}

/// The two branches, in the order the transcript takes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Branch {
    /// Knowledge of the author's signature scalar.
    Author,
    /// Knowledge of the verifier's own key.
    Verifier,
}

/// The public statement a proof is about, and everything derived from it that
/// both sides have to compute the same way.
///
/// Assembling it is where malformed input is rejected: every point is lifted
/// and checked here, before a scalar multiplication runs.
struct Statement {
    /// `S = R + e·A`, the point whose discrete log is the author's signature
    /// scalar.
    signature_point: ProjectivePoint,
    /// `lift_x(C)`, the point whose discrete log is the verifier's key.
    verifier_point: ProjectivePoint,
    /// The transcript ahead of the commitments.
    prefix: [u8; STATEMENT_BYTES],
}

impl Statement {
    fn assemble(claim: &AuthorshipClaim, nonce_point: &[u8; 32]) -> Result<Self> {
        let nonce =
            lift_x(nonce_point).context("the signature's nonce point is not on the curve")?;
        let author =
            lift_x(&claim.author.as_bytes()).context("the author's key is not on the curve")?;
        let verifier_point =
            lift_x(&claim.verifier.as_bytes()).context("the verifier's key is not on the curve")?;

        let message = signature_message(&claim.event_id, &claim.holder);
        let challenge = scalar_from_hash(&tagged_hash(
            BIP340_CHALLENGE_TAG,
            &[nonce_point, &claim.author.as_bytes(), &message],
        ));

        let signature_point = nonce + author * challenge;
        let encoded =
            encode_point(&signature_point).context("the signature names the point at infinity")?;

        let mut prefix = [0u8; STATEMENT_BYTES];
        prefix[..32].copy_from_slice(&claim.event_id);
        prefix[32..64].copy_from_slice(&claim.author.as_bytes());
        prefix[64..96].copy_from_slice(&claim.holder.as_bytes());
        prefix[96..128].copy_from_slice(&claim.verifier.as_bytes());
        prefix[128..160].copy_from_slice(nonce_point);
        prefix[160..].copy_from_slice(&encoded);

        Ok(Self {
            signature_point,
            verifier_point,
            prefix,
        })
    }

    /// The point of the branch `witness` is the discrete log of.
    fn point(&self, branch: Branch) -> ProjectivePoint {
        match branch {
            Branch::Author => self.signature_point,
            Branch::Verifier => self.verifier_point,
        }
    }

    /// Prove `branch` and simulate the other.
    ///
    /// One body for both directions, so a real proof and a simulated one differ
    /// in nothing but which branch the witness belongs to — which is the whole
    /// deniability argument, and worth having the compiler enforce rather than
    /// two functions that have to be read side by side.
    fn assemble_proof(
        &self,
        branch: Branch,
        witness: &Scalar,
        nonce_point: [u8; 32],
    ) -> Result<AuthorshipProof> {
        let (nonce, simulated_challenge, simulated_response) =
            self.nonces(witness).context("deriving the proof nonce")?;

        // The simulated branch picks its challenge and response and solves for
        // the commitment; the real one commits first and answers afterwards.
        let simulated_commitment = ProjectivePoint::GENERATOR * simulated_response
            - self.point(other(branch)) * simulated_challenge;
        let real_commitment = ProjectivePoint::GENERATOR * *nonce;

        let (author_commitment, verifier_commitment) = match branch {
            Branch::Author => (real_commitment, simulated_commitment),
            Branch::Verifier => (simulated_commitment, real_commitment),
        };

        let challenge = self.challenge(&author_commitment, &verifier_commitment)?;
        let real_challenge = challenge - simulated_challenge;
        let real_response = *nonce + real_challenge * witness;

        let (author, verifier) = match branch {
            Branch::Author => (
                (real_challenge, real_response),
                (simulated_challenge, simulated_response),
            ),
            Branch::Verifier => (
                (simulated_challenge, simulated_response),
                (real_challenge, real_response),
            ),
        };

        Ok(AuthorshipProof {
            nonce_point,
            author_challenge: scalar_bytes(&author.0),
            author_response: scalar_bytes(&author.1),
            verifier_challenge: scalar_bytes(&verifier.0),
            verifier_response: scalar_bytes(&verifier.1),
        })
    }

    /// The Fiat–Shamir challenge over the statement and both commitments.
    fn challenge(
        &self,
        author_commitment: &ProjectivePoint,
        verifier_commitment: &ProjectivePoint,
    ) -> Result<Scalar> {
        let author = encode_point(author_commitment)
            .context("the author commitment is the point at infinity")?;
        let verifier = encode_point(verifier_commitment)
            .context("the verifier commitment is the point at infinity")?;

        Ok(scalar_from_hash(&tagged_hash(
            CHALLENGE_TAG,
            &[&self.prefix, &author, &verifier],
        )))
    }

    /// The nonce, and the simulated branch's challenge and response.
    ///
    /// Everything feeding the Fiat–Shamir challenge feeds the nonce, so the
    /// nonce cannot hold still while the challenge moves — two proofs of one
    /// statement are then the same proof, which reveals nothing, rather than
    /// two proofs under one nonce, which reveals the witness. Nothing the
    /// verifier supplies per session goes in, so the verifier cannot pin one
    /// input while another varies. Auxiliary randomness is mixed in on top: it
    /// makes repeat proofs distinct, and a failing OS RNG degrades to the
    /// deterministic case rather than to a repeated nonce.
    /// Everything on the way to the nonce reconstructs the witness given one
    /// other value, so all of it is wrapped: `aux` and `mask` recover it from
    /// `masked`, and `seed` and the nonce give it up alongside a second proof.
    /// Only the two simulated values are safe to hand back bare, because they
    /// go on the wire.
    fn nonces(&self, witness: &Scalar) -> Result<(Zeroizing<Scalar>, Scalar, Scalar)> {
        let mut aux = Zeroizing::new([0u8; 32]);

        // Safe to carry on without it — the derivation is deterministic anyway,
        // so proofs repeat byte for byte rather than repeating a nonce under a
        // moving challenge. Not safe to say nothing: an RNG that never works is
        // a platform fault reaching well past this module, and this is the only
        // place positioned to notice.
        if let Err(error) = getrandom::getrandom(&mut *aux) {
            log::error!("the OS RNG refused ({error}); proof nonces are deterministic");
        }

        let mask = Zeroizing::new(tagged_hash(AUX_TAG, &[&*aux]));
        let mut masked = Zeroizing::new(scalar_bytes(witness));
        for (byte, mask) in masked.iter_mut().zip(*mask) {
            *byte ^= mask;
        }

        let seed = Zeroizing::new(tagged_hash(NONCE_TAG, &[&*masked, &self.prefix]));

        let derive = |index: u8| scalar_from_hash(&tagged_hash(DERIVE_TAG, &[&*seed, &[index]]));
        let nonce = Zeroizing::new(derive(0));
        let challenge = derive(1);
        let response = derive(2);

        if bool::from(nonce.is_zero()) {
            bail!("the derived nonce was zero");
        }

        Ok((nonce, challenge, response))
    }
}

impl RecipientSignature {
    /// Sign `event_id` for `recipient`, as its author.
    ///
    /// This is the commitment that lets the recipient forward the event, and
    /// the only thing that does. It is verifiable by anyone, so it goes to the
    /// peer it names and to nobody else.
    ///
    /// Signing goes through the audited binding's own nonce derivation.
    /// Deterministic under BIP-340 and over a message distinct per event and
    /// recipient, so no two of these share a nonce — reuse across two messages
    /// would hand the identity key to anyone holding both, and recipients hold
    /// them by design.
    ///
    /// The caller holds the key across a batch: an encounter signs one of these
    /// per event per peer, and that is one read from the Keychain or Keystore,
    /// not hundreds.
    #[must_use]
    pub fn sign(author: &SecretKey, event_id: &[u8; 32], recipient: PublicKey) -> Self {
        Self {
            event_id: hex::encode(event_id),
            author_pubkey: author.public_key(),
            recipient_pubkey: recipient,
            sig: author.sign(&signature_message(event_id, &recipient)),
        }
    }

    /// Whether this is the author's signature over the event and recipient it
    /// names.
    ///
    /// The check an inbound signature has to pass before it is stored. The
    /// author it claims has to be the event's own, which the store enforces on
    /// write — check that too, or this answers a question nobody asked.
    #[must_use]
    pub fn verifies(&self) -> bool {
        let Ok(event_id) = event_id_bytes(&self.event_id) else {
            return false;
        };
        let Ok(signature) = secp256k1::schnorr::Signature::from_slice(&self.sig) else {
            return false;
        };
        let Ok(pubkey) = secp256k1::XOnlyPublicKey::from_slice(&self.author_pubkey.as_bytes())
        else {
            return false;
        };

        let message =
            secp256k1::Message::from_digest(signature_message(&event_id, &self.recipient_pubkey));

        secp256k1::SECP256K1
            .verify_schnorr(&signature, &message, &pubkey)
            .is_ok()
    }
}

/// The 32 bytes A signs: the event and the recipient, and nothing else.
fn signature_message(event_id: &[u8; 32], recipient: &PublicKey) -> [u8; 32] {
    tagged_hash(SIGNATURE_TAG, &[event_id, &recipient.as_bytes()])
}

/// The scalar whose base multiple is `lift_x` of the verifier's key.
///
/// An x-only key names the even-`y` point, which is the negation of the
/// signer's own scalar half the time. Reads the key material, which is why it
/// is only reachable from [`AuthorshipProof::simulate`] and never from
/// [`AuthorshipProof::prove`].
///
/// `to_hex` is the one way key bytes leave a `SecretKey`, and it is a
/// three-copy escape — the hex, the decoded bytes, the scalar — so each lands
/// somewhere that scrubs itself on the way out.
fn verifier_witness(verifier: &SecretKey) -> Result<Zeroizing<Scalar>> {
    let hex = Zeroizing::new(verifier.to_hex());
    let decoded = Zeroizing::new(hex::decode(&*hex).context("the verifier's key is not hex")?);
    let bytes = Zeroizing::new(
        <[u8; 32]>::try_from(&decoded[..])
            .map_err(|_| anyhow!("the verifier's key is not 32 bytes"))?,
    );

    let scalar =
        Zeroizing::new(scalar_from_bytes(&bytes).context("the verifier's key is out of range")?);
    let odd = (ProjectivePoint::GENERATOR * *scalar)
        .to_affine()
        .y_is_odd();

    Ok(Zeroizing::new(Scalar::conditional_select(
        &scalar, &-*scalar, odd,
    )))
}

/// BIP-340's tagged hash: `sha256(sha256(tag) ‖ sha256(tag) ‖ parts)`.
fn tagged_hash(tag: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let prefix = Sha256::digest(tag);
    let mut hasher = Sha256::new();

    hasher.update(prefix);
    hasher.update(prefix);

    for part in parts {
        hasher.update(part);
    }

    hasher.finalize().into()
}

/// The even-`y` point with this x-coordinate, if there is one.
fn lift_x(x: &[u8; 32]) -> Option<ProjectivePoint> {
    let even = Choice::from(0);
    let affine: Option<AffinePoint> =
        AffinePoint::decompress(FieldBytes::from_slice(x), even).into();

    affine.map(ProjectivePoint::from)
}

/// A point as 33 compressed bytes. `None` at infinity, which would encode
/// short and has no business in a transcript.
fn encode_point(point: &ProjectivePoint) -> Option<[u8; 33]> {
    if bool::from(point.is_identity()) {
        return None;
    }

    let mut bytes = [0u8; 33];
    bytes.copy_from_slice(point.to_affine().to_encoded_point(true).as_bytes());

    Some(bytes)
}

/// A scalar from 32 bytes, rejecting anything at or above the group order.
fn scalar_from_bytes(bytes: &[u8; 32]) -> Option<Scalar> {
    Scalar::from_repr(*FieldBytes::from_slice(bytes)).into()
}

/// A scalar from a hash, reduced rather than rejected.
fn scalar_from_hash(hash: &[u8; 32]) -> Scalar {
    <Scalar as Reduce<U256>>::reduce_bytes(FieldBytes::from_slice(hash))
}

fn scalar_bytes(scalar: &Scalar) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&scalar.to_bytes());

    bytes
}

fn other(branch: Branch) -> Branch {
    match branch {
        Branch::Author => Branch::Verifier,
        Branch::Verifier => Branch::Author,
    }
}

/// An event id as the store keys it, back to the bytes the crypto is over.
fn event_id_bytes(event_id: &str) -> Result<[u8; 32]> {
    hex::decode(event_id)
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or_else(|| anyhow!("{event_id} is not a 32-byte event id"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use coracle_lib::tags::Tags;

    use super::*;
    use crate::fixtures::{note, secret};

    /// A, the author.
    fn a() -> SecretKey {
        secret(1)
    }

    /// B, which receives from A and forwards.
    fn b() -> SecretKey {
        secret(2)
    }

    /// C, which receives from B.
    fn c() -> SecretKey {
        secret(3)
    }

    fn event_id() -> [u8; 32] {
        note(a().public_key(), 100, "forwarded", Tags::new()).id
    }

    fn signature() -> RecipientSignature {
        RecipientSignature::sign(&a(), &event_id(), b().public_key())
    }

    fn claim() -> AuthorshipClaim {
        AuthorshipClaim {
            event_id: event_id(),
            author: a().public_key(),
            holder: b().public_key(),
            verifier: c().public_key(),
        }
    }

    fn proof() -> AuthorshipProof {
        AuthorshipProof::prove(&signature(), c().public_key()).unwrap()
    }

    // ------------------------------------------------------------ the witness

    #[test]
    fn a_signature_verifies_against_the_event_and_recipient_it_names() {
        let signature = signature();

        assert!(signature.verifies());
    }

    #[test]
    fn a_signature_does_not_carry_to_another_recipient() {
        let mut signature = signature();
        signature.recipient_pubkey = secret(4).public_key();

        assert!(!signature.verifies());
    }

    #[test]
    fn a_signature_does_not_carry_to_another_event() {
        let mut signature = signature();
        signature.event_id = hex::encode([9u8; 32]);

        assert!(!signature.verifies());
    }

    #[test]
    fn the_signed_message_is_pinned() {
        // A known-answer vector over `m`. Nothing enforces the tag string or
        // the order of the two values it covers — a peer running the other
        // platform's build just fails to verify anything, quietly — so the
        // bytes are pinned here rather than left to agree by inspection.
        let event_id = [0xabu8; 32];

        assert_eq!(
            hex::encode(signature_message(&event_id, &b().public_key())),
            "382bbaa762f2d34a065ce30062d6e9456e6dbef75b068834a34e5b2dacb7808d"
        );
    }

    #[test]
    fn a_signature_is_pinned() {
        // The same vector carried through the binding. BIP-340 signing is
        // deterministic, so this is stable, and it covers the whole path: the
        // tag, the layout, and that signing goes through `sign_schnorr` with no
        // aux randomness. Changing any of them is a cross-platform break, and
        // this is what says so.
        let event_id = [0xabu8; 32];
        let signature = RecipientSignature::sign(&a(), &event_id, b().public_key());

        assert_eq!(
            a().public_key().to_hex(),
            "1b84c5567b126440995d3ed5aaba0565d71e1834604819ff9c17f5e9d5dd078f"
        );
        assert_eq!(
            b().public_key().to_hex(),
            "4d4b6cd1361032ca9bd2aeb9d900aa4d45d9ead80ac9423374c451a7254d0766"
        );
        assert_eq!(
            hex::encode(signature.sig),
            "3bd36911826271b82475bb6cea09351713c2ac6d8153f2a3e40f0fb9d38d9aeb\
             f7c17cee89f095c0dfa8b8dff9449c014fc88ca1cc01abdcde76fbf842c2c684"
        );
        assert!(signature.verifies());
    }

    #[test]
    fn the_challenge_agrees_with_the_binding() {
        // The one place this implementation has to match libsecp256k1 byte for
        // byte: `e`. If it drifts, `S` is not the point the signature is over,
        // no proof this device makes ever verifies, and nothing else in the
        // module would notice.
        let signature = signature();
        let statement =
            Statement::assemble(&claim(), &signature.sig[..32].try_into().unwrap()).unwrap();

        let mut scalar = [0u8; 32];
        scalar.copy_from_slice(&signature.sig[32..]);

        assert!(signature.verifies());
        assert_eq!(
            ProjectivePoint::GENERATOR * scalar_from_bytes(&scalar).unwrap(),
            statement.signature_point
        );
    }

    // ----------------------------------------------------------- soundness

    #[test]
    fn a_proof_convinces_the_peer_it_is_designated_to() {
        assert!(proof().verifies(&claim()));
    }

    #[test]
    fn a_proof_designated_to_one_verifier_is_rejected_by_another() {
        let proof = proof();
        let mut claim = claim();
        claim.verifier = secret(4).public_key();

        assert!(!proof.verifies(&claim));
    }

    #[test]
    fn a_proof_over_the_wrong_recipient_is_rejected() {
        // The holder is what A signed over. Claiming a different one asks the
        // verifier to reconstruct a point nobody knows the discrete log of.
        let proof = proof();
        let mut claim = claim();
        claim.holder = secret(4).public_key();

        assert!(!proof.verifies(&claim));
    }

    #[test]
    fn a_proof_over_the_wrong_event_is_rejected() {
        let proof = proof();
        let mut claim = claim();
        claim.event_id = [9u8; 32];

        assert!(!proof.verifies(&claim));
    }

    #[test]
    fn a_proof_over_the_wrong_author_is_rejected() {
        let proof = proof();
        let mut claim = claim();
        claim.author = secret(4).public_key();

        assert!(!proof.verifies(&claim));
    }

    #[test]
    fn both_branches_simulated_is_rejected() {
        // Simulating a branch is free: pick the challenge and the response, and
        // the commitment they imply satisfies that branch by construction. Do
        // it on both and the only thing left unsatisfied is the one condition
        // that ties them together — the two challenge shares adding up to a
        // hash over the commitments those very shares determine.
        let proof = AuthorshipProof {
            nonce_point: signature().sig[..32].try_into().unwrap(),
            author_challenge: scalar_bytes(&scalar_from_hash(&[11u8; 32])),
            author_response: scalar_bytes(&scalar_from_hash(&[12u8; 32])),
            verifier_challenge: scalar_bytes(&scalar_from_hash(&[13u8; 32])),
            verifier_response: scalar_bytes(&scalar_from_hash(&[14u8; 32])),
        };

        assert!(!proof.verifies(&claim()));
    }

    #[test]
    fn a_tampered_proof_is_rejected() {
        for index in 0..AuthorshipProof::BYTES {
            let mut bytes = proof().to_bytes();
            bytes[index] ^= 1;

            assert!(
                !AuthorshipProof::from_bytes(&bytes).verifies(&claim()),
                "flipping bit 0 of byte {index} left the proof verifying"
            );
        }
    }

    #[test]
    fn proving_refuses_a_signature_that_does_not_verify() {
        let mut signature = signature();
        signature.sig[40] ^= 1;

        assert!(AuthorshipProof::prove(&signature, c().public_key()).is_err());
    }

    #[test]
    fn proving_refuses_a_signature_naming_another_author() {
        let mut signature = signature();
        signature.author_pubkey = secret(4).public_key();

        assert!(AuthorshipProof::prove(&signature, c().public_key()).is_err());
    }

    // ------------------------------------------------------ malformed input

    #[test]
    fn out_of_range_scalars_are_rejected() {
        // The group order is under 2^256, so all-ones is not a scalar.
        for field in 1..5 {
            let mut bytes = proof().to_bytes();
            bytes[field * 32..(field + 1) * 32].copy_from_slice(&[0xff; 32]);

            assert!(!AuthorshipProof::from_bytes(&bytes).verifies(&claim()));
        }
    }

    /// A proof identical to a real one but for the nonce point it names.
    fn with_nonce_point(x: [u8; 32]) -> AuthorshipProof {
        AuthorshipProof {
            nonce_point: x,
            ..proof()
        }
    }

    /// An x-coordinate as 32 big-endian bytes.
    fn x_of(value: u8) -> [u8; 32] {
        let mut x = [0u8; 32];
        x[31] = value;

        x
    }

    #[test]
    fn a_nonce_point_above_the_field_prime_is_rejected() {
        // All-ones exceeds p, so it is not a field element at all and never
        // reaches the curve equation. The half of "malformed points" that a
        // coordinate off the curve does not cover.
        assert!(!with_nonce_point([0xff; 32]).verifies(&claim()));
    }

    #[test]
    fn a_nonce_point_off_the_curve_is_rejected() {
        // x = 5 is a perfectly good field element with no y to go with it:
        // 5³ + 7 is not a square mod p. x = 1, 2 and 3 all are, which is why
        // this is 5 — a valid point would test something else entirely.
        assert!(!with_nonce_point(x_of(5)).verifies(&claim()));
    }

    #[test]
    fn a_nonce_point_that_is_not_the_signature_is_rejected() {
        // On the curve, so it lifts, and wrong, so it names a point whose
        // discrete log nobody knows. Rejected for a different reason than the
        // two above, which is worth keeping separate.
        assert!(!with_nonce_point(x_of(1)).verifies(&claim()));
    }

    #[test]
    fn a_malformed_event_id_is_rejected() {
        let mut signature = signature();
        signature.event_id = "not hex".to_string();

        assert!(!signature.verifies());
        assert!(AuthorshipProof::prove(&signature, c().public_key()).is_err());
    }

    // ------------------------------------------------------ zero knowledge

    #[test]
    fn the_verifier_could_have_produced_the_proof_itself() {
        // The simulator property, which is what "designated" means: C's own key
        // makes a transcript that passes the same check B's does. D watching
        // the verification learns nothing, because it cannot rule this out.
        let nonce_point: [u8; 32] = signature().sig[..32].try_into().unwrap();
        let simulated = AuthorshipProof::simulate(&claim(), &nonce_point, &c()).unwrap();

        assert!(simulated.verifies(&claim()));
    }

    #[test]
    fn simulating_needs_the_verifier_own_key() {
        let nonce_point: [u8; 32] = signature().sig[..32].try_into().unwrap();

        assert!(AuthorshipProof::simulate(&claim(), &nonce_point, &secret(4)).is_err());
    }

    #[test]
    fn a_simulated_proof_does_not_convince_a_third_party() {
        // C can fabricate for itself and only for itself. Forwarding on would
        // mean a proof designated to D, which takes knowledge C does not have.
        let nonce_point: [u8; 32] = signature().sig[..32].try_into().unwrap();
        let simulated = AuthorshipProof::simulate(&claim(), &nonce_point, &c()).unwrap();

        let mut onward = claim();
        onward.holder = c().public_key();
        onward.verifier = secret(4).public_key();

        assert!(!simulated.verifies(&onward));
    }

    #[test]
    fn real_and_simulated_proofs_are_shaped_alike() {
        // Same statement, same widths, and no field that gives the branch away.
        let nonce_point: [u8; 32] = signature().sig[..32].try_into().unwrap();
        let real = proof();
        let simulated = AuthorshipProof::simulate(&claim(), &nonce_point, &c()).unwrap();

        assert_eq!(real.nonce_point, simulated.nonce_point);
        assert!(real.verifies(&claim()));
        assert!(simulated.verifies(&claim()));
        assert_ne!(real.author_response, simulated.author_response);
    }

    /// Both commitments a proof implies, recovered the way the verifier does.
    fn commitments(statement: &Statement, proof: &AuthorshipProof) -> ([u8; 33], [u8; 33]) {
        let recover = |challenge: &[u8; 32], response: &[u8; 32], point: ProjectivePoint| {
            let challenge = scalar_from_bytes(challenge).unwrap();
            let response = scalar_from_bytes(response).unwrap();

            encode_point(&(ProjectivePoint::GENERATOR * response - point * challenge)).unwrap()
        };

        (
            recover(
                &proof.author_challenge,
                &proof.author_response,
                statement.signature_point,
            ),
            recover(
                &proof.verifier_challenge,
                &proof.verifier_response,
                statement.verifier_point,
            ),
        )
    }

    #[test]
    fn no_two_proofs_of_one_statement_share_a_commitment() {
        // The nonce is derived, not sampled, so a bug here is a nonce that
        // holds still while the challenge moves — which hands over the witness
        // and the two-hop bound with it. Distinct commitments is the visible
        // consequence of getting it right.
        //
        // Both branches of both producers, in one set. The simulator mixes
        // auxiliary randomness through the same derivation, so it has to come
        // out just as fresh: a simulated proof that repeated would be a proof
        // C could be caught having made.
        let signature = signature();
        let nonce_point: [u8; 32] = signature.sig[..32].try_into().unwrap();
        let statement = Statement::assemble(&claim(), &nonce_point).unwrap();

        let mut seen = HashSet::new();

        for round in 0..32 {
            let proofs = [
                AuthorshipProof::prove(&signature, c().public_key()).unwrap(),
                AuthorshipProof::simulate(&claim(), &nonce_point, &c()).unwrap(),
            ];

            for proof in proofs {
                let (author, verifier) = commitments(&statement, &proof);

                assert!(
                    seen.insert(author),
                    "an author commitment repeated at {round}"
                );
                assert!(
                    seen.insert(verifier),
                    "a verifier commitment repeated at {round}"
                );
            }
        }
    }

    /// One of a proof's four scalars, picked out for measuring.
    type Field = fn(&AuthorshipProof) -> [u8; 32];

    /// The fraction of bits set across one field of many proofs.
    fn bit_frequency(
        proofs: &[AuthorshipProof],
        field: impl Fn(&AuthorshipProof) -> [u8; 32],
    ) -> f64 {
        let set: u32 = proofs
            .iter()
            .map(|proof| {
                field(proof)
                    .iter()
                    .map(|byte| byte.count_ones())
                    .sum::<u32>()
            })
            .sum();

        f64::from(set) / (proofs.len() * 256) as f64
    }

    #[test]
    fn real_and_simulated_proofs_are_drawn_alike() {
        // The distributional half of the simulator property. Both populations
        // are uniform subject to one constraint — the challenge shares summing
        // to the transcript hash — so no field of either should look like
        // anything but coin flips, and no field should tell the two apart.
        //
        // A soundness bug shows up as a proof that fails. This is the other
        // kind: a proof that verifies every time while a field holds still, or
        // tracks the witness, or says which branch was real. Verification
        // cannot see any of that, so nothing above would catch it.
        const SAMPLES: usize = 64;
        // 64 x 256 bits a field, so one standard error is about 0.004 and this
        // is a seven-sigma band: it will not flake, and nothing degenerate
        // enough to matter fits inside it.
        const TOLERANCE: f64 = 0.03;

        let signature = signature();
        let nonce_point: [u8; 32] = signature.sig[..32].try_into().unwrap();

        let real: Vec<_> = (0..SAMPLES)
            .map(|_| AuthorshipProof::prove(&signature, c().public_key()).unwrap())
            .collect();
        let simulated: Vec<_> = (0..SAMPLES)
            .map(|_| AuthorshipProof::simulate(&claim(), &nonce_point, &c()).unwrap())
            .collect();

        let fields: [(&str, Field); 4] = [
            ("author_challenge", |proof| proof.author_challenge),
            ("author_response", |proof| proof.author_response),
            ("verifier_challenge", |proof| proof.verifier_challenge),
            ("verifier_response", |proof| proof.verifier_response),
        ];

        for (name, field) in fields {
            let from_prove = bit_frequency(&real, field);
            let from_simulate = bit_frequency(&simulated, field);

            assert!(
                (from_prove - 0.5).abs() < TOLERANCE,
                "{name} is not uniform when proved: {from_prove}"
            );
            assert!(
                (from_simulate - 0.5).abs() < TOLERANCE,
                "{name} is not uniform when simulated: {from_simulate}"
            );
            assert!(
                (from_prove - from_simulate).abs() < TOLERANCE,
                "{name} separates the two: {from_prove} proved against {from_simulate} simulated"
            );
        }
    }

    // ------------------------------------------------------------- the wire

    #[test]
    fn a_proof_survives_the_wire() {
        let proof = proof();

        assert_eq!(AuthorshipProof::from_bytes(&proof.to_bytes()), proof);
        assert!(AuthorshipProof::from_bytes(&proof.to_bytes()).verifies(&claim()));
    }
}
