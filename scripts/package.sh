#!/bin/bash
# Pack build/RustyFinder.app for a release:
#
#   dist/RustyFinder-<version>-macOS-arm64.dmg   drag-to-Applications installer
#   dist/RustyFinder-<version>-macOS-arm64.zip   the same app, zipped
#   dist/<each>.sha256 and dist/SHA256SUMS.txt   checksums by bare file name
#
# Signing depends on what the environment provides:
#
#   nothing                        the app stays ad-hoc signed, not notarized
#   DEVELOPER_ID_APPLICATION       "Developer ID Application: Name (TEAMID)" or
#                                  its SHA-1: app and DMG are signed with the
#                                  hardened runtime and a secure timestamp
#   ...and NOTARY_KEYCHAIN_PROFILE a `notarytool store-credentials` profile, or
#   ...and NOTARY_API_KEY_PATH,    an App Store Connect API key (.p8) with its
#          NOTARY_API_KEY_ID,      key ID and issuer ID: app and DMG are also
#          NOTARY_API_ISSUER       notarized and the tickets stapled
#
# Credentials are read from the environment or the keychain and never printed.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
APP=build/RustyFinder.app
DIST=dist
WORK=build/package

step() { printf '\n==> %s\n' "$*"; }
die() { echo "error: $*" >&2; exit 1; }

[ -d "$APP" ] || die "$APP is missing: run scripts/build.sh first"
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")"
ARCHS="$(lipo -archs "$APP/Contents/MacOS/RustyFinder")"
[ "$ARCHS" = arm64 ] || die "expected an arm64-only app, got: $ARCHS"
BASE="RustyFinder-$VERSION-macOS-arm64"

NOTARIZE=0
if [ -n "${DEVELOPER_ID_APPLICATION:-}" ]; then
  if [ -n "${NOTARY_KEYCHAIN_PROFILE:-}" ]; then
    NOTARY_ARGS=(--keychain-profile "$NOTARY_KEYCHAIN_PROFILE")
    NOTARIZE=1
  elif [ -n "${NOTARY_API_KEY_PATH:-}" ]; then
    : "${NOTARY_API_KEY_ID:?NOTARY_API_KEY_ID is required with NOTARY_API_KEY_PATH}"
    : "${NOTARY_API_ISSUER:?NOTARY_API_ISSUER is required with NOTARY_API_KEY_PATH}"
    NOTARY_ARGS=(--key "$NOTARY_API_KEY_PATH" --key-id "$NOTARY_API_KEY_ID" --issuer "$NOTARY_API_ISSUER")
    NOTARIZE=1
  fi
fi

notarize() {
  step "notarize $(basename "$1")"
  xcrun notarytool submit "$1" "${NOTARY_ARGS[@]}" --wait --timeout 30m
}

rm -rf "$WORK"
mkdir -p "$WORK" "$DIST"
STAGED="$WORK/RustyFinder.app"
ditto "$APP" "$STAGED"

if [ -n "${DEVELOPER_ID_APPLICATION:-}" ]; then
  step "sign with Developer ID (hardened runtime)"
  codesign --force --timestamp --options runtime \
    --entitlements packaging/entitlements.plist \
    --sign "$DEVELOPER_ID_APPLICATION" "$STAGED"
  if [ "$NOTARIZE" = 1 ]; then
    ditto -c -k --keepParent "$STAGED" "$WORK/notarize-app.zip"
    notarize "$WORK/notarize-app.zip"
    xcrun stapler staple "$STAGED"
    xcrun stapler validate "$STAGED"
  fi
else
  echo "note: DEVELOPER_ID_APPLICATION is not set; the app stays ad-hoc signed and is not notarized"
fi
codesign --verify --strict --deep "$STAGED"

step "zip"
ZIP="$DIST/$BASE.zip"
rm -f "$ZIP"
# ditto keeps the bundle's symlinks, extended attributes and stapled ticket.
ditto -c -k --keepParent "$STAGED" "$ZIP"

step "dmg"
DMG="$DIST/$BASE.dmg"
rm -f "$DMG"
swift packaging/dmg/make-background.swift "$WORK/dmg-background"
if ! python3 -c 'import dmgbuild' 2>/dev/null; then
  python3 -m venv "$WORK/venv"
  "$WORK/venv/bin/pip" install --quiet --disable-pip-version-check \
    --require-hashes --only-binary :all: -r packaging/dmg/requirements.txt
  PY="$WORK/venv/bin/python3"
else
  PY=python3
fi
"$PY" -m dmgbuild -s packaging/dmg/settings.py \
  -D app="$STAGED" -D background="$WORK/dmg-background/background.png" \
  "RustyFinder $VERSION" "$DMG"
if [ -n "${DEVELOPER_ID_APPLICATION:-}" ]; then
  codesign --force --timestamp --sign "$DEVELOPER_ID_APPLICATION" "$DMG"
  if [ "$NOTARIZE" = 1 ]; then
    notarize "$DMG"
    xcrun stapler staple "$DMG"
    xcrun stapler validate "$DMG"
  fi
fi
hdiutil verify -quiet "$DMG"

step "check the packed app"
MNT="$(mktemp -d)"
hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$MNT" "$DMG" >/dev/null
trap 'hdiutil detach -quiet "$MNT" 2>/dev/null || true' EXIT
[ -L "$MNT/Applications" ] || die "the DMG has no Applications link"
codesign --verify --strict --deep "$MNT/RustyFinder.app"
cmp -s "$MNT/RustyFinder.app/Contents/MacOS/RustyFinder" "$STAGED/Contents/MacOS/RustyFinder" \
  || die "the DMG's app differs from the staged app"
hdiutil detach -quiet "$MNT"
trap - EXIT
UNZIPPED="$WORK/unzipped"
ditto -x -k "$ZIP" "$UNZIPPED"
codesign --verify --strict --deep "$UNZIPPED/RustyFinder.app"

step "checksums"
(
  cd "$DIST"
  for f in "$BASE.dmg" "$BASE.zip"; do shasum -a 256 "$f" > "$f.sha256"; done
  cat "$BASE.dmg.sha256" "$BASE.zip.sha256" > SHA256SUMS.txt
  shasum -a 256 -c SHA256SUMS.txt
)

if [ "$NOTARIZE" = 1 ]; then
  SIGNING="Developer ID signed and notarized"
elif [ -n "${DEVELOPER_ID_APPLICATION:-}" ]; then
  SIGNING="Developer ID signed, not notarized"
else
  SIGNING="ad-hoc signed, not notarized"
fi
echo "$SIGNING" > "$WORK/signing.txt"
step "done ($SIGNING): $DMG, $ZIP"
