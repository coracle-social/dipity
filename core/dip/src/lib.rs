//! Dip's core: everything below the UI.
//!
//! The webview is suspended in the background, so anything that runs at
//! *encounter time* — a peer appearing while both phones are in pockets — lives
//! here rather than in TypeScript: the relay protocol both ways, Noise and
//! framing, signing, grants and grant proofs, reconciliation, the store, and
//! the session state machine. See `docs/overview.md`.
//!
//! Scaffolding only so far. Modules will mirror the design documents one to
//! one, so that a question about a module has a document that answers it.
//!
//! Platform bindings live in `dip-ffi`, kept separate so this crate
//! stays testable on the host with nothing but `cargo test`.

#![forbid(unsafe_code)]

/// The core's version, as compiled.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_reported() {
        assert!(!super::version().is_empty());
    }
}
