//! This device asking a peer: the client half.
//!
//! Where authorization happens. Every inbound event is authorized by the
//! session or by a proof designated to this device, or it is dropped, and no
//! policy flag relaxes it. `docs/proofs.md#authorship-proofs`.

use anyhow::Result;
use coracle_lib::events::HashedEvent;

use crate::db::Db;
use crate::model::AuthorshipProof;
use crate::session::Peer;
use crate::sync::{Message, Quota};

/// Why an event this device was offered was not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejected {
    /// Neither authored by the peer nor carrying a proof designated to this
    /// device.
    Unauthorized,
    /// Outside the user's Accept scope.
    OutOfScope,
    /// The peer has written as much as it may this session.
    OverQuota,
    /// Larger than the per-event cap.
    TooLarge,
}

/// Handle a message this device's client half received.
pub fn handle(_db: &Db, _peer: &Peer, _message: Message) -> Result<Vec<Message>> {
    todo!("EVENT / EOSE / OK / NEG-MSG")
}

/// Whether an event a peer offered may be stored, and why not.
///
/// Two registers and nothing else. The peer authored it, in which case the
/// authenticated session is proof of authorship to this device and to nobody
/// else, or it holds a proof naming this device, in which case it is a
/// forwarder and the event goes no further. Scope and quota narrow what
/// survives that; neither widens it.
pub fn admits(
    peer: &Peer,
    event: &HashedEvent,
    proof: Option<&AuthorshipProof>,
    _quota: Quota,
) -> Result<(), Rejected> {
    if peer.is_blocked() {
        return Err(Rejected::Unauthorized);
    }

    if !peer.authored(event) && proof.is_none() {
        return Err(Rejected::Unauthorized);
    }

    if !peer.may_store(event) {
        return Err(Rejected::OutOfScope);
    }

    Ok(())
}

/// Take in an event a peer offered, once [`admits`] has passed it.
///
/// The author's signature is stored only when this device is the recipient it
/// names, so the event travels one more hop and no further.
pub fn ingest(
    _db: &Db,
    _peer: &Peer,
    _event: &HashedEvent,
    _proof: Option<&AuthorshipProof>,
) -> Result<bool> {
    todo!("verify the proof, then db::command::receive_event")
}
