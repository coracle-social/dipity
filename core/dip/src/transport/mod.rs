//! The link: framing over one GATT characteristic, and the Noise session that
//! encrypts it. `docs/transport.md`.
//!
//! Both halves are core-side because both have to agree byte for byte with a
//! peer running the other platform's build. The shell owns the radio, the MTU
//! and the ATT queue, and moves bytes on and off the characteristic.
//!
//! | Module | Holds |
//! | --- | --- |
//! | [`frame`] | The codec: channels, fragmentation, reassembly, priority |
//! | [`noise`] | Noise XX, and the transport state a completed handshake leaves |
//! | [`wire`] | The two composed: the encrypted pipe a session talks through |

pub mod frame;
pub mod noise;
pub mod wire;

pub use frame::{Channel, Codec, Fragment, Frame, Outbox, Secrecy};
pub use noise::Noise;
pub use wire::Wire;
