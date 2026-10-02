#!/usr/bin/env bash
#
# Everything an iOS build needs that Xcode does not build itself: the Rust core
# for the platform being built, the uniffi bindings that name it, and the web
# assets the webview loads. The App target runs this as its first build phase,
# so opening the project is the whole setup — see
# core/README.md#the-xcode-project.
#
# Xcode's environment is the input: PLATFORM_NAME and ARCHS say what to
# cross-compile for. Run it by hand and it builds for an arm64 device.

set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
generated="$root/ios/App/App/Generated"

# A build phase inherits none of a login shell's PATH, and node and pnpm live
# wherever nvm, Volta, mise or pnpm's own installer put them, which only the
# user's shell knows. So the shell is asked, and the usual places follow it.
login_path=$("${SHELL:-/bin/zsh}" -lic 'printf "\n__PATH__%s\n" "$PATH"' </dev/null 2>/dev/null |
    sed -n 's/^__PATH__//p' | tail -n 1) || true
newest_nvm=$(ls -d "$HOME"/.nvm/versions/node/*/bin 2>/dev/null | sort -V | tail -n 1) || true
pnpm_home=${PNPM_HOME:-$HOME/Library/pnpm}

export PATH="${login_path:+$login_path:}$HOME/.cargo/bin:$pnpm_home:$pnpm_home/bin:${newest_nvm:+$newest_nvm:}$HOME/.volta/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

fail() {
    echo "error: $*" >&2
    exit 1
}

command -v cargo >/dev/null || fail "cargo is not on the build phase's PATH — install Rust from https://rustup.rs"
command -v pnpm >/dev/null || fail "pnpm is not on the build phase's PATH — install Node and corepack enable"
[ -d "$root/node_modules" ] || fail "no node_modules — run just setup"

# Xcode's indexer runs build phases too, and cross-compiling the core for it
# would double the cost of every edit.
if [ "${ACTION:-}" = "indexbuild" ] && [ -f "$generated/dip_ffi.swift" ]; then
    exit 0
fi

targets=()

for arch in ${ARCHS:-arm64}; do
    case "${PLATFORM_NAME:-iphoneos}/$arch" in
        iphoneos/arm64) targets+=(aarch64-apple-ios) ;;
        iphonesimulator/arm64) targets+=(aarch64-apple-ios-sim) ;;
        iphonesimulator/x86_64) targets+=(x86_64-apple-ios) ;;
        *) fail "no Rust target for ${PLATFORM_NAME:-iphoneos} $arch" ;;
    esac
done

# Release whatever the configuration is: the core is crypto, and a debug build
# of it is too slow to use on a phone.
libraries=()

for target in "${targets[@]}"; do
    (cd "$root/core" && cargo build -p dip-ffi --release --target "$target")
    libraries+=("$root/core/target/$target/release/libdip_ffi.a")
done

# Everything is generated beside Generated and moved in finished. Xcode's
# indexer runs this phase alongside the build, and a second module map left in
# Generated for even a moment is a redefinition of the module to the compiler.
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT

lipo -create "${libraries[@]}" -output "$staging/libdip_ffi.a"

# uniffi reads the compiled library rather than the source, so the bindings
# cannot describe anything but what was just built.
(cd "$root/core" && cargo run -q --bin uniffi-bindgen -- generate \
    --library "${libraries[0]}" --language swift --out-dir "$staging" --no-format)

# Swift finds a clang module by searching a directory for this one name.
mv "$staging/dip_ffiFFI.modulemap" "$staging/module.modulemap"

mkdir -p "$generated"
rm -f "$generated/dip_ffiFFI.modulemap"

# Only what changed is replaced, so an unchanged core does not send Xcode back to recompile.
for file in libdip_ffi.a dip_ffi.swift dip_ffiFFI.h module.modulemap; do
    cmp -s "$staging/$file" "$generated/$file" || mv -f "$staging/$file" "$generated/$file"
done

cd "$root"
pnpm exec vite build
pnpm exec cap copy ios
