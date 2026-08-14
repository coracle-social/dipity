//! Writes over `recipient_signature`.
//!
//! Verification is not here. A signature is checked against the event id, the
//! author and the recipient before it reaches this point; the store's job is to
//! keep it against an event it actually holds, by an author that event actually
//! names — both of which the composite foreign key enforces.

use anyhow::{Context, Result};
use coracle_lib::events::EventId;
use coracle_lib::keys::PublicKey;
use rusqlite::params;

use crate::db::Tx;
use crate::model::RecipientSignature;

use super::events::{self, RecipientSignatureChange};

/// Store a signature. Returns whether it was new.
///
/// Signatures are immutable — BIP-340 signing is deterministic, so the same
/// author, event and recipient produce the same 64 bytes — so a repeat is
/// ignored rather than overwritten.
pub fn save(tx: &Tx<'_>, signature: &RecipientSignature) -> Result<bool> {
    let written = tx
        .prepare_cached(
            "INSERT OR IGNORE INTO recipient_signature
                 (event_id, author_pubkey, recipient_pubkey, sig)
             VALUES (?1, ?2, ?3, ?4)",
        )?
        .execute(params![
            signature.event_id.to_hex(),
            signature.author_pubkey.to_hex(),
            signature.recipient_pubkey.to_hex(),
            hex::encode(signature.sig),
        ])
        .with_context(|| format!("storing the signature over {}", signature.event_id))?;

    if written == 0 {
        return Ok(false);
    }

    events::notify(
        tx,
        RecipientSignatureChange::Stored(Box::new(signature.clone())),
    );

    Ok(true)
}

/// Remove one signature. Returns whether it was there.
pub fn remove(tx: &Tx<'_>, event_id: &EventId, recipient_pubkey: &PublicKey) -> Result<bool> {
    let removed = tx
        .prepare_cached(
            "DELETE FROM recipient_signature WHERE event_id = ?1 AND recipient_pubkey = ?2",
        )?
        .execute(params![event_id.to_hex(), recipient_pubkey.to_hex()])
        .with_context(|| format!("removing the signature over {event_id}"))?;

    if removed == 0 {
        return Ok(false);
    }

    events::notify(
        tx,
        RecipientSignatureChange::Removed(*event_id, *recipient_pubkey),
    );

    Ok(true)
}

/// Remove every signature over an event. Returns how many went.
///
/// Deleting the event does this by cascade; this is for dropping the capability
/// to forward while keeping the event readable.
pub fn remove_for_event(tx: &Tx<'_>, event_id: &EventId) -> Result<usize> {
    let stored = super::query::list_for_event(tx, event_id)?;
    let mut removed = 0;

    for signature in stored {
        if remove(tx, event_id, &signature.recipient_pubkey)? {
            removed += 1;
        }
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::tags::Tags;

    use crate::db::event::command as event_command;
    use crate::db::open_in_memory;
    use crate::db::recipient_signature::query;
    use crate::fixtures::{author, id, note, peer};

    fn us() -> PublicKey {
        author(9)
    }

    fn store_note(tx: &Tx<'_>, content: &str) -> EventId {
        let event = note(author(1), 100, content, Tags::new());

        event_command::save(tx, &event, &peer(), 10).unwrap();

        id(&event)
    }

    fn signature(event_id: EventId) -> RecipientSignature {
        RecipientSignature {
            event_id,
            author_pubkey: author(1),
            recipient_pubkey: us(),
            sig: [7u8; 64],
        }
    }

    #[test]
    fn a_signature_is_stored_once() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let event_id = store_note(&tx, "signed");

        assert!(save(&tx, &signature(event_id)).unwrap());
        assert!(!save(&tx, &signature(event_id)).unwrap());
        assert_eq!(query::list_for_event(&tx, &event_id).unwrap().len(), 1);
        assert!(query::exists(&tx, &event_id, &us()).unwrap());
        assert_eq!(
            query::get(&tx, &event_id, &us()).unwrap().unwrap(),
            signature(event_id)
        );
    }

    #[test]
    fn a_signature_cannot_outlive_its_event() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let event_id = store_note(&tx, "signed");
        save(&tx, &signature(event_id)).unwrap();

        event_command::delete(&tx, &event_id).unwrap();

        assert!(query::get(&tx, &event_id, &us()).unwrap().is_none());
    }

    #[test]
    fn a_signature_needs_the_author_its_event_names() {
        // The composite foreign key. An author who did not write the event is
        // not someone whose signature over it could exist, so the row is
        // refused rather than kept for a proof that could never verify.
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let event_id = store_note(&tx, "signed");
        let mut signature = signature(event_id);
        signature.author_pubkey = author(4);

        assert!(save(&tx, &signature).is_err());
    }

    #[test]
    fn a_signature_needs_an_event_to_name() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        assert!(save(&tx, &signature(EventId::new([0u8; 32]))).is_err());
    }

    #[test]
    fn forwardable_answers_for_a_batch() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let signed = store_note(&tx, "signed");
        let unsigned = store_note(&tx, "unsigned");
        save(&tx, &signature(signed)).unwrap();

        let ids = [signed, unsigned];

        assert_eq!(query::forwardable(&tx, &ids, &us()).unwrap(), [signed]);
        assert!(
            query::forwardable(&tx, &ids, &author(4))
                .unwrap()
                .is_empty()
        );
    }
}
