#!/bin/bash
# Build RustyFinder in Release and install it.
#
#   scripts/install.sh            build, then install into /Applications
#   scripts/install.sh --user     install into ~/Applications instead
#   scripts/install.sh --no-build install what build/RustyFinder.app holds
#
# The app is ad-hoc signed (no Developer ID): macOS treats it as an app you
# built yourself. Nothing here changes Gatekeeper or other security settings.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST=/Applications
BUILD=1
for arg in "$@"; do
  case "$arg" in
    --user) DEST="$HOME/Applications" ;;
    --no-build) BUILD=0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

if [ "$BUILD" = 1 ]; then
  "$ROOT/scripts/build.sh"
fi
APP="$ROOT/build/RustyFinder.app"
[ -d "$APP" ] || { echo "build/RustyFinder.app is missing: run scripts/build.sh" >&2; exit 1; }

if pgrep -xq RustyFinder; then
  echo "RustyFinder is running. Quit it first, then run this again." >&2
  exit 1
fi

mkdir -p "$DEST"
if [ ! -w "$DEST" ]; then
  echo "$DEST is not writable by you. Try: scripts/install.sh --user" >&2
  exit 1
fi

# Replace the old copy in one step: copy next to it, then swap.
TMP="$DEST/.RustyFinder.app.installing"
rm -rf "$TMP"
ditto "$APP" "$TMP"
rm -rf "$DEST/RustyFinder.app"
mv "$TMP" "$DEST/RustyFinder.app"
codesign --verify --deep "$DEST/RustyFinder.app"
echo "Installed $DEST/RustyFinder.app"
echo "Open it with: open \"$DEST/RustyFinder.app\""
