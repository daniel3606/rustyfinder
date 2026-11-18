#!/bin/bash
# Reproduce the "system input locked after drag-out" bug with a watchdog.
#
#   scripts/dnd-diag/repro.sh OUTDIR [APP]
#
# Launches APP (default build/RustyFinder.app) with drag tracing on, showing
# a folder of two test images, and records the trace, the system input state
# (rf-diag) and the window server log. If a drag starts and has not ended
# 30 seconds later (the input-lock signature) it samples the app, saves the
# logs and kills it, so the machine always comes back. Otherwise it keeps
# recording until the app quits or 10 minutes pass. Results land in OUTDIR.
set -uo pipefail

OUT="${1:?usage: repro.sh OUTDIR [APP]}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
APP="${2:-$ROOT/build/RustyFinder.app}"
HERE="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$OUT"
rm -f "$OUT"/*.txt

pkill -x RustyFinder && while pgrep -x RustyFinder >/dev/null; do sleep 0.2; done

swiftc -O -o "$OUT/rf-diag" "$HERE/rf-diag.swift" 2>/dev/null || { echo "rf-diag build failed" >&2; exit 1; }

IMAGES="$OUT/images"
mkdir -p "$IMAGES"
for name in one two; do
  [ -f "$IMAGES/$name.png" ] || sips -s format png \
    /System/Library/CoreServices/CoreTypes.bundle/Contents/Resources/GenericDocumentIcon.icns \
    --out "$IMAGES/$name.png" >/dev/null
done

/usr/bin/log stream --level info --style compact --predicate 'subsystem == "dev.rustyfinder.dragtrace"' > "$OUT/trace.txt" 2>&1 &
LOGPID=$!
"$OUT/rf-diag" RustyFinder 900 > "$OUT/diag.txt" 2>&1 &
DIAGPID=$!

RF_DRAG_TRACE=1 "$APP/Contents/MacOS/RustyFinder" "$IMAGES" > "$OUT/app-stdout.txt" 2>&1 &
APPPID=$!
echo "app pid $APPPID; waiting for a drag (10 min max)"

collect() {
  echo "collecting: $1"
  echo "$1" > "$OUT/reason.txt"
  sample "$APPPID" 5 -file "$OUT/rf_sample.txt" >/dev/null 2>&1
  /usr/bin/log show --last 3m --info --style compact \
    --predicate 'process == "RustyFinder" OR process == "WindowServer" OR process == "Dock" OR subsystem == "dev.rustyfinder.dragtrace"' \
    > "$OUT/rf_log.txt" 2>&1
  ioreg -l -w 0 | grep -i SecureInput > "$OUT/rf_secure.txt"
  lsappinfo info -all RustyFinder > "$OUT/rf_appinfo.txt" 2>&1
  kill "$APPPID" 2>/dev/null; pkill -x RustyFinder
  sleep 1
  kill "$LOGPID" "$DIAGPID" 2>/dev/null
}

# Watchdog: a drag that began but has not ended after 30 s is the lock.
# Otherwise keep recording (several drags are fine) until the app quits or
# 10 minutes pass.
deadline=$((SECONDS + 600))
pending=""
b=0; e=0
while kill -0 "$APPPID" 2>/dev/null; do
  b=$(grep -c "drag begin" "$OUT/trace.txt" 2>/dev/null); e=$(grep -c "drag END" "$OUT/trace.txt" 2>/dev/null)
  if [ "$b" -gt "$e" ]; then
    [ -n "$pending" ] || { pending=$SECONDS; echo "drag $b started"; }
    if [ $((SECONDS - pending)) -ge 30 ]; then collect "drag never ended: begins=$b ends=$e"; exit 2; fi
  elif [ -n "$pending" ]; then
    echo "drag $b ended after $((SECONDS - pending)) s"; pending=""
  fi
  [ $SECONDS -lt $deadline ] || { collect "time limit: begins=$b ends=$e"; exit 0; }
  sleep 0.5
done
collect "app quit: begins=$b ends=$e"
echo "done: $OUT"
