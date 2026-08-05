# Serendipity — task runner. `just` with no arguments lists everything.
#
# Two halves, matching the plugin boundary in docs/overview.md: a TypeScript
# webview and a Rust core reached through uniffi. The core builds before the
# native shells, so `sync` chains them in that order and the mobile recipes go
# through `sync` rather than around it.

set shell := ["bash", "-euo", "pipefail", "-c"]

core := "core"
ffi := "serendipity_ffi"

# Staging for everything generated. Under target/, so `cargo clean` and
# `just clean` both take it and nothing generated is ever committed.
out := "core/target/ffi"

# Simulator slices are arm64 only. Add x86_64-apple-ios here and to the lipo in
# `ios-lib` if an Intel Mac ever needs to run the simulator.
ios_device := "aarch64-apple-ios"
ios_sim := "aarch64-apple-ios-sim"
android_targets := "arm64-v8a armeabi-v7a x86_64"

[private]
default:
    @just --list --unsorted

# Install toolchains and dependencies. Run once after cloning.
setup:
    npm ci
    rustup target add {{ios_device}} {{ios_sim}}
    @echo
    @echo "Android needs the NDK and cargo-ndk, neither of which this installs:"
    @echo "  Android Studio > SDK Manager > SDK Tools > NDK (Side by side)"
    @echo "  cargo install cargo-ndk"

# ---------------------------------------------------------------- development

# Vite dev server. Browser only — no plugin, so no BLE, no store, no peers.
dev:
    npx vite

# Build web assets to dist/, which the native shells load.
build:
    npx vite build

# Serve the built output as the shells see it.
preview: build
    npx vite preview

# ----------------------------------------------------------------------- core

# Build the core for the host, for a fast inner loop.
core-build:
    cd {{core}} && cargo build --workspace

# Core tests.
core-test:
    cd {{core}} && cargo test --workspace

# uniffi reads the compiled cdylib rather than the source, so the library is
# always built first. Stale bindings against a fresh library is the failure this
# ordering exists to prevent.

# Regenerate Swift and Kotlin bindings from the built library.
bindings:
    cd {{core}} && cargo build -p serendipity-ffi
    mkdir -p {{out}}/swift {{out}}/kotlin
    cd {{core}} && cargo run -q --bin uniffi-bindgen -- generate \
        --library target/debug/lib{{ffi}}.dylib \
        --language swift --out-dir target/ffi/swift --no-format
    cd {{core}} && cargo run -q --bin uniffi-bindgen -- generate \
        --library target/debug/lib{{ffi}}.dylib \
        --language kotlin --out-dir target/ffi/kotlin --no-format
    @echo "bindings → {{out}}/{swift,kotlin}"

# ------------------------------------------------------------- mobile binaries

# Build the iOS XCFramework: device and simulator slices plus the module map.
ios-lib: bindings
    cd {{core}} && cargo build -p serendipity-ffi --release --target {{ios_device}}
    cd {{core}} && cargo build -p serendipity-ffi --release --target {{ios_sim}}
    rm -rf {{out}}/headers {{out}}/SerendipityFFI.xcframework
    mkdir -p {{out}}/headers
    cp {{out}}/swift/{{ffi}}FFI.h {{out}}/headers/
    cp {{out}}/swift/{{ffi}}FFI.modulemap {{out}}/headers/module.modulemap
    xcodebuild -create-xcframework \
        -library {{core}}/target/{{ios_device}}/release/lib{{ffi}}.a \
        -headers {{out}}/headers \
        -library {{core}}/target/{{ios_sim}}/release/lib{{ffi}}.a \
        -headers {{out}}/headers \
        -output {{out}}/SerendipityFFI.xcframework
    @echo "xcframework → {{out}}/SerendipityFFI.xcframework"

# Build Android jniLibs for every ABI. Needs the NDK and cargo-ndk — see `setup`.
android-lib: bindings
    @command -v cargo-ndk >/dev/null || { echo "cargo-ndk missing: cargo install cargo-ndk"; exit 1; }
    rm -rf {{out}}/jniLibs
    cd {{core}} && cargo ndk {{ prepend('-t ', android_targets) }} \
        -o target/ffi/jniLibs build -p serendipity-ffi --release
    @echo "jniLibs → {{out}}/jniLibs"

# ------------------------------------------------------------------ native app

# Run this rather than `cap sync`: ordering is the whole point, and skipping the
# first two steps leaves the shells linked against whatever was there before.

# Core, then bindings, then xcframework, then web assets, then Capacitor.
sync: ios-lib build
    npx cap sync

# Sync, then open Xcode.
ios: sync
    npx cap open ios

# Sync, then open Android Studio.
android: sync
    npx cap open android

# -------------------------------------------------------------------------- qa

# Types across the webview.
check:
    npx svelte-check --tsconfig ./tsconfig.app.json
    npx tsc -p tsconfig.node.json

# Format the core in place.
fmt:
    cd {{core}} && cargo fmt

# Everything a change has to pass. What CI runs.
qa: check
    cd {{core}} && cargo fmt --check
    cd {{core}} && cargo clippy --workspace --all-targets -- -D warnings
    cd {{core}} && cargo test --workspace

# --------------------------------------------------------------------- cleanup

# Remove build output from both halves.
clean:
    rm -rf dist
    cd {{core}} && cargo clean
