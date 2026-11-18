#!/bin/bash
# Builds the real FileTableView source into a tiny program and checks that
# draggingEnded: (sent by AppKit at the end of every drag) does not throw.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SRC="$HERE/../../RustyFinderApp/Sources/Pane"
OUT="$(mktemp -d)"
trap 'rm -rf "$OUT"' EXIT
clang -c -fobjc-arc -o "$OUT/TryObjC.o" "$HERE/TryObjC.m"
# DragTrace is optional diagnostics; stub it when checking in isolation.
printf 'enum DragTrace { static func note(_: @autoclosure () -> String) {}; static var buttons: String { "" } }\n' > "$OUT/stub.swift"
# Top-level code must live in main.swift when compiling several files.
cp "$HERE/check-dragging-ended.swift" "$OUT/main.swift"
swiftc -import-objc-header "$HERE/TryObjC.h" -o "$OUT/check" \
  "$OUT/main.swift" "$SRC/FileTableView.swift" "$OUT/stub.swift" "$OUT/TryObjC.o"
"$OUT/check"
