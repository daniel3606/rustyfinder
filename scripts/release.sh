#!/bin/bash
# Build, test and pack a release locally: the same steps as the Release
# workflow (.github/workflows/release.yml) without publishing anything.
#
#   scripts/release.sh     -> dist/RustyFinder-<version>-macOS-arm64.{dmg,zip}
#                             plus .sha256 files and SHA256SUMS.txt
#
# Without Developer ID credentials in the environment (see scripts/package.sh)
# the app is ad-hoc signed and not notarized.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
cargo fmt --all --check
scripts/build.sh --check
scripts/package.sh
