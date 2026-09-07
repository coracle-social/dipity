//! The core's only surface to Swift and Kotlin.
//!
//! Everything crossing this boundary is declared here by hand. That is the
//! point of keeping it in its own crate: the FFI surface stays small and
//! deliberate, and [`dip`] stays free of binding machinery.
//!
//! One module per thing that crosses: [`node`] drives the radio loop,
//! [`keys`] is the Keychain the core reads the identity out of, [`store`] is
//! the store both halves of the app hold, and [`logging`] is where the core
//! writes its log.

pub mod keys;
pub mod logging;
pub mod node;
pub mod store;

uniffi::setup_scaffolding!();

/// The core's version. Used by the plugin to check it loaded the library it
/// expected rather than a stale one from a previous build.
#[uniffi::export]
#[must_use]
pub fn core_version() -> String {
    dip::version().to_owned()
}
