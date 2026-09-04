//! The nostr relay protocol, used unmodified as the peer protocol.
//!
//! Every device is a client and a relay at once, so both halves of NIP-01 move
//! in both directions over one link. A [`Req`](Message::Req) arriving is a
//! question for this device's relay half; an [`Event`](Message::Event)
//! arriving is an answer for its client half.
//!
//! `docs/sync.md`. The additions for a transport with no URL are in
//! `docs/nips/p2p-auth.md`.

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use coracle_lib::events::{Event, EventExtensionId, EventId, HashedEvent};
use coracle_lib::filters::Filter;
use serde_json::{Value, json};

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
    /// An event served on a subscription, the answer to a `REQ`. Relay →
    /// client
    Event(SubscriptionId, Box<HashedEvent>),
    /// An event published by a client, to be stored. Client → relay.
    Publish(Box<HashedEvent>),
    /// Every stored event matching a subscription has been sent.
    Eose(SubscriptionId),
    /// Whether an `EVENT` was accepted, and why not.
    Ok(String, bool, String),
    /// A NIP-42 challenge the peer must sign. Relay → client.
    /// `docs/privacy.md#the-auth-event-is-portable-evidence`.
    AuthChallenge(String),
    /// The kind 22242 event answering a challenge. Client → relay.
    AuthResponse(Box<Event>),
    /// The author's signature over an event, naming this device. The first
    /// hop carries it; it never travels a second. `docs/proofs.md`.
    RecipientSignature(SubscriptionId, EventId, Box<[u8; 64]>),
    /// A designated-verifier proof over an event. The second hop carries it
    /// instead of the signature it was built from.
    AuthorshipProof(SubscriptionId, EventId, Box<[u8; 160]>),
    /// Open a NIP-77 reconciliation over a filter.
    NegOpen(SubscriptionId, Filter, Vec<u8>),
    /// One round of it.
    NegMsg(SubscriptionId, Vec<u8>),
    /// End one.
    NegClose(SubscriptionId),
    /// A Blossom request, wrapped because there is no HTTP on a BLE link.
    /// `docs/sync.md#blob-sync`.
    BlossomRequest(Box<BlossomRequest>),
    /// A Blossom response.
    BlossomResponse(Box<BlossomResponse>),
}

/// A Blossom request, `["BLOSSOM-REQ", id, method, path, headers, body]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlossomRequest {
    /// Correlates the response with this request.
    pub id: String,
    /// `HEAD` to test for a blob and learn its length, `GET` to fetch a range.
    pub method: String,
    /// `/<sha256>`.
    pub path: String,
    /// Range and content headers, carrying the Bao slice bounds.
    pub headers: Vec<(String, String)>,
    /// The body for a `PUT`, empty otherwise.
    pub body: Vec<u8>,
}

/// A Blossom response, `["BLOSSOM-RES", id, status, headers, body]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlossomResponse {
    /// The request it answers.
    pub id: String,
    /// HTTP status: `200` when the whole blob is in the body, `206` for a
    /// range, `404` when this device does not hold it.
    pub status: u16,
    /// Content and range headers.
    pub headers: Vec<(String, String)>,
    /// The body for a `GET`, empty otherwise.
    pub body: Vec<u8>,
}

