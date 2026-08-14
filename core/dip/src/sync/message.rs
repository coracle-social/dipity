//! The nostr relay protocol, used unmodified as the peer protocol.
//!
//! Every device is a client and a relay at once, so both halves of NIP-01 move
//! in both directions over one link. A [`Req`](Message::Req) arriving is a
//! question for this device's relay half; an [`Event`](Message::Event)
//! arriving is an answer for its client half.
//!
//! `docs/sync.md`. The additions for a transport with no URL are in
//! `docs/nips/p2p-auth.md`.

use anyhow::Result;
use coracle_lib::events::HashedEvent;
use coracle_lib::filters::Filter;

/// A subscription id, scoped to one link.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubscriptionId(pub String);

/// One message of the relay protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Open a subscription. Served by the relay half.
    Req(SubscriptionId, Vec<Filter>),
    /// Close one.
    Close(SubscriptionId),
    /// An event, in either direction: published by a client, or served by a
    /// relay answering a `REQ`.
    Event(Option<SubscriptionId>, Box<HashedEvent>),
    /// Every stored event matching a subscription has been sent.
    Eose(SubscriptionId),
    /// Whether an `EVENT` was accepted, and why not.
    Ok(String, bool, String),
    /// A NIP-42 challenge, or the kind 22242 event answering one.
    /// `docs/privacy.md#the-auth-event-is-portable-evidence`.
    Auth(Box<AuthPayload>),
    /// Open a NIP-77 reconciliation over a filter.
    NegOpen(SubscriptionId, Filter, Vec<u8>),
    /// One round of it.
    NegMsg(SubscriptionId, Vec<u8>),
    /// End one.
    NegClose(SubscriptionId),
    /// A Blossom request or response, wrapped because there is no HTTP on a BLE
    /// link. `docs/sync.md#blob-sync`.
    Blossom(Box<BlossomPayload>),
}

/// Either half of the NIP-42 exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthPayload {
    /// The challenge a peer must sign.
    Challenge(String),
    /// The kind 22242 event answering one.
    Response(Box<coracle_lib::events::Event>),
}

/// A Blossom request or response, `["BLOSSOM", id, method, path, headers, body]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlossomPayload {
    /// Correlates a response with its request.
    pub id: String,
    /// `HEAD` to test for a blob and learn its length, `GET` to fetch a range.
    pub method: String,
    /// `/<sha256>`.
    pub path: String,
    /// Range and content headers, carrying the Bao slice bounds.
    pub headers: Vec<(String, String)>,
    /// The body, if any.
    pub body: Vec<u8>,
}

impl Message {
    /// Parse one message off the sync channel.
    pub fn decode(_payload: &[u8]) -> Result<Self> {
        todo!("NIP-01 JSON array encoding")
    }

    /// Serialize one for the wire.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        todo!("NIP-01 JSON array encoding")
    }
}
