//! The types the store is expressed in.
//!
//! One file per type, and every type re-exported here, so a caller writes
//! `crate::model::Policy` and where a type is defined stays an implementation
//! detail. It has to be organized this way round because the reads and writes
//! are organized the other: [`crate::db`] groups by table, and what a type
//! means does not have to line up with which table holds it — [`Policy`] is
//! assembled from `pref` and from events, and [`Query`] carries a constraint
//! from each of three.
//!
//! Nostr's own types are `coracle-lib`'s, not this crate's: events, keys, tags,
//! kinds, addresses and filters. They have to agree byte for byte with a peer
//! running the other platform's build, so there is one definition of each and
//! it is not here. What is here is what the store adds — where an event came
//! from ([`Provenance`], [`ProvenanceFilter`]), how far it may travel
//! ([`Register`]), who may be served it ([`Policy`]), and the metadata for the
//! files it references ([`Blob`]).

mod authors;
mod blob;
mod graph;
mod kind;
mod order;
mod policy;
mod pref;
mod provenance;
mod query;
mod recipient_signature;
mod registers;
mod scope;
mod tag;
mod visibility;

pub use authors::Authors;
pub use blob::{Blob, BlobRole};
pub use graph::{Graph, Standing};
pub use kind::{KIND_DELETE, KIND_MUTE, KIND_PROFILE};
pub use order::Order;
pub use policy::{DEFAULT_COOL_OFF_MINUTES, DEFAULT_DISCLOSURE_BUDGET, PeerPolicy, Policy};
pub use pref::{Pref, keys};
pub use provenance::{Provenance, ProvenanceFilter};
pub use query::Query;
pub use recipient_signature::RecipientSignature;
pub use registers::{Register, Registers};
pub use scope::Scope;
pub use tag::is_indexed_tag;
pub use visibility::{Visibility, VisibilityRule};

#[cfg(test)]
mod tests {
    use coracle_lib::prelude::*;
    use coracle_lib::tags::Tags;

    use crate::fixtures::{author, event};

    #[test]
    fn addresses_come_from_the_library() {
        let pubkey = author(1).to_hex();

        assert!(
            event(author(1), 1, 100, "", Tags::new())
                .address()
                .is_none()
        );
        assert!(
            event(author(1), 22_242, 100, "", Tags::new())
                .address()
                .is_none()
        );
        assert_eq!(
            event(author(1), 0, 100, "", Tags::new())
                .address()
                .unwrap()
                .to_string(),
            format!("0:{pubkey}:")
        );
        assert_eq!(
            event(author(1), 30_023, 100, "", Tags::new().add("d", ["post"]))
                .address()
                .unwrap()
                .to_string(),
            format!("30023:{pubkey}:post")
        );
        // A d tag on a plain replaceable kind is not part of its address.
        assert_eq!(
            event(
                author(1),
                10_002,
                100,
                "",
                Tags::new().add("d", ["ignored"])
            )
            .address()
            .unwrap()
            .to_string(),
            format!("10002:{pubkey}:")
        );
    }
}
