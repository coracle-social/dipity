//! Writes over `proof`.
//!
//! Verification is not here. A signature is checked against the event id and
//! the recipient before it reaches this point; the store's job is to keep it
//! against an event it actually holds, which the foreign key enforces.

use anyhow::{Context, Result};
use coracle_lib::keys::PublicKey;
use rusqlite::params;

use crate::db::Tx;
use crate::model::Proof;

use super::events::{self, ProofChange};

/// Store a signature. Returns whether it was new.
///
/// Signatures are immutable — BIP-340 signing is deterministic, so the same
/// author, event and recipient produce the same 64 bytes — so a repeat is
/// ignored rather than overwritten.
///
/// # Errors
///
/// If the write fails, including when the event it names is not stored.
pub fn save(tx: &Tx<'_>, proof: &Proof) -> Result<bool> {
    let written = tx
        .prepare_cached(
            "INSERT OR IGNORE INTO proof (event_id, recipient_pubkey, sig) VALUES (?1, ?2, ?3)",
        )?
        .execute(params![
            proof.event_id,
            proof.recipient_pubkey.to_hex(),
            hex::encode(proof.sig),
        ])
        .with_context(|| format!("storing the signature over {}", proof.event_id))?;

    if written == 0 {
        return Ok(false);
    }

    events::notify(tx, ProofChange::Stored(Box::new(proof.clone())));

    Ok(true)
}

/// Remove one signature. Returns whether it was there.
pub fn remove(tx: &Tx<'_>, event_id: &str, recipient_pubkey: &PublicKey) -> Result<bool> {
    let removed = tx
        .prepare_cached("DELETE FROM proof WHERE event_id = ?1 AND recipient_pubkey = ?2")?
        .execute(params![event_id, recipient_pubkey.to_hex()])
        .with_context(|| format!("removing the signature over {event_id}"))?;

    if removed == 0 {
        return Ok(false);
    }

    events::notify(
        tx,
        ProofChange::Removed(event_id.to_string(), *recipient_pubkey),
    );

    Ok(true)
}

/// Remove every signature over an event. Returns how many went.
///
/// Deleting the event does this by cascade; this is for dropping the capability
/// to forward while keeping the event readable.
pub fn remove_for_event(tx: &Tx<'_>, event_id: &str) -> Result<usize> {
    let stored = super::query::list_for_event(tx, event_id)?;
    let mut removed = 0;

    for proof in stored {
        if remove(tx, event_id, &proof.recipient_pubkey)? {
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
    use crate::db::proof::query;
    use crate::fixtures::{author, id, note, peer};

    fn us() -> PublicKey {
        author(9)
    }

    fn store_note(tx: &Tx<'_>, content: &str) -> String {
        let event = note(author(1), 100, content, Tags::new());

        event_command::save(tx, &event, &peer(), 10).unwrap();

        id(&event)
    }

    fn proof(event_id: &str) -> Proof {
        Proof {
            event_id: event_id.to_string(),
            recipient_pubkey: us(),
            sig: [7u8; 64],
        }
    }

    #[test]
    fn a_signature_is_stored_once() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let event_id = store_note(&tx, "signed");

        assert!(save(&tx, &proof(&event_id)).unwrap());
        assert!(!save(&tx, &proof(&event_id)).unwrap());
        assert_eq!(query::list_for_event(&tx, &event_id).unwrap().len(), 1);
        assert!(query::exists(&tx, &event_id, &us()).unwrap());
        assert_eq!(
            query::get(&tx, &event_id, &us()).unwrap().unwrap(),
            proof(&event_id)
        );
    }

    #[test]
    fn a_signature_cannot_outlive_its_event() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let event_id = store_note(&tx, "signed");
        save(&tx, &proof(&event_id)).unwrap();

        event_command::delete(&tx, &event_id).unwrap();

        assert!(query::get(&tx, &event_id, &us()).unwrap().is_none());
    }

    #[test]
    fn a_signature_needs_an_event_to_name() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        assert!(save(&tx, &proof(&hex::encode([0u8; 32]))).is_err());
    }

    #[test]
    fn forwardable_answers_for_a_batch() {
        let mut connection = open_in_memory().unwrap();
        let tx = Tx::begin_write(&mut connection).unwrap();

        let signed = store_note(&tx, "signed");
        let unsigned = store_note(&tx, "unsigned");
        save(&tx, &proof(&signed)).unwrap();

        let ids = [signed.clone(), unsigned];

        assert_eq!(query::forwardable(&tx, &ids, &us()).unwrap(), [signed]);
        assert!(
            query::forwardable(&tx, &ids, &author(4))
                .unwrap()
                .is_empty()
        );
    }
}
