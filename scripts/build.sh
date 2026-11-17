#!/bin/bash
# One command: Rust -> static lib -> XCFramework -> UniFFI Swift bindings ->
# Xcode project -> RustyFinder.app.
#
#   scripts/build.sh            release build, output in build/RustyFinder.app
#   scripts/build.sh --debug    debug Swift build (Rust stays release)
#   scripts/build.sh --check    also run clippy and the Rust tests first
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Keep this machine's folder names out of the binaries (Rust panic
# locations, Swift #file strings): sources appear as /rustyfinder/...,
# crates as /cargo/... A released app then names no user or folder.
CARGO_DIR="${CARGO_HOME:-$HOME/.cargo}"
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$ROOT=/rustyfinder --remap-path-prefix=$CARGO_DIR=/cargo --remap-path-prefix=$HOME=/home"

CONFIG=Release
CHECK=0
for arg in "$@"; do
  case "$arg" in
    --debug) CONFIG=Debug ;;
    --check) CHECK=1 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

BUILD="$ROOT/build"
LIB=librustyfinder_ffi.a
HOST="$(rustc -vV | sed -n 's/^host: //p')"

step() { printf '\n==> %s\n' "$*"; }

if [ "$CHECK" = 1 ]; then
  step "clippy + tests"
  cargo clippy --all-targets -- -D warnings
  cargo test --quiet -- --test-threads=4
fi

# Build every Apple target whose standard library is installed. Homebrew
# Rust ships only the host target, so the result is single-arch there;
# with rustup and both targets added it becomes a universal binary.
TARGETS=()
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  if [ -d "$(rustc --print target-libdir --target "$t" 2>/dev/null)" ]; then
    TARGETS+=("$t")
  fi
done
[ ${#TARGETS[@]} -gt 0 ] || { echo "no usable Rust target found" >&2; exit 1; }

LIBS=()
ARCHS=()
for t in "${TARGETS[@]}"; do
  step "cargo build ($t)"
  cargo build --release -p rustyfinder-ffi --target "$t"
  LIBS+=("target/$t/release/$LIB")
  case "$t" in
    aarch64-*) ARCHS+=(arm64) ;;
    x86_64-*) ARCHS+=(x86_64) ;;
  esac
done

step "lipo (${ARCHS[*]})"
mkdir -p "$BUILD/lib"
lipo -create "${LIBS[@]}" -output "$BUILD/lib/$LIB"
lipo -info "$BUILD/lib/$LIB"

step "UniFFI Swift bindings"
# The bindgen reads metadata from a dylib built for the machine running it.
cargo build --release -p rustyfinder-ffi -p uniffi-bindgen
rm -rf "$BUILD/bindings"
mkdir -p "$BUILD/bindings"
"target/release/uniffi-bindgen" generate \
  --library "target/release/librustyfinder_ffi.dylib" \
  --language swift --out-dir "$BUILD/bindings"

mkdir -p RustyFinderApp/Generated
cp "$BUILD/bindings/rustyfinder_ffi.swift" RustyFinderApp/Generated/

step "XCFramework"
rm -rf "$BUILD/headers" "$BUILD/RustyFinderFFI.xcframework"
mkdir -p "$BUILD/headers"
cp "$BUILD/bindings/rustyfinder_ffiFFI.h" "$BUILD/headers/"
# Clang only picks up a module map with this exact file name.
cp "$BUILD/bindings/rustyfinder_ffiFFI.modulemap" "$BUILD/headers/module.modulemap"
xcodebuild -create-xcframework \
  -library "$BUILD/lib/$LIB" -headers "$BUILD/headers" \
  -output "$BUILD/RustyFinderFFI.xcframework" >/dev/null

step "xcodegen"
(cd RustyFinderApp && xcodegen generate --quiet)

step "xcodebuild ($CONFIG)"
xcodebuild -project RustyFinderApp/RustyFinder.xcodeproj -scheme RustyFinder \
  -configuration "$CONFIG" -derivedDataPath "$BUILD/DerivedData" \
  ARCHS="${ARCHS[*]}" ONLY_ACTIVE_ARCH=NO \
  build -quiet

rm -rf "$BUILD/RustyFinder.app"
cp -R "$BUILD/DerivedData/Build/Products/$CONFIG/RustyFinder.app" "$BUILD/RustyFinder.app"
step "done: $BUILD/RustyFinder.app"
