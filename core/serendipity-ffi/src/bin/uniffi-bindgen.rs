//! Binding generator, invoked by `just bindings`.
//!
//! uniffi wants the generator built against the same version the library uses,
//! so it ships as a bin in this crate rather than as an installed tool.

fn main() {
    uniffi::uniffi_bindgen_main();
}
