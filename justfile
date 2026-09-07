# Dip — task runner. `just` with no arguments lists everything.
#
# Two halves, matching the plugin boundary in docs/overview.md: a TypeScript
# webview and a Rust core reached through uniffi. The core builds before the
# native shells, so `sync` chains them in that order and the mobile recipes go
# through `sync` rather than around it.

set shell := ["bash", "-euo", "pipefail", "-c"]

core := "core"
ffi := "dip_ffi"

# uniffi-bindgen reads the host build of the library, so the extension is the
# host's: a Mac generating the Swift half, Linux generating the Kotlin one.
cdylib := if os() == "macos" { "dylib" } else { "so" }

# Staging for everything generated. Under target/, so `cargo clean` and
# `just clean` both take it and nothing generated is ever committed.
out := "core/target/ffi"

# Where the Android shell picks its half up. Gitignored: what a native build
# links against is whatever the last `just sync` compiled, never a committed
# copy that has drifted from the source it was generated from. The iOS shell
# builds its own half — see scripts/xcode-prebuild.sh.
android_app := "android/app/src/main"

# The targets `just setup` installs. scripts/xcode-prebuild.sh chooses between
# them from Xcode's PLATFORM_NAME, and knows x86_64-apple-ios for an Intel
# simulator as well.
ios_device := "aarch64-apple-ios"
ios_sim := "aarch64-apple-ios-sim"
android_targets := "arm64-v8a armeabi-v7a x86_64"

[private]
default:
    @just --list --unsorted

# Install toolchains and dependencies. Run once after cloning.
setup:
    pnpm install --frozen-lockfile
    rustup target add {{ios_device}} {{ios_sim}}
    @echo
    @echo "Android needs a JDK and the SDK, which just qa compiles against, plus"
    @echo "the NDK and cargo-ndk for android-lib. None of them install here:"
    @echo "  Android Studio > SDK Manager, and SDK Tools > NDK (Side by side)"
    @echo "  cargo install cargo-ndk"

# ---------------------------------------------------------------- development

# Vite dev server. Browser only — no plugin, so no BLE, no store, no peers.
dev:
    pnpm exec vite

# Components are copied into the repo rather than depended on, so this is the
# only way one arrives. See docs/ui.md.

# Vendor shadcn-svelte components into src/lib/components/ui. No args to choose.
ui *components:
    pnpm dlx shadcn-svelte@latest add {{components}}

# Build web assets to dist/, which the native shells load.
build:
    pnpm exec vite build

# Serve the built output as the shells see it.
preview: build
    pnpm exec vite preview

# ----------------------------------------------------------------------- core

# Build the core for the host, for a fast inner loop.
core-build:
    cd {{core}} && cargo build --workspace

# Core tests.
core-test:
    cd {{core}} && cargo test --workspace

# rustfmt's opinion of the core, read rather than applied. `just fmt` applies it.
core-fmt:
    cd {{core}} && cargo fmt --check

# Clippy over the core and its tests, warnings denied.
core-lint:
    cd {{core}} && cargo clippy --workspace --all-targets -- -D warnings

# uniffi reads the compiled cdylib rather than the source, so the library is
# always built first. Stale bindings against a fresh library is the failure this
# ordering exists to prevent.

# Regenerate Swift and Kotlin bindings from the built library.
bindings:
    cd {{core}} && cargo build -p dip-ffi
    mkdir -p {{out}}/swift {{out}}/kotlin
    cd {{core}} && cargo run -q --bin uniffi-bindgen -- generate \
        --library target/debug/lib{{ffi}}.{{cdylib}} \
        --language swift --out-dir target/ffi/swift --no-format
    cd {{core}} && cargo run -q --bin uniffi-bindgen -- generate \
        --library target/debug/lib{{ffi}}.{{cdylib}} \
        --language kotlin --out-dir target/ffi/kotlin --no-format
    @echo "bindings → {{out}}/{swift,kotlin}"

# ------------------------------------------------------------- mobile binaries

# Put the generated Kotlin where the app module's source set looks for it.
android-bindings: bindings
    rm -rf {{android_app}}/uniffi
    mkdir -p {{android_app}}
    cp -R {{out}}/kotlin/uniffi {{android_app}}/uniffi

# Build Android jniLibs for every ABI. Needs the NDK and cargo-ndk — see `setup`.
android-lib: android-bindings
    @command -v cargo-ndk >/dev/null || { echo "cargo-ndk missing: cargo install cargo-ndk"; exit 1; }
    rm -rf {{out}}/jniLibs
    cd {{core}} && cargo ndk {{ prepend('-t ', android_targets) }} \
        -o target/ffi/jniLibs build -p dip-ffi --release
    rm -rf {{android_app}}/jniLibs
    cp -R {{out}}/jniLibs {{android_app}}/jniLibs
    @echo "jniLibs → {{android_app}}/jniLibs"

# ------------------------------------------------------------------ native app

# Run this rather than `cap sync`: ordering is the whole point, and skipping the
# first two steps leaves the Android shell linked against whatever was there
# before. The iOS half is Xcode's own first build phase, so nothing here stages
# it and working on iOS needs neither the NDK nor this recipe.

# Core, then bindings, then Android's library, then web assets, then Capacitor.
sync: android-lib build
    pnpm exec cap sync

# Open the iOS project. Opening ios/App/App.xcodeproj by hand is the same thing.
ios:
    pnpm exec cap open ios

# Build the Android half and open Android Studio.
android: android-lib build
    pnpm exec cap sync android
    pnpm exec cap open android

# -------------------------------------------------------------------------- qa

# Types across the webview.
check:
    pnpm exec svelte-check --tsconfig ./tsconfig.app.json
    pnpm exec tsc -p tsconfig.node.json

# Lint the webview: the UI conventions in docs/ui.md that a machine can check.
lint:
    pnpm exec eslint .

# Prettier's opinion of the webview. `just fmt` applies it instead.
format:
    pnpm exec prettier --check .

# The one-line comment rule in AGENTS.md, over both halves.
comments:
    node scripts/comments.js

# What the Xcode project references, which nothing but Xcode can otherwise read.
xcode:
    node scripts/xcode.js

# The names the Swift shell takes from the generated bindings. No compiler reads
# that Swift anywhere, so a stale one is found by running the app on a Mac.
swift: bindings
    node scripts/swift.js

# Kotlin resolves against the generated bindings, not the jniLibs, so this wants
# the Android SDK and not the NDK. The APK is thrown away; compiling it is the
# only thing that reads the shell's Kotlin at all.
android-check: android-bindings build
    pnpm exec cap sync android
    cd android && ./gradlew --no-daemon assembleDebug

# Format both halves in place.
fmt:
    pnpm exec prettier --write .
    pnpm exec eslint . --fix
    cd {{core}} && cargo fmt

# Everything a change has to pass. CI runs one step per recipe listed here except
# android-check: provisioning the SDK costs the shared runner more than the
# compile does, so the Kotlin is compiled before the push rather than after it.
qa: check lint format comments xcode core-fmt core-lint core-test swift android-check

# --------------------------------------------------------------------- cleanup

# Remove build output from both halves.
clean:
    rm -rf dist android/build android/app/build
    cd {{core}} && cargo clean
