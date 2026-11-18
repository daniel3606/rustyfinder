#!/bin/bash
# Print the GitHub release notes for a version:
#
#   scripts/release-notes.sh <version> "<signing status>"
#
# docs/releases/<version>.md (what changed) comes first when it exists,
# then download, Homebrew, checksum and signing sections. Run from the
# repository root, next to dist/ (for the checksums).
set -euo pipefail

VERSION="$1"
SIGNING="${2:-ad-hoc signed, not notarized}"
BASE="RustyFinder-$VERSION-macOS-arm64"

if [ -f "docs/releases/$VERSION.md" ]; then
  cat "docs/releases/$VERSION.md"
  echo
fi

cat <<EOF
### Download

Apple silicon Macs, macOS 26 (Tahoe) or later.

- **\`$BASE.dmg\`**: open it and drag RustyFinder to Applications.
- \`$BASE.zip\`: the same app, zipped.

Or install with [Homebrew](https://brew.sh):

\`\`\`
brew install --cask daniel3606/tap/rustyfinder
\`\`\`

### Checksums (SHA-256)

\`\`\`
EOF
cat dist/SHA256SUMS.txt
cat <<EOF
\`\`\`

Check a download with \`shasum -a 256 -c SHA256SUMS.txt\` in the folder that holds it.

### Signing

EOF
case "$SIGNING" in
  "Developer ID signed and notarized")
    echo "Signed with a Developer ID and notarized by Apple; the ticket is stapled to the app and the DMG."
    ;;
  *)
    cat <<EOF
**This build is $SIGNING by Apple.** The first time you open it (and after
each upgrade) macOS refuses; allow it in System Settings › Privacy & Security
with "Open Anyway". Nothing in RustyFinder turns off Gatekeeper or other
protections. To build it yourself instead, clone the repository and run
\`scripts/install.sh\`.
EOF
    ;;
esac

cat <<EOF

### Provenance

Built by the [Release workflow](https://github.com/daniel3606/rustyfinder/blob/v$VERSION/.github/workflows/release.yml)
from the commit tagged \`v$VERSION\`, after \`cargo fmt --check\`, \`cargo clippy -D warnings\`,
the Rust test suite and the drag-end check passed there.
EOF
