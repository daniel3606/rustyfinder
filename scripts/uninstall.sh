#!/bin/bash
# Remove RustyFinder.
#
#   scripts/uninstall.sh          remove the app (from /Applications and ~/Applications)
#   scripts/uninstall.sh --purge  also move its settings, favorites and saved
#                                 session to the Trash (asks first)
#
# Passwords saved for servers stay in your login keychain under
# "RustyFinder"; remove them in Keychain Access if you want them gone.
set -euo pipefail

PURGE=0
for arg in "$@"; do
  case "$arg" in
    --purge) PURGE=1 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

if pgrep -xq RustyFinder; then
  echo "RustyFinder is running. Quit it first." >&2
  exit 1
fi

# Everything goes to the Trash, so a mistake can be undone.
trash() {
  local name; name="$(basename "$1")"
  # A same-named item already in the Trash keeps its place.
  [ -e "$HOME/.Trash/$name" ] && name="$name $(date +%H.%M.%S)"
  mv "$1" "$HOME/.Trash/$name"
}

for app in /Applications/RustyFinder.app "$HOME/Applications/RustyFinder.app"; do
  if [ -d "$app" ]; then trash "$app" && echo "Moved $app to the Trash"; fi
done

if [ "$PURGE" = 1 ]; then
  SUPPORT="$HOME/Library/Application Support/RustyFinder"
  PREFS="$HOME/Library/Preferences/dev.rustyfinder.RustyFinder.plist"
  echo "This moves to the Trash:"
  for p in "$SUPPORT" "$PREFS"; do [ -e "$p" ] && echo "  $p"; done
  read -r -p "Continue? [y/N] " answer
  if [ "$answer" = y ] || [ "$answer" = Y ]; then
    for p in "$SUPPORT" "$PREFS"; do [ -e "$p" ] && trash "$p" && echo "Moved $p to the Trash"; done
    defaults delete dev.rustyfinder.RustyFinder >/dev/null 2>&1 || true
  fi
fi
