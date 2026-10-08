//! Binding generator, invoked by `just bindings`.
//!
//! The generator ships as a bin in this crate rather than as an installed tool,
//! because uniffi requires it built against the same version the library uses.

fn main() {
    uniffi::uniffi_bindgen_main();
}