impl Message {
    /// Parse one message off the sync channel.
    ///
    /// A NIP-01 array whose first element names the message type. Frames are
    /// hex and bodies are base64, which is what the relay protocol moved to
    /// once it had to carry bytes that a WebSocket never has.
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let value: Value = serde_json::from_slice(payload).context("decoding a message")?;
        let array = value
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("not an array"))?;

        let mut items = array.iter();
        let kind = items
            .next()
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow::anyhow!("a message has no kind"))?;

        fn string<'a>(items: &mut impl Iterator<Item = &'a Value>) -> Result<String> {
            items
                .next()
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("expected a string"))
        }

        fn id<'a>(items: &mut impl Iterator<Item = &'a Value>) -> Result<SubscriptionId> {
            let value = items
                .next()
                .ok_or_else(|| anyhow::anyhow!("expected a subscription id"))?;

            value
                .as_str()
                .map(|string| SubscriptionId(string.to_string()))
                .ok_or_else(|| anyhow::anyhow!("subscription id is not a string"))
        }

        fn hex<'a>(items: &mut impl Iterator<Item = &'a Value>) -> Result<Vec<u8>> {
            let value = string(items)?;

            hex::decode(&value).context("a frame is hex")
        }

        fn headers<'a>(
            items: &mut impl Iterator<Item = &'a Value>,
        ) -> Result<Vec<(String, String)>> {
            let headers = items
                .next()
                .ok_or_else(|| anyhow::anyhow!("no headers"))?
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("headers are not an object"))?;

            headers
                .iter()
                .map(|(key, value)| {
                    value
                        .as_str()
                        .map(|value| (key.clone(), value.to_string()))
                        .ok_or_else(|| anyhow::anyhow!("a header is not a string"))
                })
                .collect()
        }

        fn body<'a>(items: &mut impl Iterator<Item = &'a Value>) -> Result<Vec<u8>> {
            base64::engine::general_purpose::STANDARD
                .decode(string(items)?)
                .context("decoding a body")
        }

        fn event(value: &Value) -> Result<HashedEvent> {
            let event: HashedEvent =
                serde_json::from_value(value.clone()).context("parsing EVENT")?;

            // Every authorization commits to the id rather than to the bytes,
            // so an id that is not its own hash is refused here. `proofs.md`.
            if !event.verify_id() {
                bail!("an event's id is not the hash of its content");
            }

            Ok(event)
        }

        match kind {
            "REQ" => {
                let subscription = id(&mut items)?;
                let filters = items
                    .map(|value| serde_json::from_value::<Filter>(value.clone()))
                    .collect::<serde_json::Result<Vec<_>>>()
                    .context("decoding REQ filters")?;

                Ok(Self::Req(subscription, filters))
            }
            "CLOSE" => Ok(Self::Close(id(&mut items)?)),
            "EVENT" => {
                // Two-element is a client publishing; three is a relay
                // serving it on a subscription.
                match items.next() {
                    Some(Value::String(subscription)) => {
                        let value = items
                            .next()
                            .ok_or_else(|| anyhow::anyhow!("EVENT has no event"))?;

                        Ok(Self::Event(
                            SubscriptionId(subscription.clone()),
                            Box::new(event(value)?),
                        ))
                    }
                    Some(value) => Ok(Self::Publish(Box::new(event(value)?))),
                    None => Err(anyhow::anyhow!("EVENT has no event")),
                }
            }
            "EOSE" => Ok(Self::Eose(id(&mut items)?)),
            "OK" => {
                let event_id = string(&mut items)?;
                let accepted = items
                    .next()
                    .and_then(Value::as_bool)
                    .ok_or_else(|| anyhow::anyhow!("OK has no verdict"))?;
                let message = string(&mut items)?;

                Ok(Self::Ok(event_id, accepted, message))
            }
            "AUTH" => {
                let value = items
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("AUTH has no payload"))?;

                if let Some(challenge) = value.as_str() {
                    Ok(Self::AuthChallenge(challenge.to_string()))
                } else {
                    let event =
                        serde_json::from_value::<Event>(value.clone()).context("parsing AUTH")?;

                    Ok(Self::AuthResponse(Box::new(event)))
                }
            }
            "NEG-OPEN" => {
                let subscription = id(&mut items)?;
                let filter = items
                    .next()
                    .map(|value| serde_json::from_value::<Filter>(value.clone()))
                    .transpose()?
                    .ok_or_else(|| anyhow::anyhow!("NEG-OPEN has no filter"))?;
                let frame = hex(&mut items)?;

                Ok(Self::NegOpen(subscription, filter, frame))
            }
            "RECIPIENT-SIGNATURE" => {
                let subscription = id(&mut items)?;
                let event_id = EventId::from_hex(&string(&mut items)?)
                    .context("a RECIPIENT-SIGNATURE has no valid event id")?;
                let sig = hex(&mut items)?;

                Ok(Self::RecipientSignature(
                    subscription,
                    event_id,
                    Box::new(
                        sig.try_into()
                            .map_err(|_| anyhow::anyhow!("a signature is not 64 bytes"))?,
                    ),
                ))
            }
            "AUTHORSHIP-PROOF" => {
                let subscription = id(&mut items)?;
                let event_id = EventId::from_hex(&string(&mut items)?)
                    .context("an AUTHORSHIP-PROOF has no valid event id")?;
                let proof = hex(&mut items)?;

                Ok(Self::AuthorshipProof(
                    subscription,
                    event_id,
                    Box::new(
                        proof
                            .try_into()
                            .map_err(|_| anyhow::anyhow!("a proof is not 160 bytes"))?,
                    ),
                ))
            }
            "NEG-MSG" => {
                let subscription = id(&mut items)?;
                let message = hex(&mut items)?;

                Ok(Self::NegMsg(subscription, message))
            }
            "NEG-CLOSE" => Ok(Self::NegClose(id(&mut items)?)),
            "BLOSSOM-REQ" => {
                let id = string(&mut items)?;
                let method = string(&mut items)?;
                let path = string(&mut items)?;

                Ok(Self::BlossomRequest(Box::new(BlossomRequest {
                    id,
                    method,
                    path,
                    headers: headers(&mut items)?,
                    body: body(&mut items)?,
                })))
            }
            "BLOSSOM-RES" => {
                let id = string(&mut items)?;
                let status: u16 = items
                    .next()
                    .and_then(Value::as_u64)
                    .and_then(|value| u16::try_from(value).ok())
                    .ok_or_else(|| anyhow::anyhow!("BLOSSOM-RES has no status"))?;

                Ok(Self::BlossomResponse(Box::new(BlossomResponse {
                    id,
                    status,
                    headers: headers(&mut items)?,
                    body: body(&mut items)?,
                })))
            }
            other => Err(anyhow::anyhow!("unknown message kind {other}")),
        }
    }

    /// Serialize one for the wire.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let hex = |bytes: &[u8]| hex::encode(bytes);
        let to_event = |event: &HashedEvent| serde_json::to_value(event).expect("event to JSON");
        let to_filter = |filter: &Filter| serde_json::to_value(filter).expect("filter to JSON");
        let header_map = |headers: &[(String, String)]| {
            headers
                .iter()
                .map(|(key, value)| (key.clone(), json!(value)))
                .collect::<serde_json::Map<_, _>>()
        };

        let bytes = |value: Value| serde_json::to_vec(&value).expect("a Value serializes");

        match self {
            Self::Req(subscription, filters) => {
                let mut array = vec![json!("REQ"), json!(&subscription.0)];
                array.extend(filters.iter().map(to_filter));

                bytes(json!(array))
            }
            Self::Close(subscription) => bytes(json!(["CLOSE", subscription.0])),
            Self::Event(subscription, event) => {
                bytes(json!(["EVENT", subscription.0, to_event(event)]))
            }
            Self::Publish(event) => bytes(json!(["EVENT", to_event(event)])),
            Self::Eose(subscription) => bytes(json!(["EOSE", subscription.0])),
            Self::Ok(id, accepted, message) => bytes(json!(["OK", id, accepted, message])),
            Self::AuthChallenge(challenge) => bytes(json!(["AUTH", challenge])),
            Self::AuthResponse(event) => bytes(json!([
                "AUTH",
                serde_json::to_value(event.as_ref()).unwrap()
            ])),
            Self::NegOpen(subscription, filter, frame) => bytes(json!([
                "NEG-OPEN",
                subscription.0.clone(),
                to_filter(filter),
                hex(frame)
            ])),
            Self::RecipientSignature(subscription, event_id, sig) => bytes(json!([
                "RECIPIENT-SIGNATURE",
                subscription.0.clone(),
                event_id.to_hex(),
                hex(&sig[..])
            ])),
            Self::AuthorshipProof(subscription, event_id, proof) => bytes(json!([
                "AUTHORSHIP-PROOF",
                subscription.0.clone(),
                event_id.to_hex(),
                hex(&proof[..])
            ])),
            Self::NegMsg(subscription, message) => {
                bytes(json!(["NEG-MSG", subscription.0.clone(), hex(message)]))
            }
            Self::NegClose(subscription) => bytes(json!(["NEG-CLOSE", subscription.0])),
            Self::BlossomRequest(payload) => bytes(json!([
                "BLOSSOM-REQ",
                payload.id.clone(),
                payload.method.clone(),
                payload.path.clone(),
                header_map(&payload.headers),
                b64(&payload.body),
            ])),
            Self::BlossomResponse(payload) => bytes(json!([
                "BLOSSOM-RES",
                payload.id.clone(),
                payload.status,
                header_map(&payload.headers),
                b64(&payload.body),
            ])),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coracle_lib::events::{EventContent, HashedEvent};
    use coracle_lib::keys::SecretKey;
    use coracle_lib::tags::Tags;

    fn note(content: &str) -> HashedEvent {
        EventContent::new()
            .with_content(content)
            .with_tags(Tags::new())
            .with_kind(1)
            .with_created_at(1_700_000_000)
            .with_pubkey(SecretKey::generate().public_key())
            .with_id()
    }

    fn round_trips(message: Message) {
        assert_eq!(Message::decode(&message.encode()).unwrap(), message);
    }

    #[test]
    fn messages_round_trip() {
        let note = note("hello");
        let filter = Filter::new().add_kinds([1]);

        round_trips(Message::Req(
            SubscriptionId("a".into()),
            vec![filter.clone()],
        ));
        round_trips(Message::Close(SubscriptionId("a".into())));
        round_trips(Message::Event(
            SubscriptionId("a".into()),
            Box::new(note.clone()),
        ));
        round_trips(Message::Publish(Box::new(note)));
        round_trips(Message::Eose(SubscriptionId("a".into())));
        round_trips(Message::Ok("id".into(), true, "ok".into()));
        round_trips(Message::AuthChallenge("challenge".into()));
        round_trips(Message::NegOpen(
            SubscriptionId("a".into()),
            filter,
            vec![1, 2, 3],
        ));
        round_trips(Message::NegMsg(SubscriptionId("a".into()), vec![4, 5, 6]));
        round_trips(Message::NegClose(SubscriptionId("a".into())));
        round_trips(Message::RecipientSignature(
            SubscriptionId("a".into()),
            EventId::new([1u8; 32]),
            Box::new([2u8; 64]),
        ));
        round_trips(Message::AuthorshipProof(
            SubscriptionId("a".into()),
            EventId::new([1u8; 32]),
            Box::new([3u8; 160]),
        ));

        let request = BlossomRequest {
            id: "id".into(),
            method: "HEAD".into(),
            path: "/sha256".into(),
            headers: vec![("range".into(), "0-".into())],
            body: vec![],
        };
        round_trips(Message::BlossomRequest(Box::new(request)));

        let response = BlossomResponse {
            id: "id".into(),
            status: 206,
            headers: vec![("content-range".into(), "bytes 0-3/4".into())],
            body: vec![0, 1, 2, 255],
        };
        round_trips(Message::BlossomResponse(Box::new(response)));
    }

    #[test]
    fn an_auth_response_round_trips_the_signed_event() {
        let secret = SecretKey::generate();
        let hashed = note("auth");
        let id = hashed.id;
        let signed = hashed.with_sig(secret.sign(id.as_bytes()));

        let message = Message::AuthResponse(Box::new(signed.clone()));

        match Message::decode(&message.encode()).unwrap() {
            Message::AuthResponse(decoded) => assert_eq!(*decoded, signed),
            other => panic!("expected a response, got {other:?}"),
        }
    }

    #[test]
    fn a_hopeless_payload_is_an_error() {
        assert!(Message::decode(b"[]").is_err());
        assert!(Message::decode(b"[1, 2]").is_err());
        assert!(Message::decode(b"[\"UNKNOWN\"]").is_err());
    }
}
