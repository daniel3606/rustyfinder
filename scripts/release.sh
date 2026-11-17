#!/bin/bash
# Build a Release RustyFinder.app and pack it for a GitHub release:
# dist/RustyFinder-<version>-macOS-<arch>.zip and its SHA-256.
#
# The app is ad-hoc signed and not notarized. People who download it will
# have to allow it in System Settings › Privacy & Security the first time.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
scripts/build.sh --check
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' build/RustyFinder.app/Contents/Info.plist)"
ARCH="$(lipo -archs build/RustyFinder.app/Contents/MacOS/RustyFinder | tr ' ' '+')"
mkdir -p dist
ZIP="dist/RustyFinder-$VERSION-macOS-$ARCH.zip"
rm -f "$ZIP"
ditto -c -k --keepParent build/RustyFinder.app "$ZIP"
shasum -a 256 "$ZIP" | tee "$ZIP.sha256"
echo "Packed $ZIP"
