//! Dip's core: everything other than UI and platform bindings.
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
