#!/bin/bash
# Drives the built app with scripted key presses and checks what it shows.
# Runs in-process (see DebugSnapshot.swift): no Accessibility permission, and
# it works with the display asleep. Needs build/RustyFinder.app (scripts/build.sh).
#
# Every check gets a fresh support folder and a scratch Trash, so nothing
# here touches the real settings, saved session or Trash. Spec test IDs are
# noted where a check covers one.
#
#   scripts/ui-smoke.sh           run everything, 4 app instances at a time
#   scripts/ui-smoke.sh ops:      run only checks whose name contains "ops:"
#   JOBS=1 scripts/ui-smoke.sh    run one at a time
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/build/RustyFinder.app/Contents/MacOS/RustyFinder"

# Checks are independent, so they are split over JOBS shards that run side
# by side, each in its own scratch folder. A shard is this same script with
# SHARD=<index>/<count>.
JOBS="${JOBS:-4}"
if [ -z "${SHARD:-}" ] && [ "$JOBS" -gt 1 ]; then
  [ -x "$APP" ] || { echo "build/RustyFinder.app missing: run scripts/build.sh" >&2; exit 2; }
  logs=()
  for i in $(seq 0 $((JOBS - 1))); do
    log="$(mktemp)"
    logs+=("$log")
    SHARD="$i/$JOBS" "$0" "$@" >"$log" 2>&1 &
  done
  wait
  cat "${logs[@]}" | grep -vE '^[0-9]+ passed, |^$'
  passed=$(cat "${logs[@]}" | sed -n 's/^\([0-9]*\) passed, .*/\1/p' | paste -sd+ - | bc)
  failed=$(cat "${logs[@]}" | grep -c '^FAIL')
  rm -f "${logs[@]}"
  echo
  echo "$passed passed, $failed failed"
  [ "$failed" = 0 ]
  exit
fi
SHARD_INDEX="${SHARD%%/*}"; SHARD_INDEX="${SHARD_INDEX:-0}"
SHARD_COUNT="${SHARD##*/}"; SHARD_COUNT="${SHARD_COUNT:-1}"
BASE="$ROOT/.testdata/ui-smoke-$SHARD_INDEX"
DATA="$BASE/left"      # left pane (given on the command line)
START="$BASE/right"    # starting directory, so the right pane opens here
export RUSTYFINDER_SUPPORT_DIR="$BASE/support"
export RUSTYFINDER_TRASH_DIR="$BASE/trash"
ONLY="${1:-}"
[ -x "$APP" ] || { echo "build/RustyFinder.app missing: run scripts/build.sh" >&2; exit 2; }

fixture() {
  rm -rf "$BASE"
  mkdir -p "$DATA/Folder B/deep" "$DATA/folder a" "$START/Folder B" "$START/zzz" "$RUSTYFINDER_SUPPORT_DIR" "$RUSTYFINDER_TRASH_DIR"
  touch "$DATA/file10.txt" "$DATA/file2.txt" "$DATA/File1.txt" "$DATA/.hidden" \
        "$DATA/Folder B/inside.md" "$DATA/Folder B/zeta.png" "$START/r.txt"
  printf 'hello' > "$DATA/five.bytes"
  ln -s missing "$DATA/dangling"
  # Sounds off: these checks run unattended, often at night.
  printf '{"starting_directory":"%s","play_sounds":false}' "$START" > "$RUSTYFINDER_SUPPORT_DIR/settings.json"
}

fail=0
pass=0
# run <keys> [app args...]: launch, press keys, print the state dump.
run() {
  local keys="$1"; shift
  # perl alarm: a hung launch fails the check instead of stalling the run.
  perl -e 'alarm 60; exec @ARGV' "$APP" "$@" --keys "$keys" --dump 2>&1
}
# expect <name> <dump> <expected line>...
expect() {
  local name="$1" out="$2"; shift 2
  for want in "$@"; do
    # A leading "~" means "some line contains this"; otherwise whole-line match.
    local mode=-qxF
    if [ "${want:0:1}" = "~" ]; then mode=-qF; want="${want:1}"; fi
    if ! grep $mode -- "$want" <<<"$out"; then
      echo "FAIL  $name: missing line: $want"
      sed 's/^/        /' <<<"$out"
      fail=1
      return
    fi
  done
  pass=$((pass + 1))
  echo "ok    $name"
}
# wanted <name>: true if this shard should run the check. Checks are dealt
# round-robin over the shards. Those that use the clipboard all go to shard
# 0: the pasteboard is shared by every app instance.
counter=0
wanted() {
  if [ -n "$ONLY" ] && [[ "$1" != *"$ONLY"* ]]; then return 1; fi
  if [[ "$1" == *clipboard* ]]; then
    [ "$SHARD_INDEX" = 0 ]
    return
  fi
  counter=$((counter + 1))
  [ $((counter % SHARD_COUNT)) = "$SHARD_INDEX" ]
}
# check <name> <keys> <expected line>...: fresh fixture, left pane at $DATA.
check() {
  local name="$1" keys="$2"; shift 2
  wanted "$name" || return 0
  fixture
  expect "$name" "$(run "$keys" "$DATA")" "$@"
}
# refute <name> <dump> <line>...: none of these lines may appear.
refute() {
  local name="$1" out="$2"; shift 2
  for unwanted in "$@"; do
    if grep -qF -- "$unwanted" <<<"$out"; then
      echo "FAIL  $name: unexpected: $unwanted"
      sed 's/^/        /' <<<"$out"
      fail=1
      return
    fi
  done
  pass=$((pass + 1))
  echo "ok    $name"
}

LISTING="rows: folder a|Folder B|dangling|File1.txt|file2.txt|file10.txt|five.bytes"

# --- Listing and navigation -------------------------------------------------
check "listing: folders first, natural order, hidden filtered (VW-01)" "" \
  "path: $DATA" "$LISTING" "selected: folder a" \
  "status: 1 of 7 items selected, 1 hidden"

check "Cmd-Down opens the selected folder" "down,cmd+down" \
  "path: $DATA/Folder B" "rows: deep|inside.md|zeta.png" "back: true forward: false"

check "Cmd-Up goes up and selects the folder left (PT-11)" "down,cmd+down,cmd+up" \
  "path: $DATA" "selected: Folder B"

check "Back twice then Forward twice returns (PT-09)" \
  "down,cmd+down,cmd+down,cmd+[,cmd+[,cmd+],cmd+]" \
  "path: $DATA/Folder B/deep" "back: true forward: false"

check "Back restores the selection that folder had" "down,cmd+down,down,cmd+[" \
  "path: $DATA" "selected: Folder B"

check "typing selects by prefix, ignoring case" "type:fi" "selected: File1.txt"

check "status line shows the size of the selection" "type:five" \
  "status: 1 of 7 items selected, 5 bytes, 1 hidden"

check "Cmd-A selects all; Opt-Cmd-A deselects" "cmd+a,alt+cmd+a" \
  "selected: " "status: 7 items, 1 hidden"

# --- View -------------------------------------------------------------------
check "Shift-Cmd-. shows hidden files in every pane, selection kept" "type:fi,cmd+shift+." \
  "count: 8" "selected: File1.txt" \
  "sort: name asc=true foldersFirst=true hidden=true" \
  "other.sort: name asc=true foldersFirst=true hidden=true"

check "Ctrl-Cmd-6 sorts by Size; Opt-Cmd-O makes it descending (VW-03)" "cmd+ctrl+6,alt+cmd+o" \
  "sort: size asc=false foldersFirst=true hidden=false" \
  "rows: Folder B|folder a|five.bytes|file10.txt|file2.txt|File1.txt|dangling"

check "Ctrl-Cmd-3 sorts by Date Added, newest first when descending" \
  "touch:$DATA/new1,wait,wait,touch:$DATA/new2,wait,wait,cmd+ctrl+3,alt+cmd+o" \
  "sort: added asc=false foldersFirst=true hidden=false" "~|new2|new1|"

check "Ctrl-Cmd-7 sorts by Tags, untagged last" \
  "type:file2,tag:Red,wait,wait,cmd+ctrl+7" \
  "sort: tags asc=true foldersFirst=true hidden=false" "~rows: folder a|Folder B|file2.txt|"

check "Ctrl-Cmd-0 sorts by Permissions" "cmd+ctrl+0" "sort: permissions asc=true foldersFirst=true hidden=false"

check "Ctrl-Opt-Cmd-5 groups by Size; headers sit above their items (VW-05)" "cmd+ctrl+alt+5" \
  "groups: Folders@0|Less than 10 KB@2" "rows: folder a|Folder B|dangling|File1.txt|file2.txt|file10.txt|five.bytes"

check "Ctrl-Opt-Cmd-1 groups by Kind with folders first" "cmd+ctrl+alt+1" \
  "~groups: Folder@0|"

check "Ctrl-Opt-Cmd-3 groups by Date Modified: all made today" "cmd+ctrl+alt+3" "groups: Today@0"

check "Ctrl-Opt-Cmd-6 groups by Tags, No Tags last" "type:five,tag:Red,wait,wait,cmd+ctrl+alt+6" \
  "groups: Red@0|No Tags@1" "~rows: five.bytes|"

check "Ctrl-Opt-Cmd-0 ungroups" "cmd+ctrl+alt+5,cmd+ctrl+alt+0" "groups: "

check "grouped: arrows skip headers and selection maps to items" \
  "cmd+ctrl+alt+5,down,down" "selected: dangling" "groups: Folders@0|Less than 10 KB@2"

check "grouped: Cmd-A selects every item, no headers" "cmd+ctrl+alt+5,cmd+a" \
  "status: 7 of 7 items selected, 5 bytes, 1 hidden"

check "grouped: typing selects past a header" "cmd+ctrl+alt+5,type:file2" "selected: file2.txt"

check "Calculate Folder Size fills the Size column (VW-10)" "type:Folder B,cmd+shift+s,wait,wait" \
  "~cells: Folder B|" "~|Zero KB|"

check "folders show -- until calculated" "type:Folder B" "~|--|Folder"

check "Select All Files" "menu:Select All Files" "selected: dangling|File1.txt|file2.txt|file10.txt|five.bytes"

check "Select All Folders" "menu:Select All Folders" "selected: folder a|Folder B"

check "End and Home select the last and first item in List view" "end" "selected: five.bytes"

check "Home goes back to the first item" "end,home" "selected: folder a"

check "Always Sort Folders Alphabetically keeps folders in name order" \
  "cmd+ctrl+6,alt+cmd+o,menu:Always Sort Folders Alphabetically" \
  "rows: folder a|Folder B|five.bytes|file10.txt|file2.txt|File1.txt|dangling"

# --- View Options and per-folder views -----------------------------------
check "Cmd-J opens View Options for the active folder" "cmd+j,wait" "viewoptions: shown folder=left scope=folder"

check "a folder keeps its own view; its parent keeps the default (VW-07)" \
  "type:Folder B,cmd+down,wait,cmd+1,wait,cmd+up,wait,wait" "path: $DATA" "layout: list size=13"

check "coming back to the folder brings its view back (VW-07)" \
  "type:Folder B,cmd+down,wait,cmd+1,wait,cmd+up,wait,wait,cmd+down,wait,wait" \
  "path: $DATA/Folder B" "~layout: icons"

check "This Folder and Subfolders: a folder below opens the same way (VW-08)" \
  "cmd+ctrl+6,wait,cmd+j,wait,viewoptions:subfolders,wait,type:Folder B,cmd+down,wait,wait" \
  "path: $DATA/Folder B" "sort: size asc=true foldersFirst=true hidden=false"

check "Opt-Cmd-R restores the default view" "cmd+1,wait,alt+cmd+r,wait,wait" "layout: list size=13"

check "Use as Defaults: folders without their own view open that way" \
  "cmd+ctrl+6,wait,cmd+j,wait,viewoptions:defaults,wait,type:folder a,cmd+down,wait,wait" \
  "path: $DATA/folder a" "sort: size asc=true foldersFirst=true hidden=false"

check "View Options: bold folder names" "cmd+j,wait,viewoptions:toggle:Bold folder names,wait" \
  "selected: folder a" "namefont: bold"

check "folder names are regular by default" "" "namefont: regular"

check "View Options: Show item info puts the size under a file's icon" \
  "cmd+1,wait,cmd+j,wait,viewoptions:toggle:Show item info,wait,type:five,wait,wait" "gridinfo: 5 bytes"

if wanted "per-folder views survive a relaunch"; then
fixture
run "type:Folder B,cmd+down,wait,cmd+1,wait" "$DATA" >/dev/null
expect "per-folder views survive a relaunch" "$(run "type:Folder B,cmd+down,wait,wait" "$DATA")" \
  "path: $DATA/Folder B" "~layout: icons"
fi

check "a slow second click on a selected name starts renaming it" "type:five,slowclick:five.bytes,wait,wait,wait,wait,wait,wait,wait" \
  "~renaming: five.bytes"

# Finder's "hide extension" flag lives in the FinderInfo attribute.
hide_extension() { xattr -wx com.apple.FinderInfo 0000000000000000001000000000000000000000000000000000000000000000 "$1"; }

if wanted "hidden extension"; then
fixture
hide_extension "$DATA/file2.txt"
expect "hidden extension: the list shows the name without it" "$(run "type:file2,wait" "$DATA")" \
  "selected: file2.txt" "~cells: file2|"
fixture
hide_extension "$DATA/file2.txt"
run "type:file2,return,type:renamed,return,wait,wait,wait" "$DATA" >/dev/null
if [ -e "$DATA/renamed.txt" ] && [ ! -e "$DATA/renamed" ]; then
  pass=$((pass + 1)); echo "ok    hidden extension: renaming keeps the extension"
else
  echo "FAIL  hidden extension: renaming keeps the extension"; ls "$DATA" | sed 's/^/        /'; fail=1
fi
fi

check "group by Kind in Icon view: one section per group" "cmd+1,wait,cmd+ctrl+alt+1,wait,wait" \
  "~groups: Folder@0|" "~layout: icons"

check "grouped Icon view: arrows walk the items across groups" "cmd+1,wait,cmd+ctrl+alt+5,wait,wait,end" \
  "selected: five.bytes"

# --- Expandable folders (List view) --------------------------------------
check "Cmd-Right expands the selected folder in place" "type:Folder B,cmd+right,wait,wait" \
  "tree: folder a|Folder B|>deep|>inside.md|>zeta.png|dangling|File1.txt|file2.txt|file10.txt|five.bytes" \
  "status: 1 of 10 items selected, 1 hidden"

check "Cmd-Left collapses it again" "type:Folder B,cmd+right,wait,wait,cmd+left,wait" "tree: -" "selected: Folder B"

check "items inside an expanded folder can be selected" "type:Folder B,cmd+right,wait,wait,down,down" \
  "selected: inside.md"

check "Cmd-Left on an item inside collapses its folder and selects it" \
  "type:Folder B,cmd+right,wait,wait,down,down,cmd+left,wait" "tree: -" "selected: Folder B"

check "Opt-Cmd-Right expands everything inside" "type:Folder B,alt+cmd+right,wait,wait,wait" \
  "~tree: folder a|Folder B|>deep|>inside.md|>zeta.png|"

check "deleting an item inside an expanded folder updates the list" \
  "type:Folder B,cmd+right,wait,wait,down,down,down,cmd+delete,wait,wait,wait,wait" \
  "~tree: folder a|Folder B|>deep|>inside.md|dangling|"

check "expanded folders close when leaving List view" "type:Folder B,cmd+right,wait,wait,cmd+1,wait,cmd+2,wait" "tree: -"

check "grouped lists don't expand" "cmd+ctrl+alt+5,wait,type:Folder B,cmd+right,wait,wait" "tree: -"

check "spring-loading: pausing a drag over a folder opens it" "springhover:Folder B,wait,wait,wait,wait" \
  "path: $DATA/Folder B"

check "spring-loading: moving off the folder in time cancels it" "springhover:Folder B,springhover:,wait,wait,wait,wait" \
  "path: $DATA"

# --- Column view ---------------------------------------------------------
B="$ROOT/.testdata/ui-smoke-"
check "Cmd-3 shows columns: the parent, then this folder in focus" "cmd+3,wait" \
  "layout: columns size=13" "~columnview: " "~=left|[$DATA]" "selected: folder a"

check "columns: a selected folder opens to the right" "cmd+3,wait,type:Folder B,wait,wait" \
  "~|[$DATA]|>$DATA/Folder B" "~columnrows: " "selected: Folder B"

check "columns: Right enters the folder, a new column appears" "cmd+3,wait,type:Folder B,wait,right,wait,wait" \
  "path: $DATA/Folder B" "~=Folder B|[$DATA/Folder B]" "selected: deep"

check "columns: Left goes back and keeps the folder selected" "cmd+3,wait,type:Folder B,right,wait,wait,left,wait,wait" \
  "path: $DATA" "selected: Folder B" "~|>$DATA/Folder B"

check "columns: a selected file shows a preview column" "cmd+3,wait,type:five,wait" "~|>file:five.bytes"

check "columns: stay packed on the left, nothing stretched to fill the window" "alt+cmd+p,cmd+3,wait,type:five,wait,wait" \
  "~SideColumn:0+230 NSStackView:230+235 FilePreviewColumn:465+230"

check "columns: switching to List shows the deepest folder shown (VW-06)" \
  "cmd+3,wait,type:Folder B,wait,wait,cmd+2,wait,wait" "layout: list size=13" "path: $DATA/Folder B"

check "columns: folder in focus keeps the list's commands (Return renames)" \
  "cmd+3,wait,type:five,return" "~renaming: five.bytes"

check "columns: going above the first column re-roots" "cmd+3,wait,left,wait,wait,left,wait,wait" \
  "~|[$(dirname "$(dirname "$DATA")")]"

check "default columns are Name, Date Modified, Size, Kind" "" \
  "columns: Name|Date Modified|Size|Kind"

check "a shown column goes last and shows its values" \
  "column:permissions,column:tags,type:five" \
  "columns: Name|Date Modified|Size|Kind|Permissions|Tags" "~cells: five.bytes|" "~|-rw-r--r--|"

check "hiding a column removes it" "column:size" "columns: Name|Date Modified|Kind"

check "Tags column lists the item's tags" "type:file2,tag:Red,wait,wait,column:tags" "~|Red"

if wanted "columns survive a relaunch (VW-09)"; then
fixture
run "column:comments,column:added" >/dev/null
expect "columns survive a relaunch (VW-09)" "$(run "")" \
  "columns: Name|Date Modified|Size|Kind|Comments|Date Added"
fi

if wanted "both panes follow a column change"; then
fixture
expect "both panes follow a column change" "$(run "column:owner,tab" "$DATA")" \
  "columns: Name|Date Modified|Size|Kind|Owner"
fi

# --- Panes ------------------------------------------------------------------
wanted "fresh window" && fixture && \
expect "fresh window: two panes at the starting directory, left active (PT-01)" "$(run "")" \
  "active: left" "layout: dual vertical sync=false" \
  "path: $START" "other.path: $START" "focus: active-list"

check "Tab moves focus to the other pane" "tab" \
  "active: right" "path: $START" "other.path: $DATA" "focus: active-list"

check "single pane mode and back keeps the other pane (PT-02)" "alt+cmd+p,alt+cmd+p" \
  "layout: dual vertical sync=false" "other.path: $START"

check "Tab does nothing in single pane mode" "alt+cmd+p,tab" \
  "layout: single vertical sync=false" "active: left"

check "Clone Pane shows the active location in the other pane (PT-04)" "down,cmd+down,cmd+shift+c" \
  "path: $DATA/Folder B" "other.path: $DATA/Folder B"

check "Sync Browsing follows into a same-named subfolder (PT-05)" "cmd+shift+y,down,cmd+down" \
  "layout: dual vertical sync=true" "path: $DATA/Folder B" "other.path: $START/Folder B"

check "Sync Browsing: no counterpart, other pane stays (PT-05)" "cmd+shift+y,cmd+down" \
  "path: $DATA/folder a" "other.path: $START" "layout: dual vertical sync=true"

check "Open in Other Pane (Opt-Shift-Cmd-Down)" "down,alt+shift+cmd+down" \
  "path: $DATA" "other.path: $DATA/Folder B"

# --- Tabs -------------------------------------------------------------------
check "one tab: tab bar hidden" "" "tabbar: left=false right=false" "left.tabs: *left"

check "Cmd-T opens a tab at the same location and shows the tab bar" "cmd+t" \
  "left.tabs: left|*left" "tabbar: left=true right=false" "path: $DATA"

check "Opt-Cmd-Down opens the folder in a new tab" "down,alt+cmd+down" \
  "left.tabs: left|*Folder B" "path: $DATA/Folder B"

check "Ctrl-1 and Ctrl-Tab select tabs" "down,alt+cmd+down,ctrl+1,ctrl+tab,ctrl+tab" \
  "left.tabs: *left|Folder B"

check "Cmd-W closes the tab" "cmd+t,cmd+w" "left.tabs: *left" "tabbar: left=false right=false"

check "a pinned tab ignores Cmd-W and Close Other Tabs (PT-06)" \
  "alt+cmd+shift+p,cmd+w,cmd+t,alt+cmd+w" \
  "left.tabs: left(pinned)|*left"

check "navigating a pinned tab opens a new tab instead" "alt+cmd+shift+p,down,cmd+down" \
  "left.tabs: left(pinned)|*Folder B"

check "Move Tab to Other Pane" "down,alt+cmd+down,alt+cmd+shift+t" \
  "left.tabs: *left" "right.tabs: right|*Folder B"

# --- Sidebar, refresh, restore ----------------------------------------------
FAVORITES="Home"
for d in Desktop Documents Downloads; do
  if [ -d "$HOME/$d" ]; then FAVORITES="$FAVORITES,$d"; fi
done
if [ -d /Applications ]; then FAVORITES="$FAVORITES,Applications"; fi

check "sidebar lists devices, favorites and recent folders (PT-12)" "down,cmd+down" \
  "~sidebar: shown Devices[" "~] Favorites[$FAVORITES] Recent Folders[Folder B,left,right]"

check "Opt-Cmd-S hides the sidebar" "alt+cmd+s" \
  "~sidebar: hidden Devices["

check "a file created on disk appears without a manual refresh" "touch:$DATA/appeared.txt,wait,wait,wait,wait" \
  "rows: folder a|Folder B|appeared.txt|dangling|File1.txt|file2.txt|file10.txt|five.bytes"

check "deleting the open folder falls back to its parent (PT-15)" \
  "down,cmd+down,cmd+down,rm:$DATA/Folder B/deep,wait,wait,wait,wait" \
  "path: $DATA/Folder B" "error: -"

if wanted "relaunch restores"; then
fixture
run "cmd+t,down,cmd+down,alt+cmd+shift+p,tab,cmd+ctrl+6" >/dev/null
expect "relaunch restores tabs, pins, focus and sort (PT-14)" "$(run "")" \
  "left.tabs: right|*zzz(pinned)" "right.tabs: *right" "active: right" \
  "sort: size asc=true foldersFirst=true hidden=false" "other.path: $START/zzz"
fi

if wanted "restored tab whose folder is gone"; then
fixture
run "cmd+down" >/dev/null   # both panes at $START; left enters "Folder B"
rmdir "$START/Folder B"
expect "a restored tab whose folder is gone shows an inline error" "$(run "")" \
  "error: “Folder B” can’t be opened." "count: 0"
fi

# --- File operations (M3) ----------------------------------------------------
W="wait,wait,wait"   # let an operation finish and the pane refresh

check "ops: F5 copies the selection to the other pane (FO-01)" "type:five,f5,$W" \
  "$LISTING" "other.rows: Folder B|zzz|five.bytes|r.txt" \
  "log.last: Copied 1 item [success]" "activities: " "undo: Undo Copy"

check "ops: F6 moves the selection to the other pane" "type:five,f6,$W" \
  "rows: folder a|Folder B|dangling|File1.txt|file2.txt|file10.txt" \
  "other.rows: Folder B|zzz|five.bytes|r.txt" "log.last: Moved 1 item [success]" "undo: Undo Move"

check "ops: F5 is unavailable in single pane mode" "alt+cmd+p,type:five,f5,$W" \
  "log.last: -" "$LISTING"

check "ops: Cmd-D duplicates and selects the copy (FO-12)" "type:five,cmd+d,$W" \
  "rows: folder a|Folder B|dangling|File1.txt|file2.txt|file10.txt|five copy.bytes|five.bytes" \
  "selected: five copy.bytes" "undo: Undo Duplicate"

check "ops: Shift-Cmd-N makes a folder and starts renaming it (FO-13)" "cmd+shift+n,$W" \
  "rows: folder a|Folder B|untitled folder|dangling|File1.txt|file2.txt|file10.txt|five.bytes" \
  "selected: untitled folder" "renaming: untitled folder editor=untitled folder"

check "ops: typing a name and Return finishes the new folder" "cmd+shift+n,$W,type:Projects,return,$W" \
  "rows: folder a|Folder B|Projects|dangling|File1.txt|file2.txt|file10.txt|five.bytes" \
  "selected: Projects" "renaming: -" "undo: Undo Rename" "log.last: Renamed 1 item [success]"

check "ops: Opt-Shift-Cmd-N makes a file" "alt+cmd+shift+n,$W" \
  "selected: untitled file.txt" "renaming: untitled file.txt editor=untitled file.txt"

check "ops: Return renames in place and keeps the extension" "type:file2,return,type:notes,return,$W" \
  "rows: folder a|Folder B|dangling|File1.txt|file10.txt|five.bytes|notes.txt" \
  "selected: notes.txt" "focus: active-list"

check "ops: Esc cancels a rename" "type:file2,return,type:zzz,escape,wait" \
  "$LISTING" "renaming: -" "log.last: -" "focus: active-list"

check "ops: Delete while renaming edits the name, not the file (VW-12)" \
  "type:file2,return,type:abc,delete,return,$W" \
  "rows: folder a|Folder B|ab.txt|dangling|File1.txt|file10.txt|five.bytes"

check "ops: Cmd-Delete while renaming does not trash anything (KS-04)" \
  "type:file2,return,cmd+delete,wait,escape,wait" "$LISTING" "log.last: -"

check "ops: renaming to a taken name is refused, nothing overwritten" \
  "type:file2,return,type:file10,return,$W" \
  "$LISTING" "~already exists" "undo: -"

check "ops: Cmd-Delete moves to the Trash without asking (FO-10)" "type:five,cmd+delete,$W" \
  "rows: folder a|Folder B|dangling|File1.txt|file2.txt|file10.txt" \
  "undo: Undo Move to Trash" "confirm: -" "log.last: Moved 1 item to the Trash [success]"

check "ops: Cmd-Z puts it back; Shift-Cmd-Z trashes it again (FO-10)" \
  "type:five,cmd+delete,$W,cmd+z,$W" \
  "$LISTING" "undo: -" "redo: Redo Move to Trash" "selected: five.bytes"

check "ops: Shift-Cmd-Z redoes the trash" "type:five,cmd+delete,$W,cmd+z,$W,cmd+shift+z,$W" \
  "rows: folder a|Folder B|dangling|File1.txt|file2.txt|file10.txt" "undo: Undo Move to Trash" "redo: -"

check "ops: Cmd-Z in a rename field undoes typing, not the file operation" \
  "type:five,cmd+delete,$W,type:file2,return,type:xyz,cmd+z,escape,$W" \
  "rows: folder a|Folder B|dangling|File1.txt|file2.txt|file10.txt" "undo: Undo Move to Trash"

check "ops: Opt-Cmd-Delete asks first and Cancel keeps the file (FO-09)" \
  "type:five,alt+cmd+delete,wait,confirm:cancel,$W" "$LISTING" "log.last: -"

check "ops: confirming Delete Immediately erases, with no undo (FO-09)" \
  "type:five,alt+cmd+delete,wait,confirm:delete,$W" \
  "rows: folder a|Folder B|dangling|File1.txt|file2.txt|file10.txt" \
  "undo: -" "log.last: Deleted “five.bytes” [success]"

if wanted "ops: the delete confirmation"; then
fixture
out="$(run "type:five,alt+cmd+delete,wait" "$DATA")"
expect "ops: the delete confirmation names the item and nothing is deleted yet" "$out" \
  "confirm: Delete “five.bytes” immediately?" "$LISTING"
fi

check "ops: clipboard: Cmd-C then Cmd-V in the other pane copies" "type:five,cmd+c,tab,cmd+v,$W" \
  "active: right" "rows: Folder B|zzz|five.bytes|r.txt" "other.$LISTING"

check "ops: clipboard: Cmd-X then Cmd-V moves" "type:five,cmd+x,tab,cmd+v,$W" \
  "rows: Folder B|zzz|five.bytes|r.txt" \
  "other.rows: folder a|Folder B|dangling|File1.txt|file2.txt|file10.txt"

check "ops: clipboard: pasting into the same folder makes a copy" "type:five,cmd+c,cmd+v,$W" \
  "rows: folder a|Folder B|dangling|File1.txt|file2.txt|file10.txt|five copy.bytes|five.bytes"

# Conflicts: the other pane already has a "five.bytes" and a "Folder B".
conflict_fixture() {
  fixture
  printf 'old' > "$START/five.bytes"
}

if wanted "ops: a name conflict"; then
conflict_fixture
expect "ops: a name conflict asks and waits (CF-12 buttons)" "$(run "type:five,f5,$W" "$DATA")" \
  "prompt: conflict fileFile five.bytes" \
  "prompt.buttons: Skip|Replace|Replace Older|Keep Both|Stop" \
  "activities: Copying “five.bytes” [running]"
[ "$(cat "$START/five.bytes")" = "old" ] || { echo "FAIL  target changed before the answer"; fail=1; }
fi

if wanted "ops: Keep Both"; then
conflict_fixture
expect "ops: Keep Both writes a numbered copy (CF-04)" "$(run "type:five,f5,$W,answer:keepboth,$W" "$DATA")" \
  "other.rows: Folder B|zzz|five 2.bytes|five.bytes|r.txt" "prompt: -" "log.last: Copied 1 item [success]"
[ "$(cat "$START/five.bytes")" = "old" ] || { echo "FAIL  Keep Both changed the existing file"; fail=1; }
fi

if wanted "ops: Replace"; then
conflict_fixture
expect "ops: Replace overwrites after the answer" "$(run "type:five,f5,$W,answer:replace,$W" "$DATA")" \
  "other.rows: Folder B|zzz|five.bytes|r.txt" "prompt: -"
[ "$(cat "$START/five.bytes")" = "hello" ] || { echo "FAIL  Replace did not replace"; fail=1; }
fi

if wanted "ops: Stop"; then
conflict_fixture
expect "ops: Stop ends the activity and plays no part (CF-12)" "$(run "type:five,f5,$W,answer:stop,$W" "$DATA")" \
  "prompt: -" "activities: " "log.last: Copying “five.bytes” [stopped]"
[ "$(cat "$START/five.bytes")" = "old" ] || { echo "FAIL  Stop changed the existing file"; fail=1; }
fi

if wanted "ops: folder conflicts offer Merge"; then
conflict_fixture
expect "ops: folder conflicts offer Merge (CF-05)" "$(run "down,f5,$W" "$DATA")" \
  "prompt: conflict folderFolder Folder B" \
  "prompt.buttons: Skip|Replace|Merge|Replace Older|Keep Both|Stop"
fi

if wanted "ops: Merge adds"; then
conflict_fixture
out="$(run "down,f5,$W,answer:merge,$W" "$DATA")"
expect "ops: Merge adds the missing children (CF-05)" "$out" "prompt: -" "log.last: Copied 4 items [success]"
[ -f "$START/Folder B/inside.md" ] && [ -d "$START/Folder B/deep" ] || { echo "FAIL  merge did not add children"; fail=1; }
fi

if wanted "ops: replacing a folder with extra items"; then
conflict_fixture
printf 'mine' > "$START/Folder B/keep.txt"
out="$(run "down,f5,$W,answer:replace,$W" "$DATA")"
expect "ops: replacing a folder with extra items asks to confirm (conflicts §1)" "$out" \
  "prompt: confirm-replace Folder B" "prompt.buttons: Cancel|Replace"
out="$(run "down,f5,$W,answer:replace,$W,answer:cancel,$W" "$DATA")"
expect "ops: and Cancel goes back to the choices" "$out" \
  "prompt: conflict folderFolder Folder B extras=keep.txt"
[ -f "$START/Folder B/keep.txt" ] || { echo "FAIL  the folder was replaced without confirmation"; fail=1; }
out="$(run "down,f5,$W,answer:replace,$W,answer:replace,$W,$W" "$DATA")"
expect "ops: and confirming replaces it" "$out" "prompt: -"
[ ! -e "$START/Folder B/keep.txt" ] && [ -f "$START/Folder B/inside.md" ] || { echo "FAIL  the confirmed Replace did not replace"; fail=1; }
fi

if wanted "ops: a partial file"; then
fixture
head -c 3000000 /dev/zero | tr '\0' 'r' > "$DATA/big.bin"
touch -t 202001010000 "$DATA/big.bin"
head -c 1000000 "$DATA/big.bin" > "$START/big.bin"
size=$(stat -f %z "$DATA/big.bin"); mtime=$(( $(stat -f %m "$DATA/big.bin") * 1000 ))
# What a stopped copy records (see ops/partials.rs).
printf '[{"target_fs":"local","target":"%s","source_fs":"local","source":"%s","source_size":%s,"source_modified_ms":%s}]' \
  "$START/big.bin" "$DATA/big.bin" "$size" "$mtime" > "$RUSTYFINDER_SUPPORT_DIR/partials.json"
expect "ops: a partial file left by a stopped copy offers Resume (CF-11)" "$(run "type:big,f5,$W" "$DATA")" \
  "prompt: conflict fileFile big.bin resumable" "prompt.buttons: Skip|Resume|Replace|Replace Older|Keep Both|Stop"
expect "ops: and Resume completes it" "$(run "type:big,f5,$W,answer:resume,$W,$W" "$DATA")" "prompt: -"
cmp -s "$DATA/big.bin" "$START/big.bin" || { echo "FAIL  the resumed file differs from the source"; fail=1; }
fi

if wanted "ops: shortcuts do not act behind a prompt"; then
conflict_fixture
out="$(run "type:five,f5,$W,cmd+delete,alt+cmd+delete,cmd+d,cmd+shift+n,$W" "$DATA")"
expect "ops: shortcuts do not act behind a prompt (EC-08)" "$out" \
  "prompt: conflict fileFile five.bytes" "$LISTING" "confirm: -"
[ -z "$(ls -A "$RUSTYFINDER_TRASH_DIR")" ] || { echo "FAIL  something was trashed behind a prompt"; fail=1; }
fi

# --- App Deleter (app-deleter spec) ------------------------------------------
# A fake app and a fake home: the real ~/Library is never read here.
app_fixture() {
  fixture
  local on="${1:-true}"
  mkdir -p "$DATA/Foo.app/Contents/MacOS"
  {
    echo '<?xml version="1.0" encoding="UTF-8"?>'
    echo '<plist version="1.0"><dict>'
    echo '<key>CFBundleIdentifier</key><string>com.example.foo</string>'
    echo '<key>CFBundleName</key><string>Foo</string>'
    echo '<key>CFBundleExecutable</key><string>Foo</string>'
    echo '<key>CFBundlePackageType</key><string>APPL</string>'
    echo '</dict></plist>'
  } > "$DATA/Foo.app/Contents/Info.plist"
  printf 'bin' > "$DATA/Foo.app/Contents/MacOS/Foo"
  export RUSTYFINDER_APP_DELETER_HOME="$BASE/home"
  local lib="$BASE/home/Library"
  mkdir -p "$lib/Preferences" "$lib/Caches/com.example.foo" "$lib/Application Support/Foo" "$lib/Caches/com.example.foobar"
  printf 'prefs' > "$lib/Preferences/com.example.foo.plist"
  printf 'cache' > "$lib/Caches/com.example.foo/blob"
  printf '{"starting_directory":"%s","play_sounds":false,"detect_app_when_deleting":%s}' "$START" "$on" \
    > "$RUSTYFINDER_SUPPORT_DIR/settings.json"
}
APPROWS="rows: folder a|Folder B|Foo.app|dangling|File1.txt|file2.txt|file10.txt|five.bytes"

if wanted "appdeleter: moving an app to the Trash lists its files"; then
app_fixture
out="$(run "type:Foo.,cmd+delete,$W" "$DATA")"
expect "appdeleter: moving an app to the Trash lists its files, possible matches unchecked (AD-01, AD-02)" "$out" \
  "~appdeleter: Foo scanning=false items=[x] Foo.app app|[x] com.example.foo confident|[x] com.example.foo.plist confident|[ ] Foo possible total=3 of 4 checked" \
  "$APPROWS"
refute "appdeleter: look-alikes are never listed" "$out" "com.example.foobar"
[ -d "$DATA/Foo.app" ] || { echo "FAIL  the app went before the window was answered"; fail=1; }
fi

if wanted "appdeleter: unchecked items stay"; then
app_fixture
out="$(run "type:Foo.,cmd+delete,$W,ad:uncheck=com.example.foo,ad:trash,wait,confirm:move to trash,$W" "$DATA")"
expect "appdeleter: Move to Trash takes the checked items only, as one undoable step (AD-03)" "$out" \
  "appdeleter: -" "undo: Undo Move to Trash" "log.last: Moved 2 items to the Trash [success]" \
  "rows: folder a|Folder B|dangling|File1.txt|file2.txt|file10.txt|five.bytes"
lib="$BASE/home/Library"
[ ! -e "$lib/Preferences/com.example.foo.plist" ] && [ -d "$lib/Caches/com.example.foo" ] \
  && [ -d "$lib/Application Support/Foo" ] && [ -e "$RUSTYFINDER_TRASH_DIR/com.example.foo.plist" ] \
  || { echo "FAIL  App Deleter trashed the wrong items"; ls -R "$lib" "$RUSTYFINDER_TRASH_DIR"; fail=1; }
fi

if wanted "appdeleter: Undo puts everything back"; then
app_fixture
out="$(run "type:Foo.,cmd+delete,$W,ad:trash,wait,confirm:move to trash,$W,cmd+z,$W" "$DATA")"
expect "appdeleter: Undo puts the app and its files back (AD-04)" "$out" "$APPROWS" "redo: Redo Move to Trash"
lib="$BASE/home/Library"
[ -e "$lib/Preferences/com.example.foo.plist" ] && [ -f "$lib/Caches/com.example.foo/blob" ] \
  || { echo "FAIL  Undo did not restore the app's files"; fail=1; }
fi

if wanted "appdeleter: Cancel"; then
app_fixture
out="$(run "type:Foo.,cmd+delete,$W,ad:cancel,$W" "$DATA")"
expect "appdeleter: Cancel leaves everything" "$out" "appdeleter: -" "$APPROWS" "undo: -"
fi

if wanted "appdeleter: setting off"; then
app_fixture false
out="$(run "type:Foo.,cmd+delete,$W" "$DATA")"
expect "appdeleter: with the setting off the app is trashed directly (AD-05)" "$out" \
  "appdeleter: -" "undo: Undo Move to Trash" "rows: folder a|Folder B|dangling|File1.txt|file2.txt|file10.txt|five.bytes"
[ -e "$BASE/home/Library/Preferences/com.example.foo.plist" ] || { echo "FAIL  leftovers trashed with the setting off"; fail=1; }
fi

if wanted "appdeleter: the menu command"; then
app_fixture false
out="$(run "type:Foo.,menu:Move App to Trash with Related Files…,$W" "$DATA")"
expect "appdeleter: the menu command opens it with the setting off" "$out" "~appdeleter: Foo scanning=false"
fi
unset RUSTYFINDER_APP_DELETER_HOME

# --- Aliases, symlinks, undo of tags (file-ops §1, §8) ----------------------
if wanted "links: Ctrl-Cmd-A makes a Finder alias"; then
fixture
out="$(run "type:five,ctrl+cmd+a,$W" "$DATA")"
expect "links: Ctrl-Cmd-A makes a Finder alias next to the item" "$out" \
  "rows: folder a|Folder B|dangling|File1.txt|file2.txt|file10.txt|five.bytes|five.bytes alias" \
  "undo: Undo Make Alias" "selected: five.bytes alias"
# A real alias holds bookmark data, not a copy and not a symlink.
[ "$(head -c 4 "$DATA/five.bytes alias")" = "book" ] && [ ! -L "$DATA/five.bytes alias" ] \
  || { echo "FAIL  the alias is not bookmark data"; fail=1; }
fi

check "links: Undo removes the alias" "type:five,ctrl+cmd+a,$W,cmd+z,$W" "$LISTING" "redo: Redo Make Alias"

if wanted "links: alias and symlink in the other pane"; then
fixture
out="$(run "type:five,menu:Make Alias in Other Pane,$W,menu:Make Symlink in Other Pane,$W" "$DATA")"
expect "links: Make Alias / Make Symlink in Other Pane create them there" "$out" \
  "other.rows: Folder B|zzz|five.bytes alias|five.bytes symlink|r.txt" "$LISTING"
# Same volume: the symlink is relative, so the pair can move together.
[ "$(readlink "$START/five.bytes symlink")" = "../left/five.bytes" ] && [ "$(cat "$START/five.bytes symlink")" = "hello" ] \
  || { echo "FAIL  symlink target: $(readlink "$START/five.bytes symlink")"; fail=1; }
fi

if wanted "undo: tags"; then
fixture
printf '{"starting_directory":"%s","play_sounds":true}' "$START" > "$RUSTYFINDER_SUPPORT_DIR/settings.json"
out="$(run "type:five,tag:Red,wait,cmd+z,$W" "$DATA")"
expect "undo: Cmd-Z takes a tag change back, with the undo sound (file-ops §8)" "$out" \
  "tags: five.bytes=[]" "redo: Redo Tags" "undo: -" "sound.last: Tink" "log.last: Undo Tags [success]"
out="$(run "type:five,tagsedit:Work+Home,wait,cmd+z,$W,cmd+shift+z,$W" "$DATA")"
expect "undo: Redo puts the tags back" "$out" "tags: five.bytes=[Work+Home]" "undo: Undo Tags" "redo: -"
fi

# --- Edit, Print, Share, toolbar, Help (file-ops §1, keyboard §1) -----------
SET="menu:Settings…,wait"
check "edit: Cmd-E without editors opens the usual application" "type:file2,cmd+e" \
  "external: default <- $DATA/file2.txt"

check "edit: an editor listed for the extension is used (settings §3)" \
  "$SET,settings:editor=add=/System/Applications/TextEdit.app,settings:editor=ext=0=.BYTES txt,wait,type:five,cmd+e" \
  "external: TextEdit.app <- $DATA/five.bytes" "~editors=TextEdit.app[bytes,txt]"

if wanted "edit: the default editor"; then
fixture
out="$(run "$SET,settings:editor=add=/System/Applications/TextEdit.app,settings:editor=default=0,wait,type:File1,cmd+e" "$DATA")"
expect "edit: the default editor takes every other extension, and is kept" "$out" \
  "external: TextEdit.app <- $DATA/File1.txt" "~editors=TextEdit.app[]*"
expect "edit: editors survive a relaunch" "$(run "$SET" "$DATA")" "~editors=TextEdit.app[]*"
fi

check "edit: folders can't be edited" "type:folder,cmd+e" "external: -"

check "print: Cmd-P prints text through RustyFinder's own panel" "type:file2,cmd+p" "external: print text <- file2.txt"

check "share: Share… offers the selection to the share sheet" "type:five,menu:Share…" "external: share five.bytes"

check "toolbar: Copy to, Move to and Delete buttons are there" "" "~|Enclosing Folder|Copy to|Move to|Delete|"

check "toolbar: Delete moves to the Trash" "type:five,toolbar:delete,$W" \
  "rows: folder a|Folder B|dangling|File1.txt|file2.txt|file10.txt" "undo: Undo Move to Trash"

check "toolbar: Option-Delete asks before deleting for good" "type:five,opt:on,toolbar:delete,wait" \
  "confirm: Delete “five.bytes” immediately?" "$LISTING"

check "toolbar: Copy to copies to the other pane" "type:five,toolbar:copy to,$W" "other.rows: Folder B|zzz|five.bytes|r.txt"

check "help: Cmd-? shows the README from inside the app" "cmd+?,wait" "~help: RustyFinder Help chars=" "~first=RustyFinder"

if wanted "notify: a long activity"; then
fixture
out="$(RUSTYFINDER_PRETEND_BACKGROUND=1 RUSTYFINDER_NOTIFY_AFTER=0 run "type:five,f5,$W" "$DATA")"
expect "notify: a long activity finishing in the background posts a notification (file-ops §5.6)" "$out" \
  "notify.last: Copying “five.bytes”: Finished."
fixture
out="$(RUSTYFINDER_PRETEND_BACKGROUND=1 run "type:five,f5,$W" "$DATA")"
expect "notify: a quick one does not" "$out" "notify.last: -"
fixture
out="$(RUSTYFINDER_PRETEND_BACKGROUND=0 RUSTYFINDER_NOTIFY_AFTER=0 run "type:five,f5,$W" "$DATA")"
expect "notify: nothing while RustyFinder is in front" "$out" "notify.last: -"
fi

if wanted "queue: dragging a queued activity"; then
conflict_fixture
printf '{"starting_directory":"%s","play_sounds":false,"max_transfers":1}' "$START" > "$RUSTYFINDER_SUPPORT_DIR/settings.json"
out="$(run "type:five,f5,$W,actcopy:$DATA/file2.txt>$START,actcopy:$DATA/file10.txt>$START,wait,actmove:1>0,wait" "$DATA")"
expect "queue: a queued activity dragged up starts earlier (FO-07)" "$out" \
  "activities: Copying “five.bytes” [running]|Copying “file10.txt” [queued]|Copying “file2.txt” [queued]"
fi

# --- Permissions, locks and iCloud (edge-cases §4, cloud.md §2) -------------
if wanted "admin: a permission problem offers Retry as Administrator"; then
fixture
mkdir -p "$DATA/sys" && printf 'x' > "$DATA/sys/x" && chmod 555 "$DATA/sys"
out="$(run "type:sys,cmd+down,wait,type:x,cmd+delete,$W" "$DATA")"
expect "admin: a permission problem offers Retry as Administrator (file-ops §1)" "$out" \
  "prompt: error x admin" "prompt.buttons: Retry as Administrator…|Retry|Skip|Skip All|Stop"
out="$(run "type:sys,cmd+down,wait,type:x,cmd+delete,$W,answer:retryasadministrator…,$W" "$DATA")"
expect "admin: declining the password is a failure, never a success" "$out" \
  "log.last: Moving “x” to the Trash [failed]" "~external: admin: '/bin/mv' '-n' '--' '$DATA/sys/x' '$RUSTYFINDER_TRASH_DIR/x'"
chmod 755 "$DATA/sys"
[ -f "$DATA/sys/x" ] || { echo "FAIL  the item moved without administrator rights"; fail=1; }
fi

if wanted "access: an unreadable folder"; then
fixture
mkdir -p "$DATA/locked" && chmod 000 "$DATA/locked"
out="$(run "type:locked,cmd+down,$W,banner:extra,wait,confirm:authorize,$W" "$DATA")"
chmod 755 "$DATA/locked"
expect "access: an unreadable folder offers Authorize, through the system prompt (edge-cases §4)" "$out" \
  "error: “locked” can’t be opened." "error.action: Authorize…" \
  "~external: admin: '/bin/chmod' '+a' 'user:$(id -un) allow list,search,readattr,readextattr,readsecurity' '$DATA/locked'" \
  "confirm: Access to “locked” wasn’t changed."
fi

if wanted "lock: a locked item"; then
fixture
chflags uchg "$DATA/five.bytes"
out="$(run "type:five,cmd+delete,wait" "$DATA")"
expect "lock: trashing a locked item offers to unlock it first (edge-cases §4)" "$out" \
  "preflight: “five.bytes” is locked." "$LISTING"
out="$(run "type:five,cmd+delete,wait,confirm:unlock and continue,$W" "$DATA")"
expect "lock: Unlock and Continue trashes it" "$out" \
  "rows: folder a|Folder B|dangling|File1.txt|file2.txt|file10.txt" "preflight: -"
chflags -R nouchg "$BASE" 2>/dev/null
fi

if wanted "icloud: a busy item"; then
fixture
out="$(RUSTYFINDER_FAKE_ICLOUD_BUSY=five.bytes run "type:five,cmd+delete,wait" "$DATA")"
expect "icloud: an item iCloud is syncing can't be trashed right now, and says so (edge-cases §4)" "$out" \
  "preflight: “five.bytes” can’t be moved to the Trash right now." "$LISTING"
fi

if wanted "icloud: cloud-only items"; then
fixture
out="$(RUSTYFINDER_FAKE_CLOUD_ONLY=five.bytes,file2.txt run "type:five,cmd+shift+p,$W" "$DATA")"
expect "icloud: cloud-only items carry a badge and the preview never downloads them (CL-05, CL-07)" "$out" \
  "cloud.badges: file2.txt|five.bytes" "preview.kind: cloud" "cloud.last: - problem=-" \
  "ql.first: placeholder five.bytes (in iCloud, not downloaded)"
out="$(RUSTYFINDER_FAKE_CLOUD_ONLY=five.bytes run "type:five,cloud:download,wait" "$DATA")"
expect "icloud: Download Now asks the system (outside iCloud it reports why it can't)" "$out" \
  "cloud.last: download five.bytes failed problem=Unable to Download"
out="$(run "type:file,shift+down,shift+down,cloud:evict,wait" "$DATA")"
expect "icloud: Remove Download takes several items and reports what failed (CL-06)" "$out" \
  "cloud.last: evict 3 failed 3 problem=Unable to Remove Download"
fi

# --- Links and volumes (edge-cases §2, §3) ------------------------------------
check "alias: opening an alias to a folder shows that folder (edge-cases §3)" \
  "type:folder a,ctrl+cmd+a,$W,type:folder a ,cmd+down,$W" "path: $DATA/folder a"

check "alias: an alias whose original is gone can't be resolved" \
  "type:five,ctrl+cmd+a,$W,rm:$DATA/five.bytes,wait,wait,type:five,cmd+down,$W" \
  "confirm: The item “five.bytes alias” can’t be resolved."

if wanted "volume: exFAT"; then
fixture
# Disk images mount reliably only from the temporary folder.
img="$(mktemp -d)"
vol="$img/vol"
mkdir -p "$vol"
hdiutil create -quiet -size 4m -fs ExFAT -volname RFSmoke -layout NONE "$img/vol.dmg" \
  && hdiutil attach -quiet -nobrowse -mountpoint "$vol" "$img/vol.dmg"
touch "$vol/tagged.txt" && xattr -w com.apple.metadata:_kMDItemUserTags Red "$vol/tagged.txt"
[ -e "$vol/._tagged.txt" ] || { echo "FAIL  no AppleDouble companion was written"; fail=1; }
printf '{"starting_directory":"%s","play_sounds":false,"show_hidden":true}' "$START" > "$RUSTYFINDER_SUPPORT_DIR/settings.json"
out="$(run "" "$vol")"
expect "volume: AppleDouble companions are never listed, even with hidden files shown (EC-06)" "$out" \
  "rows: tagged.txt"
out="$(run "cmd+shift+e,$W,wait,wait" "$vol")"
expect "volume: Cmd-Shift-E moves the pane off the volume and ejects it" "$out" \
  "path: $START" "eject.last: ejected vol"
if mount | grep -qF "$vol"; then echo "FAIL  the volume is still mounted"; fail=1; fi
hdiutil detach -quiet -force "$vol" 2>/dev/null
rm -rf "$img"
fi

if wanted "volume: a busy volume"; then
fixture
# Disk images mount reliably only from the temporary folder.
img="$(mktemp -d)"
vol="$img/vol"
mkdir -p "$vol"
hdiutil create -quiet -size 4m -fs HFS+ -volname RFBusy -layout NONE "$img/vol.dmg" \
  && hdiutil attach -quiet -nobrowse -mountpoint "$vol" "$img/vol.dmg"
printf 'x' > "$vol/held.txt"
# A process keeps a file open on it.
tail -f "$vol/held.txt" >/dev/null 2>&1 &
holder=$!
out="$(run "cmd+shift+e,$W,wait,wait,wait,wait" "$vol")"
kill "$holder" 2>/dev/null; wait "$holder" 2>/dev/null
expect "volume: a busy volume names the program using it (edge-cases §2)" "$out" \
  "~eject.last: The volume “RFBusy” can’t be ejected because it’s in use by tail." "path: $START"
hdiutil detach -quiet -force "$vol" 2>/dev/null
rm -rf "$img"
fi

# --- Quick Look (M4) ---------------------------------------------------------
check "ql: Space opens Quick Look" "type:five,wait,wait,wait,wait,wait,space,wait,wait" \
  "quicklook: visible" "ql.first: file five.bytes"

# Closing the panel is not checked here: while the app is not frontmost (as
# under this harness) the system panel ignores orderOut. See PROGRESS.md.

check "ql: a space typed mid-name is part of the name, not Quick Look" "type:folder,space,type:a" \
  "selected: folder a" "quicklook: hidden"

# --- Preview pane, Info, Get Info (M4) ---------------------------------------
# Two text files inside "folder a": one UTF-8, one Shift-JIS without a mark.
PV="$DATA/folder a"
preview_fixture() {
  fixture
  printf 'hello world\n' > "$PV/notes.txt"
  chmod 644 "$PV/notes.txt"
  python3 -c "import sys; open(sys.argv[1],'wb').write('これは日本語のテキストです。ファイルマネージャーのプレビューを確認します。'.encode('shift_jis'))" "$PV/sjis.txt"
}
# pv <name> <keys> <expected...>: open "folder a", then run the keys.
pv() {
  local name="$1" keys="$2"; shift 2
  wanted "$name" || return 0
  preview_fixture
  PV_OUT="$(run "cmd+down,wait,$keys" "$DATA")"
  expect "$name" "$PV_OUT" "$@"
}

pv "pv: the preview pane is hidden until Cmd-Shift-P" "type:notes" "preview: hidden"

pv "pv: a text file shows as editable text with its encoding and exact size (PV-01)" \
  "type:notes,cmd+shift+p,wait,wait" \
  "preview: shown" "preview.mode: preview" "preview.kind: text" "preview.items: notes.txt" \
  "preview.text: hello world\\n" "preview.encoding: Unicode (UTF-8)" \
  "info: notes.txt" "info.Size: 12 bytes (12 bytes)" "info.Octal: 644" "~info.Where: "

pv "pv: the preview follows the selection" \
  "cmd+shift+p,type:notes,wait,wait,wait,wait,wait,type:sjis,wait,wait,wait" \
  "preview.items: sjis.txt"

pv "pv: Shift-JIS without a mark is shown as readable Japanese (PV-03)" \
  "type:sjis,cmd+shift+p,wait,wait" \
  "preview.kind: text" "preview.encoding: Japanese (Shift JIS)" "~preview.text: これは日本語のテキストです"

pv "pv: editing and saving writes the file (PV-02)" \
  "type:notes,cmd+shift+p,wait,wait,previewtext:changed text,previewsave,wait" "preview.kind: text"
if wanted "pv: saved text is on disk"; then
  preview_fixture
  run "cmd+down,wait,type:notes,cmd+shift+p,wait,wait,previewtext:changed text,previewsave,wait" "$DATA" >/dev/null
  if [ "$(cat "$PV/notes.txt")" = "changed text" ] && [ "$(stat -f %Lp "$PV/notes.txt")" = 644 ]; then
    pass=$((pass + 1)); echo "ok    pv: saved text is on disk, permissions kept (PV-02)"
  else
    echo "FAIL  pv: saved text is on disk: got '$(cat "$PV/notes.txt")' mode $(stat -f %Lp "$PV/notes.txt")"; fail=1
  fi
fi

if wanted "pv: saving keeps the file's encoding"; then
  preview_fixture
  run "cmd+down,wait,type:sjis,cmd+shift+p,wait,wait,previewtext:日本語を保存,previewsave,wait" "$DATA" >/dev/null
  if [ "$(iconv -f SHIFT_JIS -t UTF-8 "$PV/sjis.txt" 2>/dev/null)" = "日本語を保存" ]; then
    pass=$((pass + 1)); echo "ok    pv: saving keeps the file's encoding (PV-02)"
  else
    echo "FAIL  pv: saving keeps the file's encoding: $(xxd "$PV/sjis.txt" | head -2)"; fail=1
  fi
fi

pv "pv: Log and back to Preview shows the selection again (PV-05)" \
  "type:notes,cmd+shift+p,wait,wait,previewmode:log,wait,previewmode:preview,wait" \
  "preview.mode: preview" "preview.kind: text" "preview.text: hello world\\n"

pv "pv: Cmd-Shift-U shows the pane in Activities mode" "cmd+shift+u,wait" \
  "preview: shown" "preview.mode: activities"

pv "pv: the Log lists finished operations truthfully" \
  "type:notes,cmd+d,$W,cmd+shift+p,previewmode:log,wait" "preview.mode: log" "~Duplicated “notes.txt”"

pv "pv: a folder shows its icon; several items show a summary" \
  "cmd+up,wait,cmd+shift+p,wait,wait" "preview.kind: folder" "preview.items: folder a"

pv "pv: several selected items are summarised" "cmd+shift+p,cmd+a,wait,wait" \
  "preview.kind: several" "info: 2 items"

if wanted "pv: typing 664 into Octal"; then
  preview_fixture
  out="$(run "cmd+down,wait,type:notes,cmd+shift+p,wait,wait,setoctal:664,$W" "$DATA")"
  expect "pv: typing 664 into Octal changes the file mode (PV-06)" "$out" "info.Octal: 664" \
    "log.last: Changed permissions of “notes.txt” to 664 [success]"
  [ "$(stat -f %Lp "$PV/notes.txt")" = 664 ] || { echo "FAIL  mode is $(stat -f %Lp "$PV/notes.txt"), not 664"; fail=1; }
fi

pv "pv: invalid octal is refused and the old value comes back" \
  "type:notes,cmd+shift+p,wait,wait,setoctal:999,$W" "info.Octal: 644" "log.last: -"

pv "pv: ticking Hidden hides the file from the list (PV-14)" \
  "type:notes,cmd+shift+p,wait,wait,infoclick:hidden,$W,wait" "rows: sjis.txt" "status: 1 item, 1 hidden"

pv "pv: Cmd-I opens a Get Info window for the selection" "type:notes,cmd+i,wait" \
  "infowindows: 1" "window.info: notes.txt" "window.info.Octal: 644"

pv "pv: Get Info opens one window per item" "cmd+a,cmd+i,wait" "infowindows: 2"

if wanted "pv: ACL"; then
  preview_fixture
  chmod +a "everyone deny write" "$PV/notes.txt"
  expect "pv: ACL entries show as R ACL / W ACL (§1.2)" \
    "$(run "cmd+down,wait,type:notes,cmd+shift+p,wait,wait" "$DATA")" "info.ACL: W ACL"
  chmod -N "$PV/notes.txt"
fi

if wanted "pv: a symlink's original"; then
  preview_fixture
  ln -s notes.txt "$PV/link"
  expect "pv: a symlink's original is described (§1.2)" \
    "$(run "cmd+down,wait,type:link,cmd+shift+p,wait,wait" "$DATA")" \
    "~info.Original: $PV/notes.txt Change…" "info.Original size: 12 bytes (12 bytes)" "~info.Original modified: "
  expect "pv: a symlink can be pointed at another original (§1.2)" \
    "$(run "cmd+down,wait,type:link,cmd+shift+p,wait,wait,infotarget:$PV/sjis.txt,wait" "$DATA")" \
    "~info.Original: $PV/sjis.txt Change…"
  [ "$(readlink "$PV/link")" = "$PV/sjis.txt" ] && { pass=$((pass + 1)); echo "ok    pv: the link on disk points at the new original"; } \
    || { echo "FAIL  pv: link points at $(readlink "$PV/link")"; fail=1; }
fi

if wanted "pv: PDF display options"; then
  preview_fixture
  python3 - "$PV/doc.pdf" <<'PY'
import sys
objs = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>", b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>"]
out = b"%PDF-1.4\n"; offsets = []
for i, o in enumerate(objs):
    offsets.append(len(out)); out += b"%d 0 obj\n" % (i + 1) + o + b"\nendobj\n"
xref = len(out)
out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1) + b"".join(b"%010d 00000 n \n" % o for o in offsets)
out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)
open(sys.argv[1], "wb").write(out)
PY
  run "cmd+down,wait,type:doc,cmd+shift+p,wait,wait,previewpdf:0/1.5,wait" "$DATA" >/dev/null
  expect "pv: PDF display options are remembered across launches (§1.1)" \
    "$(run "cmd+down,wait,type:doc,cmd+shift+p,wait,wait" "$DATA")" "preview.kind: pdf" "preview.pdf: mode=0 scale=1.50"
fi

pv "pv: several items: common tags, and adding one tags them all (PV-09)" \
  "type:notes,shift+down,tag:Red,cmd+shift+p,wait,wait,infotags:Red+Green,wait,wait" \
  "info: 2 items" "~notes.txt=[Red+Green]" "~sjis.txt=[Red+Green]" "~info.Added: " "~info.Created: "

pv "pv: several items: folder sizes are calculated on request (§1.2)" \
  "cmd+up,wait,cmd+a,cmd+shift+p,wait,wait,infoclick:calculate,$W,wait" \
  "~info: 7 items" "~info.Size: " "~info.Modified: "
if wanted "pv: several folders not counted"; then
  preview_fixture
  expect "pv: several items: folders are not counted until calculated" \
    "$(run "cmd+a,cmd+shift+p,wait,wait" "$DATA")" "~plus 2 folders not counted"
fi

pv "pv: a Get Info window follows changes made elsewhere (§1.3)" \
  "type:notes,cmd+i,wait,wait,touch:$PV/notes.txt,wait,wait,wait,wait" \
  "window.info.Size: Zero KB (0 bytes)" "~infowindow.changes: "

check "ql: the slideshow moves on by itself (§2)" "type:file2,shift+down,alt+cmd+y,wait,wait,slideshowtick,wait" \
  "~slideshow=true" "~ql.position: 1/2"

# --- Icon view and sizes (M4) -----------------------------------------------
check "view: Cmd-1 switches to icons and keeps selection and focus" "type:five,cmd+1,wait,wait" \
  "layout: icons size=64" "selected: five.bytes" "focus: active-list" "other.layout: list size=13"

check "view: Cmd-2 switches back to the list" "type:five,cmd+1,wait,cmd+2,wait,wait" \
  "layout: list size=13" "selected: five.bytes" "focus: active-list"

check "view: Right walks icons in order, Left stops at the first (VW-11)" \
  "cmd+1,wait,wait,left,right,right" "selected: dangling"

check "view: End and Home jump in icon view" "cmd+1,wait,wait,end" "selected: five.bytes"

check "view: commands work in icon view (duplicate)" "cmd+1,wait,wait,type:five,cmd+d,$W" \
  "selected: five copy.bytes" "layout: icons size=64"

check "view: rename works in icon view" "cmd+1,wait,wait,type:file2,return,type:notes,return,$W" \
  "rows: folder a|Folder B|dangling|File1.txt|file10.txt|five.bytes|notes.txt" "selected: notes.txt"

# Views are per folder (views.md §6): a folder without its own settings
# opens with the defaults, so icon view set as the default follows along.
check "view: Cmd-Down opens a folder from icon view; it opens with the defaults" \
  "cmd+1,wait,wait,right,cmd+down,wait" "path: $DATA/Folder B" "layout: list size=13"

check "view: with icon view as the default, opened folders show icons" \
  "cmd+1,wait,cmd+j,wait,viewoptions:defaults,wait,right,cmd+down,wait" "path: $DATA/Folder B" "layout: icons size=64"

check "view: Cmd-plus and Cmd-minus change the list text size" "cmd++,cmd++,cmd+-,wait" \
  "layout: list size=14" "other.layout: list size=14"

check "view: icon size is separate from text size" "cmd+1,wait,cmd++,wait,cmd+2,wait,wait" \
  "layout: list size=13"

if wanted "view: layout and sizes survive a relaunch"; then
fixture
run "cmd+1,wait,cmd++,wait" >/dev/null
expect "view: layout and sizes survive a relaunch" "$(run "")" "layout: icons size=96"
fi

# --- SFTP (M5) ---------------------------------------------------------------
# A throwaway sshd on 127.0.0.1, run as the current user with its own keys
# and config, started the first time a shard needs it. The app is pointed at
# a matching ssh config and known_hosts, so ~/.ssh is never read or written.
SSHD_DIR="$ROOT/.testdata/ui-smoke-sshd-$SHARD_INDEX"
SSHD_PID=""
REMOTE="$BASE/remote"
sshd_stop() { [ -n "$SSHD_PID" ] && kill "$SSHD_PID" 2>/dev/null; rm -rf "$SSHD_DIR"; }
trap sshd_stop EXIT
sshd_start() {
  [ -n "$SSHD_PID" ] && return 0
  rm -rf "$SSHD_DIR"; mkdir -p "$SSHD_DIR"
  ssh-keygen -q -t ed25519 -N "" -f "$SSHD_DIR/host_key" >/dev/null
  ssh-keygen -q -t ed25519 -N "" -f "$SSHD_DIR/client_key" >/dev/null
  cp "$SSHD_DIR/client_key.pub" "$SSHD_DIR/authorized_keys"
  # Shards run side by side and each picks a free port, so two can pick
  # the same one. A server only counts if its own process is still alive
  # once the port answers; otherwise try another port.
  local port attempt
  for attempt in 1 2 3 4 5; do
    port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
    cat > "$SSHD_DIR/sshd_config" <<CFG
Port $port
ListenAddress 127.0.0.1
HostKey $SSHD_DIR/host_key
AuthorizedKeysFile $SSHD_DIR/authorized_keys
PidFile $SSHD_DIR/sshd.pid
UsePAM no
PasswordAuthentication no
KbdInteractiveAuthentication no
StrictModes no
Subsystem sftp /usr/libexec/sftp-server
LogLevel ERROR
CFG
    /usr/sbin/sshd -D -e -f "$SSHD_DIR/sshd_config" >/dev/null 2>&1 &
    SSHD_PID=$!
    for _ in $(seq 1 50); do
      nc -z 127.0.0.1 "$port" 2>/dev/null && break
      sleep 0.1
    done
    sleep 0.2
    if nc -z 127.0.0.1 "$port" 2>/dev/null && kill -0 "$SSHD_PID" 2>/dev/null; then
      break
    fi
    kill "$SSHD_PID" 2>/dev/null
    SSHD_PID=""
  done
  [ -n "$SSHD_PID" ] || return 1
  cat > "$SSHD_DIR/ssh_config" <<CFG
Host t
  HostName 127.0.0.1
  Port $port
  User $USER
  IdentityFile $SSHD_DIR/client_key
  IdentitiesOnly yes
CFG
  ssh-keyscan -p "$port" 127.0.0.1 > "$SSHD_DIR/known_hosts" 2>/dev/null
  : > "$SSHD_DIR/unknown_hosts"
  export RUSTYFINDER_SSH_CONFIG="$SSHD_DIR/ssh_config"
  export RUSTYFINDER_KNOWN_HOSTS="$SSHD_DIR/known_hosts"
}
# sftp <name> <keys> <expected...>: fresh local and remote folders, then keys.
SFTP_OUT=""
SFTP_RAN=""   # set when the last sftp check really ran in this shard
sftp() {
  local name="$1" keys="$2"; shift 2
  SFTP_RAN=""
  wanted "$name" || return 0
  if ! sshd_start; then
    echo "skip  $name (sshd could not be started here)"
    return 0
  fi
  fixture
  mkdir -p "$REMOTE/docs"
  printf 'remote hello' > "$REMOTE/hello.txt"
  SFTP_OUT="$(run "$keys" "$DATA")"
  SFTP_RAN=1
  expect "$name" "$SFTP_OUT" "$@"
}
# same_bytes <what> <file> <expected>: checked right after the sftp check
# that produced the file, in the same shard.
same_bytes() {
  [ -n "$SFTP_RAN" ] || return 0
  if [ "$(cat "$2" 2>/dev/null)" = "$3" ]; then
    pass=$((pass + 1)); echo "ok    $1"
  else
    echo "FAIL  $1: got '$(cat "$2" 2>&1)'"; fail=1
  fi
}
C="connect:sftp://t$REMOTE,wait,wait,wait,wait,wait,wait"   # connect, then let it settle

sftp "sftp: connecting shows the server's folder in the active pane (SF-01)" "$C" \
  "conn: remote" "path: $REMOTE" "rows: docs|hello.txt" "connections: t" \
  "left.tabs: *remote" "subtitle: t:$REMOTE" "other.conn: local" "problem: -"

sftp "terminal: on SFTP Open in Terminal starts ssh in the remote folder (§2)" "$C,type:docs,cmd+shift+r" \
  "~external: ssh: ssh -t -- 't' 'cd '" "~$REMOTE/docs'" "~|| exit 1; exec \"\$SHELL\" -l'"

sftp "sftp: F5 uploads the selection to the server" "tab,$C,tab,type:five,f5,$W,$W" \
  "other.rows: docs|five.bytes|hello.txt" "log.last: Copied 1 item [success]" "undo: -"
same_bytes "sftp: the uploaded file has the right bytes" "$REMOTE/five.bytes" "hello"

sftp "sftp: F5 downloads from the server to the local pane" "$C,type:hello,f5,$W,$W" \
  "other.rows: Folder B|zzz|hello.txt|r.txt" "log.last: Copied 1 item [success]"
same_bytes "sftp: the downloaded file has the right bytes" "$START/hello.txt" "remote hello"

sftp "sftp: rename works on the server" "$C,type:hello,return,type:renamed,return,$W,$W" \
  "rows: docs|renamed.txt" "selected: renamed.txt"

sftp "sftp: New Folder works on the server and the pane refreshes by itself" "$C,cmd+shift+n,$W,$W" \
  "rows: docs|untitled folder|hello.txt" "selected: untitled folder"

if wanted "sftp: a changed host key"; then
if sshd_start; then
fixture
mkdir -p "$REMOTE"
port="$(awk '/Port/ {print $2}' "$SSHD_DIR/ssh_config")"
ssh-keygen -q -t ed25519 -N "" -f "$BASE/other_key" >/dev/null
printf '[127.0.0.1]:%s %s\n' "$port" "$(cut -d' ' -f1,2 "$BASE/other_key.pub")" > "$BASE/changed_hosts"
out="$(RUSTYFINDER_KNOWN_HOSTS="$BASE/changed_hosts" run "$C,confirm:cancel,wait,wait" "$DATA")"
expect "sftp: a changed host key is a warning, and Cancel connects nothing (SF-03)" "$out" \
  "conn: local" "connect.decision: The identity of “t” has changed -> cancel"
out="$(RUSTYFINDER_KNOWN_HOSTS="$BASE/changed_hosts" run "$C,confirm:connect anyway,wait,wait,wait,wait" "$DATA")"
expect "sftp: Connect Anyway accepts the new key the warning showed (SF-03)" "$out" "conn: remote"
if grep -q "$(cut -d' ' -f2 "$BASE/other_key.pub")" "$BASE/changed_hosts"; then
  echo "FAIL  sftp: the old host key is still on record"; fail=1
else
  pass=$((pass + 1)); echo "ok    sftp: the old host key was replaced on record"
fi
fi
fi

if wanted "sftp: thumbnails for files on a server"; then
if sshd_start; then
fixture
mkdir -p "$REMOTE/docs"
python3 -c "
import zlib, struct, sys
def chunk(t, d): return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
raw = b''.join(b'\\x00' + b'\\x20\\x80\\xff' * 64 for _ in range(64))
open(sys.argv[1], 'wb').write(b'\\x89PNG\\r\\n\\x1a\\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 64, 64, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b''))
" "$REMOTE/pic.png"
expect "sftp: thumbnails for files on a server stay off by default (views §3.2)" \
  "$(run "$C,cmd+1,wait,wait,wait,wait,wait" "$DATA")" "grid.thumbs: "
expect "sftp: thumbnails for files on a server when switched on (views §3.2)" \
  "$(run "$C,cmd+1,wait,cmd+j,wait,viewoptions:toggle:Also for files on servers ,wait,wait,wait,wait,wait,wait,wait" "$DATA")" \
  "~grid.thumbs: pic.png"
fi
fi

sftp "sftp: Cmd-Delete asks first on a server, which has no Trash (FO-11)" \
  "$C,type:hello,cmd+delete,wait" \
  "confirm: Delete “hello.txt” from the server?" "rows: docs|hello.txt"

sftp "sftp: confirming deletes on the server, nothing goes to the Trash" \
  "$C,type:hello,cmd+delete,wait,confirm:delete,$W,$W" \
  "rows: docs" "log.last: Deleted “hello.txt” [success]" "undo: -"

sftp "sftp: Cmd-Down enters a remote folder and Cmd-Up comes back" "$C,cmd+down,$W,cmd+up,$W" \
  "path: $REMOTE" "selected: docs" "conn: remote"

sftp "sftp: Back leaves the server for the local folder" "$C,cmd+[,$W" \
  "conn: local" "path: $DATA" "back: true forward: true"

sftp "sftp: Disconnect sends the pane to the starting directory" "$C,disconnect,$W" \
  "conn: local" "path: $START" "connections: " "left.tabs: *right"

sftp "sftp: Quick Look shows a server's file after fetching it (PV-12)" "$C,type:hello,wait,wait,wait,wait,wait,space,wait,wait,wait,wait" \
  "quicklook: visible" "ql.items: hello.txt"

sftp "sftp: Get Info works on a server, with owner and group (SF-13)" "$C,type:hello,cmd+i,wait" \
  "infowindows: 1" "window.info: hello.txt" "~window.info.Owner (" "~window.info.Octal: "

sftp "sftp: the preview fetches a small file and shows it (PV-04)" "$C,type:hello,cmd+shift+p,wait,wait,wait,wait" \
  "preview.kind: text" "preview.text: remote hello" "~preview.source: copy fetched hello.txt"

sftp "sftp: permissions are changed on the server from Info (SF-13)" \
  "$C,type:hello,cmd+shift+p,wait,wait,wait,setoctal:600,$W" "~info.Octal: 600" "~log.last: Changed permissions"
same_bytes "sftp: the server's file has the new mode" <(stat -f %Lp "$REMOTE/hello.txt" 2>/dev/null) "600"

if wanted "sftp: a big file waits for the Download button"; then
  if sshd_start; then
    fixture
    mkdir -p "$REMOTE"
    head -c 11000000 /dev/zero > "$REMOTE/big.bin"
    out="$(run "$C,type:big,cmd+shift+p,wait,wait,wait,wait" "$DATA")"
    expect "sftp: a big file is not downloaded for the preview until asked (PV-04)" "$out" \
      "preview.kind: download" "preview.note: 11 MB, not downloaded"
    expect "sftp: the Download button fetches it (PV-04)" \
      "$(run "$C,type:big,cmd+shift+p,wait,wait,previewdownload,$W,$W,wait" "$DATA")" "~preview.source: copy fetched big.bin"
  else
    echo "skip  sftp: a big file waits for the Download button (sshd could not be started here)"
  fi
fi

sftp "sftp: Refresh fetches the previewed file again (§1.1)" \
  "$C,type:hello,cmd+shift+p,wait,wait,wait,touch:$REMOTE/hello.txt,cmd+r,$W,wait,wait" \
  "preview.kind: text" "preview.text: "

if wanted "sftp: an unknown host shows its fingerprint"; then
  if sshd_start; then
    fixture; mkdir -p "$REMOTE"
    out="$(RUSTYFINDER_KNOWN_HOSTS="$SSHD_DIR/unknown_hosts" run "$C" "$DATA")"
    expect "sftp: an unknown host shows its fingerprint before connecting (SF-02)" "$out" \
      "~ssh.prompt: hostKey: " "~SHA256:" "conn: remote"
    : > "$SSHD_DIR/unknown_hosts"
  fi
fi

check "sftp: the Connect panel needs a server before Connect is enabled" "cmd+k,wait" \
  "~connectsheet: protocol=SFTP server= user= port= path= password=- connect=false error=- "

check "sftp: a pasted URL fills in the Connect panel (SF-12)" \
  "cmd+k,wait,connectfill:sftp://bob@example.org:2222/var/www" \
  "~connectsheet: protocol=SFTP server=example.org user=bob port=2222 path=/var/www password=- connect=true error=- "

# --- Quick Open, Go to Folder, Quick Select (M6) -----------------------------
check "qo: Esc in the file list opens Quick Open with favorites first" "escape,wait" \
  "~palette: field= rows=50 top=*Home | "

check "qo: fuzzy typing finds a favorite; Return opens it (QO-01)" \
  "escape,wait,palette:appl,palettekey:return,$W" "path: /Applications" "palette: -"

check "qo: commands show their menu path and key binding (QO-02)" "escape,wait,palette:size" \
  "~View › Sort By › Size [⌃⌘6]"

check "qo: running a command from Quick Open does what the menu does" \
  "escape,wait,palette:sort size,palettekey:return,$W" \
  "sort: size asc=true foldersFirst=true hidden=false" "palette: -"

if wanted "qo: no Open With section without a selected file"; then
  fixture
  refute "qo: no Open With section without a selected file (QO-03)" \
    "$(run "escape,wait,palette:open with" "$DATA")" "Open With ›"
fi

check "qo: Open With appears when a file is selected" "type:file1,escape,wait,palette:open with" \
  "~Open With ›"

check "qo: no pane command runs behind Quick Open (EC-08)" "type:five,escape,wait,cmd+delete,cmd+d,$W" \
  "$LISTING" "log.last: -"

check "qo: Ctrl-D lists devices only" "ctrl+d,wait" "~top=*Macintosh HD"

check "goto: Esc closes Go to Folder and does not open Quick Open (KS-05)" \
  "cmd+shift+g,wait,palettekey:escape,wait" "palette: -" "path: $DATA"

check "goto: a quoted path is accepted without its quotes (QO-04)" \
  "cmd+shift+g,wait,palette:'$DATA/Folder B',palettekey:return,$W" "path: $DATA/Folder B" "palette: -"

check "goto: Tab completes with exactly one trailing slash (QO-05)" \
  "cmd+shift+g,wait,palette:$DATA/Fo,palettekey:tab" \
  "~palette: field=$DATA/folder a/ rows=0"

check "goto: completion lists matching entries, folders first" "cmd+shift+g,wait,palette:$DATA/f" \
  "~palette: field=$DATA/f rows=6 top=*folder a | Folder B | File1.txt | file2.txt | file10.txt | five.bytes"

check "goto: a relative path is taken from the current folder" \
  "cmd+shift+g,wait,palette:Folder B/deep,palettekey:return,$W" "path: $DATA/Folder B/deep"

check "goto: a file path shows its folder with the file selected (QO-06)" \
  "tab,cmd+shift+g,wait,palette:$DATA/five.bytes,palettekey:return,$W" \
  "active: right" "path: $DATA" "selected: five.bytes"

check "goto: a missing path says so and the pop-up stays open" \
  "cmd+shift+g,wait,palette:/nope/nothing,palettekey:return" \
  "~message=“/nope/nothing” can’t be found." "path: $DATA"

check "goto: earlier destinations are offered newest first (QO-07)" \
  "cmd+shift+g,wait,palette:$DATA/Folder B,palettekey:return,$W,cmd+shift+g,wait,palette:$START,palettekey:return,$W,cmd+shift+g,wait" \
  "~palette: field= rows=2 top=*${START/#$HOME/~} | ${DATA/#$HOME/~}/Folder B"

# The system stores the name decomposed (NFD); typing plain "resume" must
# still find it.
RESUME_NFD="$(python3 -c "import unicodedata; print(unicodedata.normalize('NFD', 'résumé.pdf'))")"
check "qs: plain text matches ignoring accents (QO-08)" \
  "touch:$DATA/résumé.pdf,$W,cmd+s,wait,qs:resume,qskey:return" "selected: $RESUME_NFD"

check "qs: Remove from Selection with an extension (QO-09)" \
  "cmd+a,cmd+s,wait,qsmode:2,qs:.txt,qskey:return" "selected: folder a|Folder B|dangling|five.bytes"

check "qs: Add to Selection keeps what was selected" "cmd+s,wait,qsmode:1,qs:*.bytes,qskey:return" \
  "selected: folder a|five.bytes"

check "qs: matches are counted, and Esc restores the selection" "cmd+s,wait,qs:file,qskey:escape" \
  "selected: folder a" "quickselect: -"

check "qs: the count is shown while typing" "cmd+s,wait,qs:file" \
  "quickselect: 3 matches" "selected: File1.txt|file2.txt|file10.txt"

check "ops: copying a folder into itself is refused with a reason (FO-04)" \
  "down,alt+shift+cmd+down,wait,f5,$W" \
  "~into itself" "log.last: Copying “Folder B” [failed]" "undo: -"

# --- Multi-Rename (M6) -------------------------------------------------------
# rule <kind> <body json> [target]: one rule in the core's JSON shape.
rule() {
  printf '{"kind":{"%s":%s},"target":"%s","enabled":true,"files":true,"folders":true}' "$1" "$2" "${3:-Name}"
}
# rules <file> <rule>...: write a rule list for the `mrrules:` token.
rules() {
  local file="$1"; shift
  local IFS=,
  printf '[%s]' "$*" > "$file"
}
SEQ='{"start":1,"step":1,"padding":3,"position":"Beginning","prefix":"","suffix":"_"}'
TWO="type:file2,shift+down,return,wait"   # file2.txt + file10.txt, then Return

check "rename: Return with several items opens Multi-Rename, nothing to do yet" "$TWO" \
  "~multirename: " "~file2.txt→file2.txt,file10.txt→file10.txt | rename=false rules=1"

if wanted "rename: a sequence numbers the items"; then
fixture; rules "$BASE/r.json" "$(rule AddSequence "$SEQ")"
expect "rename: a sequence numbers the items (MR-01)" "$(run "$TWO,mrrules:$BASE/r.json,wait" "$DATA")" \
  "multirename: 2 of 2 items will be renamed | file2.txt→001_file2.txt,file10.txt→002_file10.txt | rename=true rules=1" \
  "$LISTING"
fi

if wanted "rename: reordering the rows renumbers"; then
fixture; rules "$BASE/r.json" "$(rule AddSequence "$SEQ")"
expect "rename: reordering the rows renumbers, Rename applies, Cmd-Z restores (MR-07)" \
  "$(run "$TWO,mrrules:$BASE/r.json,mrmove:1>0,wait,mrrename,$W" "$DATA")" \
  "rows: folder a|Folder B|001_file10.txt|002_file2.txt|dangling|File1.txt|five.bytes" \
  "log.last: Renamed 2 items [success]" "multirename: -" "undo: Undo Rename"
fixture; rules "$BASE/r.json" "$(rule AddSequence "$SEQ")"
expect "rename: one undo puts every name back" \
  "$(run "$TWO,mrrules:$BASE/r.json,wait,mrrename,$W,cmd+z,$W" "$DATA")" "$LISTING"
fi

if wanted "rename: two items ending up with one name"; then
fixture
rules "$BASE/r.json" "$(rule ReplaceText '{"find":"file\\d+","replace":"y","case_sensitive":false,"regex":true,"occurrences":"All","entire_text":false}')"
expect "rename: two items ending up with one name are flagged, Rename is off (MR-03)" \
  "$(run "$TWO,mrrules:$BASE/r.json,wait,mrrename,$W" "$DATA")" \
  "~file2.txt→y.txt(conflict),file10.txt→y.txt(conflict) | rename=false" "$LISTING"
fi

if wanted "rename: a slash in a new name"; then
fixture
rules "$BASE/r.json" "$(rule AddText '{"text":"a/b","position":"End"}')"
expect "rename: a slash in a new name is invalid, Rename is off (MR-09)" \
  "$(run "$TWO,mrrules:$BASE/r.json,wait" "$DATA")" "~(invalid)" "~rename=false"
fi

if wanted "rename: swapping two names"; then
fixture
printf A > "$DATA/a"; printf BB > "$DATA/b"
swap() { rule ReplaceText "{\"find\":\"$1\",\"replace\":\"$2\",\"case_sensitive\":true,\"regex\":false,\"occurrences\":\"All\",\"entire_text\":false}"; }
rules "$BASE/r.json" "$(swap a '#')" "$(swap b a)" "$(swap '#' b)"
out="$(run "type:a,shift+down,return,wait,mrrules:$BASE/r.json,wait,mrrename,$W" "$DATA")"
out+=$'\n'"a holds $(cat "$DATA/a"), b holds $(cat "$DATA/b")"
expect "rename: swapping two names works without a prompt (MR-04)" "$out" \
  "a holds BB, b holds A" "log.last: Renamed 2 items [success]" "prompt: -"
fi

check "rename: Add Text at a Specific Index (MR-10)" \
  "$TWO,mradd:AddText,mredit:Text=X,mredit:Position=Specific Index,mredit:Index=2,wait" \
  "~file2.txt→fiXle2.txt,file10.txt→fiXle10.txt | rename=true"

check "rename: a Specific Index counted from the end" \
  "$TWO,mradd:AddText,mredit:Text=X,mredit:Position=Specific Index,mredit:Index=1,mredit:from end=on,wait" \
  "~file2.txt→fileX2.txt,file10.txt→file1X0.txt"

check "rename: a rule switched off does nothing (§2)" "$TWO,mradd:AddText,mredit:Text=Q,mrtoggle:1=off,wait" \
  "~file2.txt→file2.txt,file10.txt→file10.txt | rename=false rules=2"

check "rename: rules apply in the order they are dragged into (§2)" \
  "$TWO,mradd:AddText,mredit:Text=a,mradd:ChangeCase,mredit:Case=Upper Case,wait,mrrulemove:2>1,wait" \
  "~file2.txt→FILE2a.txt,file10.txt→FILE10a.txt"

check "rename: a custom date with a preset format (§3.4)" \
  "$TWO,mradd:AddDate,mredit:Date=Custom Date,mredit:Custom date=1735560000000,mredit:Date format presets=yyyyMMdd,wait" \
  "~file2.txt→20241230 file2.txt,file10.txt→20241230 file10.txt"

# exif_jpeg <file.jpg>: a 2x1 JPEG taken "2023:07:14 09:30:00" with a camera "TestCam".
exif_jpeg() {
  local out="$1" png="$1.png"
  python3 - "$png" <<'PY'
import sys, zlib, struct
def chunk(t, d): return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
raw = b'\x00' + b'\xff\x00\x00' * 2
png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 2, 1, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')
open(sys.argv[1], 'wb').write(png)
PY
  sips -s format jpeg "$png" --out "$out" >/dev/null 2>&1
  rm -f "$png"
  python3 - "$out" <<'PY'
import sys, struct
path = sys.argv[1]
data = open(path, 'rb').read()
model = b'TestCam\x00'
date = b'2023:07:14 09:30:00\x00'
# TIFF, big endian. IFD0 at 8: Model, ExifIFD pointer. Exif IFD: DateTimeOriginal.
ifd0 = 8
ifd0_size = 2 + 2 * 12 + 4
exif = ifd0 + ifd0_size
exif_size = 2 + 12 + 4
model_at = exif + exif_size
date_at = model_at + len(model)
t = b'MM\x00\x2a' + struct.pack('>I', ifd0)
t += struct.pack('>H', 2)
t += struct.pack('>HHII', 0x0110, 2, len(model), model_at)
t += struct.pack('>HHII', 0x8769, 4, 1, exif)
t += struct.pack('>I', 0)
t += struct.pack('>H', 1)
t += struct.pack('>HHII', 0x9003, 2, len(date), date_at)
t += struct.pack('>I', 0)
t += model + date
app1 = b'Exif\x00\x00' + t
seg = b'\xff\xe1' + struct.pack('>H', len(app1) + 2) + app1
open(path, 'wb').write(data[:2] + seg + data[2:])
PY
}
if wanted "rename: photo metadata"; then
fixture
exif_jpeg "$DATA/photo.jpg"; touch "$DATA/plain.jpg"
expect "rename: the photo's capture date, camera and size come from its metadata (§3.4, §3.6)" \
  "$(run "type:photo,shift+down,return,wait,wait,mradd:AddDate,mredit:Date=Metadata,mradd:AddMetadata,mredit:Field=Camera,mradd:AddMetadata,mredit:Field=Dimensions,wait" "$DATA")" \
  "~photo.jpg→2023-07-14 photoTestCam2x1.jpg,plain.jpg→plain.jpg | rename=true"
fi

# --- Sync (M6) ---------------------------------------------------------------
SYNC="alt+cmd+shift+s,wait,wait"
PLAN="rows=[dangling:Add:>:on,File1.txt:Add:>:on,file2.txt:Add:>:on,file10.txt:Add:>:on,five.bytes:Add:>:on,folder a:Add:>:on,Folder B/deep:Add:>:on,Folder B/inside.md:Add:>:on,Folder B/zeta.png:Add:>:on]"

check "sync: Sync to… compares source and target, no deletions by default" "$SYNC" \
  "~sync: dir=LeftToRight busy=false delete=false" "~$PLAN" \
  "~All: 9 (5 bytes) | Added: 9 (5 bytes) | Updated: 0 " "~sync=true"

check "sync: from the right pane the direction points left" "tab,$SYNC" \
  "~sync: dir=RightToLeft" "~rows=[r.txt:Add:<:on,zzz:Add:<:on]"

check "sync: unavailable in single-pane mode" "alt+cmd+p,$SYNC" "sync: -"

check "sync: resting on a row expands it to show both sides (§2)" "$SYNC,sync:hover=five.bytes,wait" \
  "~hover=five.bytes height=84 five.bytes / Left: $DATA/five.bytes / " "~Right: $START/five.bytes /     not there"

if wanted "sync: a tag rule"; then
fixture
tag_hex="$(python3 -c 'import plistlib,sys; sys.stdout.write(plistlib.dumps(["Red\n6"], fmt=plistlib.FMT_BINARY).hex())')"
xattr -wx com.apple.metadata:_kMDItemUserTags "$tag_hex" "$DATA/five.bytes"
expect "sync: a Has tag rule leaves tagged items out (§4)" "$(run "$SYNC,sync:tag=Red,wait,wait" "$DATA")" \
  "~rows=[dangling:Add:>:on,File1.txt:Add:>:on,file2.txt:Add:>:on,file10.txt:Add:>:on,folder a:Add:>:on,"
expect "sync: a Doesn't have tag rule leaves the others out (§4)" "$(run "$SYNC,sync:tag=!Red,wait,wait" "$DATA")" \
  "~rows=[five.bytes:Add:>:on]"
fi

check "sync: the toolbar Sync button opens the Sync window (§1)" "toolbarclick:Sync,wait,wait" \
  "~sync: dir=LeftToRight busy=false"

if wanted "sync: deleting orphans"; then
fixture
out="$(run "$SYNC,sync:delete,wait" "$DATA")"
expect "sync: deleting orphans asks first and stays off until confirmed (SY-03)" "$out" \
  "~delete=false" "~Deleted: 0 "
fixture
expect "sync: once confirmed, orphans get Delete rows (SY-03)" \
  "$(run "$SYNC,sync:delete,wait,confirm:turn on,wait,wait,sync:filter=deleted" "$DATA")" \
  "~delete=true" "~rows=[r.txt:Delete:>:on,zzz:Delete:>:on]" "~Deleted: 2 "
fixture
expect "sync: cancelling the question leaves deletion off" \
  "$(run "$SYNC,sync:delete,wait,confirm:cancel,wait" "$DATA")" "~delete=false" "~Deleted: 0 "
fi

check "sync: with every row unchecked Synchronize is disabled (SY-08)" "$SYNC,sync:uncheck=all" \
  "~:Add:>:off" "~sync=false"

check "sync: unchecking one row keeps Synchronize available" "$SYNC,sync:uncheck=five.bytes" \
  "~five.bytes:Add:>:off" "~sync=true"

if wanted "sync: a tally filters the list"; then
fixture
printf 'longer' > "$START/five.bytes"; touch -t 202001010000 "$START/five.bytes"
out="$(run "$SYNC,sync:filter=updated" "$DATA")"
expect "sync: a differing file is an Update, and the tally filters to it (SY-02, SY-09)" "$out" \
  "~rows=[five.bytes:Update:>:on] All: 9 " "~Updated: 1 (5 bytes)"
fixture
printf 'longer' > "$START/five.bytes"
expect "sync: All brings every row back (SY-09)" "$(run "$SYNC,sync:filter=updated,sync:filter=all" "$DATA")" \
  "~dangling:Add:>:on" "~five.bytes:Update:>:on"
fi

check "sync: an excluded extension appears nowhere (SY-06)" "$SYNC,sync:exclude=ext:txt,wait,wait" \
  "~rows=[dangling:Add:>:on,five.bytes:Add:>:on,folder a:Add:>:on,Folder B/deep:Add:>:on,Folder B/inside.md:Add:>:on,Folder B/zeta.png:Add:>:on]"

check "sync: a half-typed exclude rule is simply inactive" "$SYNC,sync:exclude=size:,wait,sync:exclude=size:abc,wait,wait" \
  "~$PLAN"

check "sync: the offset field can be emptied and counts as 0" "$SYNC,sync:offset=60,wait,sync:offset=,wait" \
  "~offset=0“”"

check "sync: the offset stops at 1440 and ignores letters" "$SYNC,sync:offset=99x999,wait" \
  "~offset=1440“1440”"

check "sync: both ways adds to each side and never deletes (SY-04)" "$SYNC,sync:dir=both,wait,wait" \
  "~sync: dir=BothWays" "~r.txt:Add:<:on" "~five.bytes:Add:>:on" "~Deleted: 0 "

if wanted "sync: Synchronize runs the plan"; then
fixture
out="$(run "$SYNC,sync:run,$W,wait,wait" "$DATA")"
expect "sync: Synchronize runs the plan as one activity" "$out" \
  "sync: -" "other.rows: folder a|Folder B|zzz|dangling|File1.txt|file2.txt|file10.txt|five.bytes|r.txt" \
  "~log.last: Synchronized 9 items"
out="$(perl -e 'alarm 60; exec @ARGV' "$APP" "$DATA" "$START" --keys "$SYNC" --dump 2>&1)"
expect "sync: analysing again right after a sync finds nothing to do (SY-07)" "$out" \
  "~rows=[] All: 0 " "~sync=false status=The folders are in sync."
fi

# --- Compare (M6) ------------------------------------------------------------
if wanted "compare: two files"; then
fixture
printf '#!/bin/sh\nprintf "%%s\\n" "$@" > "%s/compared"\n' "$BASE" > "$BASE/tool"; chmod +x "$BASE/tool"
RUSTYFINDER_COMPARE_TOOL="$BASE/tool" run "type:file2,shift+down,menu:Compare…,$W" "$DATA" >/dev/null
expect "compare: two files in one pane go to the compare tool" "$(cat "$BASE/compared" 2>&1)" \
  "$DATA/file2.txt" "$DATA/file10.txt"
fixture
printf '#!/bin/sh\nprintf "%%s\\n" "$@" > "%s/compared"\n' "$BASE" > "$BASE/tool"; chmod +x "$BASE/tool"
RUSTYFINDER_COMPARE_TOOL="$BASE/tool" run "type:five,tab,type:r,tab,menu:Compare…,$W" "$DATA" >/dev/null
expect "compare: one file in each pane works too" "$(cat "$BASE/compared" 2>&1)" \
  "$DATA/five.bytes" "$START/r.txt"
fixture
expect "compare: a missing tool is explained (SY-13)" \
  "$(RUSTYFINDER_COMPARE_TOOL="$BASE/none" run "type:file2,shift+down,menu:Compare…,wait" "$DATA")" \
  "~problem: FileMerge isn’t available." "~Install Xcode"
fixture
printf '{"starting_directory":"%s","play_sounds":false,"compare_tool":"kaleidoscope"}' "$START" > "$RUSTYFINDER_SUPPORT_DIR/settings.json"
expect "compare: the tool chosen in Settings is used, and its absence explained (§7)" \
  "$(RUSTYFINDER_COMPARE_TOOL="$BASE/none" run "type:file2,shift+down,menu:Compare…,wait" "$DATA")" \
  "~problem: Kaleidoscope isn’t available." "~ksdiff" "~compare=kaleidoscope"
fixture
expect "compare: the choice is made in Settings (§7)" "$(run "comparetool:beyondcompare,wait" "$DATA")" \
  "~tools.inheritpath: on compare=beyondcompare"
fixture
printf '#!/bin/sh\nprintf "%%s\\n" "$@" > "%s/compared"\n' "$BASE" > "$BASE/tool"; chmod +x "$BASE/tool"
RUSTYFINDER_COMPARE_TOOL="$BASE/tool" run "type:file2,shift+down,toolbarclick:Compare,$W" "$DATA" >/dev/null
expect "compare: the toolbar Compare button compares (§7)" "$(cat "$BASE/compared" 2>&1)" \
  "$DATA/file2.txt" "$DATA/file10.txt"
fixture
printf '#!/bin/sh\nprintf "%%s\\n" "$@" > "%s/compared"\n' "$BASE" > "$BASE/tool"; chmod +x "$BASE/tool"
RUSTYFINDER_COMPARE_TOOL="$BASE/tool" run "cmd+a,menu:Compare…,wait,down,shift+down,menu:Compare…,$W" "$DATA" >/dev/null
expect "compare: needs exactly two files, folders don't count" "$([ -e "$BASE/compared" ] && echo ran || echo idle)" "idle"
fi

# --- Search and filter (M7) --------------------------------------------------
check "search: typing filters the folder as you type (SE-01)" "search:five" \
  "rows: five.bytes" "status: Filtered: 1" "~searchfield: text=five criterion=Name" "search: -"

check "search: the filter ignores case and accents" "search:FILE1" \
  "rows: File1.txt|file10.txt" "status: Filtered: 2"

check "search: Esc ends the filter and keeps the selection" "search:five,searchkey:escape,wait" \
  "$LISTING" "selected: five.bytes" "~searchfield: text= "

check "search: emptying the field ends the filter" "search:five,search:,wait" "$LISTING"

check "search: a filter stays in the folder it was typed in" "search:folder,wait,down,cmd+down,$W" \
  "path: $DATA/Folder B" "rows: deep|inside.md|zeta.png" "~searchfield: text= "

check "search: Return searches subfolders and lists where each match is (SE-02)" \
  "searchopt:subfolders,search:zeta,searchkey:return,$W" \
  "rows: Folder B/zeta.png" "status: 1 of 1 found selected" "title: Search: “zeta”" \
  "left.tabs: *Search: “zeta”" "search: zeta running=false note=-" "path: $DATA"

check "search: Back returns from the results to the folder (SE-02)" \
  "searchopt:subfolders,search:zeta,searchkey:return,$W,cmd+[,$W" \
  "$LISTING" "search: -" "title: left" "~searchfield: text= "

check "search: Esc returns from the results to the folder" \
  "searchopt:subfolders,search:zeta,searchkey:return,$W,searchkey:escape,$W" "$LISTING" "search: -"

check "search: with subfolders off nothing is searched until Return says so" \
  "searchopt:subfolders,search:zeta" "$LISTING" "search: -"

check "search: by content, in this folder only (SE-04)" \
  "searchopt:criterion:Content,search:hell,searchkey:return,$W" \
  "rows: five.bytes" "search: hell running=false note=-"

check "search: by extension (SE-05)" \
  "searchopt:criterion:Extension,searchopt:subfolders,search:png,searchkey:return,$W" \
  "rows: Folder B/zeta.png"

check "search: by kind" "searchopt:criterion:Kind,searchopt:subfolders,search:folder,searchkey:return,$W" \
  "rows: Folder B/deep|folder a|Folder B" "selected: Folder B/deep"

check "search: a regular expression that is broken is explained" \
  "searchopt:regex,search:(,searchkey:return,$W" "~note=The regular expression isn’t valid"

check "search: Spotlight with a quote in the term does no harm (SE-06)" \
  "searchopt:engine:spotlight,search:he said \"hi\",searchkey:return,$W" \
  "~search: he said \"hi\" running=false" "problem: -"

check "search: a result can be copied to the other pane" \
  "searchopt:subfolders,search:zeta,searchkey:return,$W,f5,$W" \
  "other.rows: Folder B|zzz|r.txt|zeta.png" "log.last: Copied 1 item [success]"

if wanted "search: a spinner and a Stop button"; then
fixture
# Enough folders that the walk is still going when the dump is taken.
python3 -c "
import os
for i in range(250):
    for j in range(200): os.makedirs(f'$DATA/many/d{i}/e{j}')
"
expect "search: a spinner and a Stop button show while a search runs (§1)" \
  "$(run "searchopt:subfolders,search:nomatch,searchkey:return" "$DATA")" \
  "search.progress: running" "~running=true"
expect "search: Stop ends it and keeps the results (§1)" \
  "$(run "searchopt:subfolders,search:nomatch,searchkey:return,searchstop,$W" "$DATA")" \
  "search.progress: idle" "search: nomatch running=false note=Search stopped." "left.tabs: *Search: “nomatch”"
fi

check "search: Sync is unavailable while results are on show (SY-12)" \
  "searchopt:subfolders,search:zeta,searchkey:return,$W,alt+cmd+shift+s,wait" "sync: -"

check "search: opening a folder from the results goes to that folder" \
  "searchopt:criterion:Kind,searchopt:subfolders,search:folder,searchkey:return,$W,cmd+down,$W" \
  "path: $DATA/Folder B/deep" "search: -"

sftp "search: on a server Spotlight hands over to RustyFinder's own search (SE-03)" \
  "$C,searchopt:engine:spotlight,searchopt:subfolders,search:hello,searchkey:return,$W,$W" \
  "rows: hello.txt" "~note=Spotlight can’t answer this here" "conn: remote"

# --- Archives (M7) -----------------------------------------------------------
# a.zip holds docs/readme.txt and top.txt; one.zip holds only docs/.
arc_fixture() {
  fixture
  mkdir -p "$BASE/pack/docs"
  printf 'read me' > "$BASE/pack/docs/readme.txt"; printf 'top' > "$BASE/pack/top.txt"
  (cd "$BASE/pack" && zip -q -r "$DATA/a.zip" docs top.txt && zip -q -r "$DATA/one.zip" docs)
}
# arc <name> <keys> <expected...>
ARC_RAN=""
arc() {
  local name="$1" keys="$2"; shift 2
  ARC_RAN=""
  wanted "$name" || return 0
  arc_fixture
  ARC_RAN=1
  expect "$name" "$(run "$keys" "$DATA")" "$@"
}
# holds <what> <file> <expected content>: checked after the arc check before it.
holds() {
  [ -n "$ARC_RAN" ] || return 0
  if [ "$(cat "$2" 2>/dev/null)" = "$3" ]; then
    pass=$((pass + 1)); echo "ok    $1"
  else
    echo "FAIL  $1: got '$(cat "$2" 2>&1)'"; fail=1
  fi
}
ZIP="type:a.zip"

arc "archive: Cmd-Down browses a zip like a folder (AR-01)" "$ZIP,cmd+down,$W" \
  "conn: archive" "path: /" "rows: docs|top.txt" "subtitle: a.zip:/" "back: true forward: false"

arc "archive: folders inside open, and Cmd-Up at the top leaves with the archive selected (AR-01)" \
  "$ZIP,cmd+down,$W,cmd+down,$W,cmd+up,$W,cmd+up,$W" \
  "conn: local" "path: $DATA" "selected: a.zip"

arc "archive: a file is copied out with F5 (AR-02)" "$ZIP,cmd+down,$W,type:top,f5,$W" \
  "other.rows: Folder B|zzz|r.txt|top.txt" "log.last: Copied 1 item [success]"
holds "archive: the copied file has the archived content" "$START/top.txt" "top"

arc "archive: a search result inside a zip is copied out (SE-08)" \
  "$ZIP,cmd+down,$W,searchopt:subfolders,search:readme,searchkey:return,$W,f5,$W" \
  "rows: docs/readme.txt" "other.rows: Folder B|zzz|r.txt|readme.txt" "log.last: Copied 1 item [success]"
holds "search: the file copied from the archive's results has its content (SE-08)" "$START/readme.txt" "read me"

arc "archive: preview and Info work inside an archive (§1)" "$ZIP,cmd+down,$W,type:top,cmd+shift+p,wait,wait,wait" \
  "preview.kind: text" "preview.text: top" "info: top.txt" "info: Inside an archive (read-only)"

arc "archive: Quick Look works inside an archive (PV-12)" "$ZIP,cmd+down,$W,type:top,wait,wait,wait,wait,wait,space,wait,wait,wait" \
  "quicklook: visible" "ql.items: top.txt"

arc "archive: an item is dragged out of an archive (§1)" "$ZIP,cmd+down,$W,type:docs,dragout:$START,$W" \
  "dragout: wrote docs"
holds "archive: the dragged-out folder has its file" "$START/docs/readme.txt" "read me"

if wanted "archive: icons inside an archive"; then
  arc_fixture
  python3 -c "
import zlib, struct, sys
def chunk(t, d): return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
raw = b''.join(b'\\x00' + b'\\x20\\x80\\xff' * 64 for _ in range(64))
open(sys.argv[1], 'wb').write(b'\\x89PNG\\r\\n\\x1a\\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 64, 64, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b''))
" "$BASE/pack/pic.png"
  (cd "$BASE/pack" && zip -q "$DATA/pics.zip" pic.png top.txt)
  expect "archive: icons inside an archive show thumbnails (§1)" \
    "$(run "cmd+1,wait,type:pics,cmd+down,$W,wait,wait,wait,wait,wait" "$DATA")" "~grid.thumbs: pic.png"
fi

arc "archive: nothing can be changed inside" "$ZIP,cmd+down,$W,cmd+shift+n,type:top,cmd+delete,f6,$W" \
  "rows: docs|top.txt" "other.rows: Folder B|zzz|r.txt" "confirm: -"

arc "archive: Extract puts several items in a folder named after the archive (AR-03)" "$ZIP,cmd+shift+x,$W" \
  "rows: a|folder a|Folder B|a.zip|dangling|File1.txt|file2.txt|file10.txt|five.bytes|one.zip" \
  "selected: a" "log.last: Extracted “a.zip” [success]" "undo: Undo Extract" "connections: "
holds "archive: the extracted file has the right content" "$DATA/a/docs/readme.txt" "read me"

arc "archive: a single top-level item is extracted as itself" "type:one,cmd+shift+x,$W" \
  "~rows: docs|folder a|" "selected: docs"
holds "archive: and its content is there" "$DATA/docs/readme.txt" "read me"

arc "archive: Extract to puts it in the other pane's folder" "$ZIP,alt+cmd+shift+x,$W" \
  "other.rows: a|Folder B|zzz|r.txt"
holds "archive: extracted into the other pane" "$START/a/top.txt" "top"

arc "archive: extracting onto an existing folder asks, like any folder conflict" \
  "touch:$DATA/docs,rm:$DATA/docs,type:one,cmd+shift+x,$W,type:one,cmd+shift+x,$W" \
  "prompt: conflict folderFolder docs"

check "archive: Compress zips the selection and selects the archive (AR-05)" \
  "type:file2,shift+down,cmd+shift+m,$W" \
  "selected: Archive.zip" "log.last: Compressed 2 items [success]" "undo: Undo Compress"

check "archive: a second Compress does not reuse the name (AR-05)" \
  "type:file2,shift+down,cmd+shift+m,$W,type:file2,shift+down,cmd+shift+m,$W" \
  "selected: Archive 2.zip" "~|Archive 2.zip|Archive.zip|"

check "archive: the toolbar Compress button compresses the selection (§4)" "type:five,toolbarclick:Compress,$W" \
  "selected: five.zip" "log.last: Compressed “five.bytes” [success]"

check "archive: one item gives the archive its name (AR-07)" "type:five,cmd+shift+m,$W" \
  "selected: five.zip"

check "archive: a compressed folder opens with its contents" \
  "down,cmd+shift+m,$W,cmd+down,$W,cmd+down,$W" "conn: archive" "path: /Folder B" "rows: deep|inside.md|zeta.png"

check "archive: Compress with Options follows the format" "type:five,alt+cmd+shift+m,wait,compress:format=1" \
  "compresssheet: name=five.tar.gz format=Tar + Gzip password=disabled"

check "archive: Compress with Options makes the chosen format" \
  "type:five,alt+cmd+shift+m,wait,compress:format=1,confirm:compress,$W" "selected: five.tar.gz"

check "archive: a password-protected zip asks, and Cancel leaves it closed (AR-06, AR-08)" \
  "type:five,alt+cmd+shift+m,wait,compress:password=pw,confirm:compress,$W,cmd+down,$W" \
  "selected: five.zip" "conn: local" "archive.prompt: Enter password for the archive “five.zip”."

check "archive: a wrong password asks again, the right one opens it (AR-08)" \
  "type:five,alt+cmd+shift+m,wait,compress:password=pw,confirm:compress,$W,archivepw:bad,archivepw:pw,cmd+down,$W,wait" \
  "conn: archive" "rows: five.bytes" \
  "archive.prompt: The password is not correct. Enter password for the archive “five.zip”."

check "archive: 7z takes a password in Compress with Options (§4)" \
  "type:five,alt+cmd+shift+m,wait,compress:format=4" "~compresssheet: name=five.7z format=7-Zip password=enabled"

check "archive: a 7z made with a password asks for it, and opens with it (§2, §4)" \
  "type:five,alt+cmd+shift+m,wait,compress:format=4,compress:password=pw,confirm:compress,$W,archivepw:bad,archivepw:pw,cmd+down,$W,wait" \
  "conn: archive" "rows: five.bytes" \
  "archive.prompt: The password is not correct. Enter password for the archive “five.7z”."

if wanted "archive: a RAR with a password"; then
fixture
cp "$ROOT/crates/rustyfinder-core/testdata/crypted.rar" "$DATA/"
expect "archive: a RAR with a password asks for it, and opens with it (§2)" \
  "$(run "type:crypted,archivepw:unrar,cmd+down,$W,wait" "$DATA")" \
  "conn: archive" "~1 hidden" "archive.prompt: Enter password for the archive “crypted.rar”."
fi

if wanted "archive: a lone .xz file"; then
fixture
printf 'one file\n' | python3 -c 'import lzma,sys; sys.stdout.buffer.write(lzma.compress(sys.stdin.buffer.read()))' > "$DATA/notes.txt.xz"
expect "archive: a lone .xz file opens as a folder holding the file (§1)" "$(run "type:notes,cmd+down,$W,wait" "$DATA")" \
  "conn: archive" "rows: notes.txt"
fi

if wanted "archive: a file that is not an archive"; then
fixture
printf 'not an archive at all' > "$DATA/junk.zip"
expect "archive: a file that is not an archive says so and stays closed" "$(run "type:junk,cmd+down,$W" "$DATA")" \
  "conn: local" "archive.problem: “junk.zip” can’t be opened. Unrecognized archive format."
fi

if wanted "archive: a tar.gz is browsed"; then
arc_fixture
(cd "$BASE/pack" && tar -czf "$DATA/t.tar.gz" docs top.txt)
expect "archive: a tar.gz is browsed too" "$(run "type:t.tar,cmd+down,$W,wait" "$DATA")" \
  "conn: archive" "rows: docs|top.txt"
fi

if wanted "archive: an archive on a server"; then
  if sshd_start; then
    arc_fixture
    mkdir -p "$REMOTE"; rm -rf "$REMOTE"/*
    (cd "$BASE/pack" && tar -czf "$REMOTE/t.tar.gz" docs top.txt)
    expect "archive: an archive on a server is fetched, then browsed (AR-10)" \
      "$(run "$C,type:t.tar,cmd+down,$W,$W" "$DATA")" "conn: archive" "rows: docs|top.txt" "subtitle: t.tar.gz:/"
  else
    echo "skip  archive: an archive on a server (sshd could not be started here)"
  fi
fi

# --- Tools, checksum, Terminal, git (M7) -------------------------------------
# tool_json <id> <name> <command> <show_output> <per_file> [shortcut]
tool_json() {
  python3 -c 'import json,sys; a=sys.argv; print(json.dumps({"id":int(a[1]),"name":a[2],"command":a[3],"show_output":a[4]=="true","per_file":a[5]=="true","shortcut":a[6] if len(a)>6 else "","in_toolbar":False,"in_context_menu":True}))' "$@"
}
tools_fixture() {
  fixture
  touch "$DATA/a b.txt" "$DATA/c\"d.txt"
  printf 'hello\n' > "$DATA/hello"
  printf '[%s,%s,%s,%s,%s]' \
    "$(tool_json 1 Names 'echo $SOURCE_SELECTION_NAMES' true false ctrl+alt+t)" \
    "$(tool_json 2 Each 'echo one:$SOURCE_SELECTION_NAMES' true true)" \
    "$(tool_json 3 Fail 'echo oops; exit 2' true false)" \
    "$(tool_json 4 Quiet 'exit 3' false false)" \
    "$(tool_json 5 Where 'echo $SOURCE_PATH $TARGET_PATH' true false)" > "$RUSTYFINDER_SUPPORT_DIR/tools.json"
}
# tool <name> <keys> <expected...>
tool() {
  local name="$1" keys="$2"; shift 2
  wanted "$name" || return 0
  tools_fixture
  expect "$name" "$(run "$keys" "$DATA")" "$@"
}
TWO_ODD="type:a b,shift+down"   # "a b.txt" and c"d.txt

tool "tools: names with spaces and quotes arrive as whole arguments (TL-01)" "$TWO_ODD,menu:Names,$W" \
  'tool.output: a b.txt c"d.txt⏎' "tool.windows: 1" "log.last: Ran “Names” [success]"

tool "tools: a tool's keyboard shortcut runs it" "$TWO_ODD,ctrl+alt+t,$W" 'tool.output: a b.txt c"d.txt⏎'

tool "tools: run for every selected file runs once per item (TL-02)" \
  "type:File1,shift+down,shift+down,menu:Each,$W" \
  "tool.output: one:File1.txt⏎one:file2.txt⏎one:file10.txt⏎"

tool "tools: a non-zero exit shows its code in the output (TL-04)" "menu:Fail,$W" \
  "tool.output: oops⏎Task exit code: 2⏎" "tool.windows: 1" "~finish.last: Running “Fail” [failed]"

tool "tools: with output hidden a failure is an alert" "menu:Quiet,$W" \
  "tool.windows: 0" "~Task exit code: 3"

tool "tools: the pane folders fill the path placeholders" "menu:Where,$W" "tool.output: $DATA $START⏎"

tool "tools: Esc while creating a tool adds nothing (TL-05)" "menu:Edit Tools…,wait,tool:add,wait,tool:escape" \
  "~toolswindow: Names|Each|Fail|Quiet|Where selected="

tool "tools: a new tool is a draft until it has a command" "menu:Edit Tools…,wait,tool:add,tool:name=Fresh" \
  "~toolswindow: Names|Each|Fail|Quiet|Where|Fresh(draft) selected=5"

if wanted "tools: a saved tool is there after a relaunch"; then
tools_fixture
run "menu:Edit Tools…,wait,tool:add,tool:name=Fresh,tool:command=echo hi there" "$DATA" >/dev/null
expect "tools: a saved tool is there after a relaunch, in the menu" "$(run "menu:Fresh,$W" "$DATA")" \
  "tool.output: hi there⏎"
fi

tool "tools: renaming a tool renames it in the list at once" \
  "menu:Edit Tools…,wait,tool:select=1,tool:name=Every" "~toolswindow: Names|Every|Fail|Quiet|Where selected=1"

tool "tools: Show in toolbar adds a button that runs the tool (§1.1)" \
  "$TWO_ODD,menu:Edit Tools…,wait,tool:select=0,tool:toolbar,wait,toolbarclick:Names,$W" \
  "~toolbar: Sidebar|Back|Forward|Enclosing Folder|Copy to|Move to|Delete|Compress|Sync|Compare|Names|Activities|" \
  'tool.output: a b.txt c"d.txt⏎'

if wanted "tools: a toolbar tool is there after a relaunch"; then
tools_fixture
run "menu:Edit Tools…,wait,tool:select=4,tool:toolbar,wait" "$DATA" >/dev/null
expect "tools: a toolbar tool is there after a relaunch" "$(run "wait" "$DATA")" "~|Compare|Where|Activities|"
expect "tools: a toolbar tool leaves when unticked" \
  "$(run "menu:Edit Tools…,wait,tool:select=4,tool:toolbar,wait" "$DATA")" \
  "~toolbar: Sidebar|Back|Forward|Enclosing Folder|Copy to|Move to|Delete|Compress|Sync|Compare|Activities|"
fi

if wanted "tools: a clicked placeholder"; then
tools_fixture
run 'menu:Edit Tools…,wait,tool:add,tool:name=Here,tool:command=echo at:,tool:placeholder=$SOURCE_PATH' "$DATA" >/dev/null
expect "tools: a clicked placeholder goes into the command (§1.1)" "$(run "menu:Here,$W" "$DATA")" \
  "tool.output: at:$DATA⏎"
fi

tool "tools: a shortcut another tool has is named and nothing changes (§1.1)" \
  "menu:Edit Tools…,wait,tool:select=1,tool:shortcut=alt+ctrl+t,wait" \
  "~shortcut=- conflict=tool:1"

tool "tools: Reassign moves a tool's shortcut to this tool (§1.1)" \
  "menu:Edit Tools…,wait,tool:select=1,tool:shortcut=alt+ctrl+t,wait,confirm:reassign,wait,type:File1,ctrl+alt+t,$W" \
  "~shortcut=ctrl+alt+t conflict=-" "tool.output: one:File1.txt⏎"

tool "tools: a shortcut a menu command has is named too (§1.1)" \
  "menu:Edit Tools…,wait,tool:select=1,tool:shortcut=cmd+shift+n,wait" "~conflict=File/New Folder"

tool "shortcuts: tools are listed in Settings and checked against each other (KS-06)" \
  "menu:Settings…,wait,settings:tab=Shortcuts,settings:bind=tool:2=ctrl+alt+t,wait" "~conflict=tool:2 wants ctrl+alt+t held by tool:1"

tool "shortcuts: a tool shortcut set in Settings runs the tool (KS-06)" \
  "type:File1,menu:Settings…,wait,settings:tab=Shortcuts,settings:bind=tool:2=ctrl+alt+e,wait,ctrl+alt+e,$W" \
  "tool.output: one:File1.txt⏎"

tool "shortcuts: a menu command cannot silently take a tool's shortcut (KS-06)" \
  "menu:Settings…,wait,settings:tab=Shortcuts,settings:bind=File/New File=ctrl+alt+t,wait" "~conflict=File/New File wants ctrl+alt+t held by tool:1"

if wanted "tools: Inherit PATH"; then
tools_fixture
printf '[%s]' "$(tool_json 1 ShowPath 'echo $PATH' true false)" > "$RUSTYFINDER_SUPPORT_DIR/tools.json"
expect "tools: Inherit \$PATH can be turned off in Settings (SD §4)" \
  "$(run "menu:Settings…,wait,settings:tab=General,settingsclick:tools_inherit_path,wait,menu:ShowPath,$W" "$DATA")" \
  "~tools.inheritpath: off" "tool.output: /usr/bin:/bin:/usr/sbin:/sbin⏎"
fi

tool "checksum: SHA-256 of a known file (TL-06)""type:hello,menu:Checksum…,$W" \
  "checksum: hello=5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03 algo=SHA-256"

tool "checksum: another algorithm recomputes" "type:hello,menu:Checksum…,wait,checksum:algo=md5,$W" \
  "checksum: hello=b1946ac92492d2347c6235b4d2611184 algo=MD5"

tool "checksum: a pasted digest is matched whatever its case and spacing" \
  "type:hello,menu:Checksum…,$W,checksum:compare= 5891B5B522D5DF08 6D0FF0B110FBD9D21BB4FC7163AF34D08286A2E846F6BE03" \
  "~(match)"

if wanted "checksum: clipboard"; then
tools_fixture
run "type:hello,shift+up,menu:Checksum…,$W,checksum:copy" "$DATA" >/dev/null
expect "checksum: clipboard gets one CSV line per row (TL-07)" "$(pbpaste)" \
  "hello,SHA-256,5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03" \
  "~five.bytes,SHA-256,"
fi

check "terminal: Cmd-Shift-R opens the Terminal app at the selected folder (TL-10)" "cmd+shift+r" \
  "external: Terminal.app <- $DATA/folder a"

check "terminal: with a file selected it opens at the pane's folder" "type:five,cmd+shift+r" \
  "external: Terminal.app <- $DATA"

# A repository with one committed file that is then changed.
git_fixture() {
  fixture
  git -C "$DATA" init -q
  git -C "$DATA" config user.name Tester; git -C "$DATA" config user.email tester@example.invalid
  git -C "$DATA" config commit.gpgsign false
  git -C "$DATA" add File1.txt && git -C "$DATA" commit -q -m first
  printf 'changed' > "$DATA/File1.txt"
}
if wanted "git: status"; then
git_fixture
expect "git: status marks modified and untracked items (TL-08)" "$(run "menu:Show SCM Status,$W" "$DATA")" \
  "~File1.txt=M" "~file2.txt=?" "other.scm: "
fixture
expect "git: outside a repository nothing is shown" "$(run "menu:Show SCM Status,$W" "$DATA")" "scm: " "problem: -"
fi

if wanted "git: icon view"; then
git_fixture
expect "git: Icon view shows status badges (§4)" "$(run "menu:Show SCM Status,$W,cmd+1,$W,wait" "$DATA")" \
  "~File1.txt=M" "~file2.txt=?"
git_fixture
expect "git: the item context menu has Source Control (§4)" "$(run "cmd+1,$W,type:File1,ctxmenu" "$DATA")" \
  "~|Source Control[Add|Add All|Commit…|Pull|Push]|"
fi

if wanted "git: commit"; then
git_fixture
expect "git: Commit stays off while the message is empty (TL-09)" "$(run "menu:Commit…,wait" "$DATA")" \
  "commitsheet: commit=disabled"
git_fixture
expect "git: typing a message switches Commit on" "$(run "menu:Commit…,wait,commit:a message" "$DATA")" \
  "commitsheet: commit=enabled"
git_fixture
run "menu:Add All,$W,menu:Commit…,wait,commit:second commit,confirm:commit,$W" "$DATA" >/dev/null
expect "git: Add All then Commit makes the commit" "$(git -C "$DATA" log --format=%s -1; git -C "$DATA" status --porcelain | wc -l | tr -d ' ')" \
  "second commit" "0"
git_fixture
expect "git: a failing git command shows git's own message" "$(run "menu:Push,$W" "$DATA")" \
  "~finish.last: SCM Operation: Push [failed]" "~Task exit code:"
fi

# --- Tags (M7). The colour tag names are the English ones. ------------------
# stored_tags <file>: the tag names in the file's Finder tags attribute.
stored_tags() {
  xattr -px com.apple.metadata:_kMDItemUserTags "$1" 2>/dev/null | xxd -r -p | plutil -p - 2>/dev/null \
    | sed -n 's/.*=> "\([^\\"]*\).*/\1/p' | paste -sd+ -
}
if wanted "tags: a colour tag"; then
fixture
expect "tags: a colour tag goes on the selection and shows as a dot (TG-01)" "$(run "type:five,tag:Red,wait" "$DATA")" \
  "tags: five.bytes=[Red]" "tagdots: 1"
expect "tags: Finder's own attribute holds the tag" "$(stored_tags "$DATA/five.bytes")" "Red"
fixture
expect "tags: toggling twice leaves no tag (TG-01)" "$(run "type:five,tag:Red,tag:Red,wait" "$DATA")" \
  "tags: five.bytes=[]" "tagdots: 0"
expect "tags: and the attribute is empty again" "[$(stored_tags "$DATA/five.bytes")]" "[]"
fi

check "tags: the Tags field adds a tag to every selected item (TG-02)" \
  "type:File1,shift+down,shift+down,tagsedit:Work,wait" \
  "tags: File1.txt=[Work] file2.txt=[Work] file10.txt=[Work]"

check "tags: adding a tag an item already has keeps one (TG-03)" "type:five,tag:Blue,tagsedit:Blue+blue,wait" \
  "tags: five.bytes=[Blue]"

check "tags: on a mixed selection a toggle adds, a second one removes" \
  "type:File1,tag:Red,shift+down,tag:Red,wait" "tags: File1.txt=[Red] file2.txt=[Red]"

check "tags: removing a common tag leaves each item's other tags" \
  "type:File1,tag:Red,shift+down,tag:Blue,tagsedit:,wait" "tags: File1.txt=[Red] file2.txt=[]"

check "tags: at most three dots are shown (VW-13)" \
  "type:five,tag:Red,tag:Blue,tag:Green,tag:Yellow,tag:Purple,wait" \
  "tags: five.bytes=[Red+Blue+Green+Yellow+Purple]" "tagdots: 3"

if wanted "tags: an old-style colour label"; then
fixture
# Finder info with label 7 (orange) and no tags attribute.
xattr -wx com.apple.FinderInfo "0000000000000000000E00000000000000000000000000000000000000000000" "$DATA/five.bytes"
expect "tags: an old-style colour label shows as that colour (TG-08)" "$(run "type:five" "$DATA")" \
  "tags: five.bytes=[Orange]" "tagdots: 1"
fi

check "tags: a tag listing is a Spotlight search of the home folder" "showtag:rustyfinder-no-such-tag,$W" \
  "title: Search: “rustyfinder-no-such-tag”" "path: $HOME" "search: rustyfinder-no-such-tag running=false note=-" "count: 0"

sftp "tags: on a server there are no tags to set" "$C,type:hello,tag:Red,wait" "tags: " "problem: -" "conn: remote"

# --- FTP (M7), against the scripted server from the Rust tests ---------------
FTPD="$ROOT/target/debug/examples/ftpd"
FTPD_PID=""
# ftp <name> <keys with PORT for the port> <expected...>: fresh folders and
# a fresh server holding docs/ and hello.txt, then the keys.
FTP_RAN=""
ftp() {
  local name="$1" keys="$2"; shift 2
  FTP_RAN=""
  wanted "$name" || return 0
  if [ ! -x "$FTPD" ] && ! (cd "$ROOT" && cargo build -q -p rustyfinder-remote --example ftpd 2>/dev/null); then
    echo "skip  $name (the test FTP server could not be built)"
    return 0
  fi
  fixture
  FTP_ROOT="$BASE/ftp"
  mkdir -p "$FTP_ROOT/docs"
  printf 'remote hello' > "$FTP_ROOT/hello.txt"
  "$FTPD" "$FTP_ROOT" > "$BASE/ftp.port" &
  FTPD_PID=$!
  disown "$FTPD_PID" 2>/dev/null   # no "Terminated" line when it is killed
  for _ in $(seq 1 50); do [ -s "$BASE/ftp.port" ] && break; sleep 0.1; done
  local port; port="$(cat "$BASE/ftp.port")"
  FTP_RAN=1
  expect "$name" "$(run "${keys//PORT/$port}" "$DATA")" "$@"
  kill "$FTPD_PID" 2>/dev/null; FTPD_PID=""
}
ftp_holds() {
  [ -n "$FTP_RAN" ] || return 0
  if [ "$(cat "$2" 2>/dev/null)" = "$3" ]; then
    pass=$((pass + 1)); echo "ok    $1"
  else
    echo "FAIL  $1: got '$(cat "$2" 2>&1)'"; fail=1
  fi
}
F="connect:ftp://bob:secret@127.0.0.1:PORT/,wait,wait,wait,wait"

ftp "ftp: connecting shows the server's folder" "$F" \
  "conn: remote" "path: /" "rows: docs|hello.txt" "subtitle: bob@127.0.0.1:/" "problem: -"

ftp "ftp: F5 uploads the selection" "tab,$F,tab,type:five,f5,$W,$W" \
  "other.rows: docs|five.bytes|hello.txt" "log.last: Copied 1 item [success]"
ftp_holds "ftp: the uploaded file has the right bytes" "$BASE/ftp/five.bytes" "hello"

ftp "ftp: F5 downloads to the local pane" "$F,type:hello,f5,$W,$W" \
  "other.rows: Folder B|zzz|hello.txt|r.txt" "log.last: Copied 1 item [success]"
ftp_holds "ftp: the downloaded file has the right bytes" "$START/hello.txt" "remote hello"

ftp "ftp: rename, new folder and delete work on the server" \
  "$F,type:hello,return,type:renamed,return,$W,cmd+shift+n,$W,escape,type:docs,cmd+delete,wait,confirm:delete,$W,$W" \
  "rows: untitled folder|renamed.txt"

ftp "ftp: a folder is uploaded with its contents" "tab,$F,tab,down,f5,$W,$W" \
  "other.rows: docs|Folder B|hello.txt"
ftp_holds "ftp: files inside the uploaded folder arrived" "$BASE/ftp/Folder B/inside.md" ""

ftp "ftp: a wrong password is explained" "connect:ftp://bob:wrong@127.0.0.1:PORT/,wait,wait,wait,wait" \
  "conn: local" "~The server refused the user name or password."

check "ftp: a pasted FTP URL fills the Connect panel and picks the protocol" \
  "cmd+k,wait,connectfill:ftp://bob@example.org:2121/pub" \
  "~connectsheet: protocol=FTP server=example.org user=bob port=2121 path=/pub"

check "ftp: an FTPS URL picks FTP with TLS" "cmd+k,wait,connectfill:ftps://files.example.org" \
  "~connectsheet: protocol=FTP with TLS server=files.example.org"

# --- Servers: items, certificates, the Connect panel --------------------------
ftp "remote: Copy Path gives the URL without the password, and the server path" \
  "$F,type:hello,remote:copyurl,wait" "~remote.copied: ftp://bob@127.0.0.1:" "~/hello.txt"
ftp "remote: Copy Path can give the plain server path" "$F,type:hello,remote:copypath,wait" \
  "remote.copied: /hello.txt"
ftp "remote: Open With downloads the file first, then opens it with the app" \
  "$F,type:hello,remote:openwith:TextEdit,$W,$W" "remote.opened: hello.txt with TextEdit.app"
ftp "remote: a remote item dropped on another app is downloaded there (file-ops §3)" \
  "$F,type:hello,remote:drop:$START,$W,$W" "remote.drop: ok hello.txt"
ftp_holds "remote: the dropped file has the server's bytes" "$START/hello.txt" "remote hello"

if wanted "remote: an edited file that cannot go back"; then
if [ -x "$FTPD" ] || (cd "$ROOT" && cargo build -q -p rustyfinder-remote --example ftpd 2>/dev/null); then
fixture
mkdir -p "$BASE/ftp"; printf 'remote hello' > "$BASE/ftp/hello.txt"
"$FTPD" "$BASE/ftp" > "$BASE/ftp.port" & pid=$!; disown "$pid" 2>/dev/null
for _ in $(seq 1 50); do [ -s "$BASE/ftp.port" ] && break; sleep 0.1; done
port="$(cat "$BASE/ftp.port")"
# The server takes nothing new: the upload of the saved file must fail.
chmod 555 "$BASE/ftp"
out="$(run "connect:ftp://bob:secret@127.0.0.1:$port/,wait,wait,wait,wait,type:hello,cmd+down,$W,$W,remoteedit:hello.txt=changed,$W,$W,$W,$W" "$DATA")"
chmod 755 "$BASE/ftp"
expect "remote: an edited file that cannot go back raises Remote Editing Error with Retry (file-ops §7)" "$out" \
  "remote.editing: hello.txt" "~remote.editerror: hello.txt: "
kill "$pid" 2>/dev/null
fi
fi

if wanted "ftp: an FTPS certificate"; then
if [ -x "$FTPD" ] || (cd "$ROOT" && cargo build -q -p rustyfinder-remote --example ftpd 2>/dev/null); then
fixture
mkdir -p "$BASE/ftps"; printf 'secret' > "$BASE/ftps/tls.txt"
"$FTPD" "$BASE/ftps" explicit > "$BASE/ftps.port" & pid=$!; disown "$pid" 2>/dev/null
for _ in $(seq 1 50); do [ -s "$BASE/ftps.port" ] && break; sleep 0.1; done
port="$(cat "$BASE/ftps.port")"
FS="connect:ftps://bob:secret@127.0.0.1:$port/,wait,wait,wait,wait"
expect "ftp: an FTPS certificate that can't be verified is a question, Cancel connects nothing (FT-03)" \
  "$(run "$FS,confirm:cancel,wait,wait" "$DATA")" "conn: local" \
  "connect.decision: The certificate of “127.0.0.1” can’t be verified -> cancel"
expect "ftp: Always Trust connects over TLS" "$(run "$FS,confirm:always trust,wait,wait,wait,wait" "$DATA")" \
  "conn: remote" "rows: tls.txt"
expect "ftp: and the next connection does not ask again (FT-03)" "$(run "$FS,wait,wait" "$DATA")" \
  "conn: remote" "connect.decision: -"
kill "$pid" 2>/dev/null
fi
fi

ftp "connect: the panel saves the connection with name, local path and colour (Add to Favorites)" \
  "cmd+k,wait,connectfill:ftp://bob:secret@127.0.0.1:PORT/,connectset:name=Work FTP,connectset:local=$START,connectset:color=2,connectset:fav=on,connectset:go,wait,wait,wait,wait" \
  "conn: remote" "rows: docs|hello.txt"
if [ -n "$FTP_RAN" ]; then
  if grep -q '"name": "Work FTP"' "$RUSTYFINDER_SUPPORT_DIR/connections.json" 2>/dev/null \
     && grep -q '"color": "#FF9500"' "$RUSTYFINDER_SUPPORT_DIR/connections.json" \
     && grep -q '"passive": true' "$RUSTYFINDER_SUPPORT_DIR/connections.json" \
     && ! grep -q 'secret' "$RUSTYFINDER_SUPPORT_DIR/connections.json"; then
    pass=$((pass + 1)); echo "ok    connect: the saved connection holds the fields, never the password"
  else
    echo "FAIL  connect: saved connection: $(cat "$RUSTYFINDER_SUPPORT_DIR/connections.json" 2>&1)"; fail=1
  fi
fi

check "connect: the panel has Name, Local Path and Color fields" \
  "cmd+k,wait,connectset:name=Site,connectset:local=/tmp,connectset:color=5" "~name=Site local=/tmp color=Blue"

check "connect: S3 offers every AWS region and a storage class (remote-s3 §1.1, §1.3)" \
  "cmd+k,wait,connectfill:s3://AKIAEXAMPLE@s3.amazonaws.com/b,connectset:storage=GLACIER" \
  "~regions=36" "~storage=GLACIER"

check "connect: SFTP and FTP offer a text encoding, WebDAV does not" \
  "cmd+k,wait,connectfill:sftp://h,wait" "~encoding=on"

check "connect: implicit TLS is an option of FTP with TLS" \
  "cmd+k,wait,connectfill:ftps://files.example.org,connectset:implicit=on" "~implicit=on"

# --- WebDAV (M7), against the scripted server from the Rust tests ------------
DAVD="$ROOT/target/debug/examples/davd"
# dav <name> <keys with PORT for the port> <expected...>
DAV_RAN=""
dav() {
  local name="$1" keys="$2"; shift 2
  DAV_RAN=""
  wanted "$name" || return 0
  if [ ! -x "$DAVD" ] && ! (cd "$ROOT" && cargo build -q -p rustyfinder-remote --example davd 2>/dev/null); then
    echo "skip  $name (the test WebDAV server could not be built)"
    return 0
  fi
  fixture
  mkdir -p "$BASE/dav/My Docs #1"
  printf 'remote hello' > "$BASE/dav/hello.txt"
  "$DAVD" "$BASE/dav" > "$BASE/dav.port" &
  local pid=$!
  disown "$pid" 2>/dev/null
  for _ in $(seq 1 50); do [ -s "$BASE/dav.port" ] && break; sleep 0.1; done
  local port; port="$(cat "$BASE/dav.port")"
  DAV_RAN=1
  expect "$name" "$(run "${keys//PORT/$port}" "$DATA")" "$@"
  kill "$pid" 2>/dev/null
}
dav_holds() {
  [ -n "$DAV_RAN" ] || return 0
  if [ "$(cat "$2" 2>/dev/null)" = "$3" ]; then
    pass=$((pass + 1)); echo "ok    $1"
  else
    echo "FAIL  $1: got '$(cat "$2" 2>&1)'"; fail=1
  fi
}
# Basic over plain HTTP: the password would go out unencrypted, so the
# user is asked first (remote-webdav §1).
V="connect:http://bob:secret@127.0.0.1:PORT/,wait,wait,confirm:send anyway,wait,wait,wait,wait"

dav "webdav: connecting shows the server's folder" "$V" \
  "conn: remote" "path: /" "rows: My Docs #1|hello.txt" "subtitle: bob@127.0.0.1:/" "problem: -" \
  "connect.decision: Send the password unencrypted? -> Send Anyway"

dav "webdav: Cancel on the unencrypted-password warning connects nothing" \
  "connect:http://bob:secret@127.0.0.1:PORT/,wait,wait,confirm:cancel,wait,wait" \
  "conn: local" "connect.decision: Send the password unencrypted? -> cancel"

dav "webdav: a folder with a space and a hash opens (WD-03)" "$V,cmd+down,$W" \
  "path: /My Docs #1" "conn: remote" "count: 0"

dav "webdav: F5 uploads the selection" "tab,$V,tab,type:five,f5,$W,$W" \
  "other.rows: My Docs #1|five.bytes|hello.txt" "log.last: Copied 1 item [success]"
dav_holds "webdav: the uploaded file has the right bytes" "$BASE/dav/five.bytes" "hello"

dav "webdav: F5 downloads to the local pane" "$V,type:hello,f5,$W,$W" \
  "other.rows: Folder B|zzz|hello.txt|r.txt"
dav_holds "webdav: the downloaded file has the right bytes" "$START/hello.txt" "remote hello"

dav "webdav: rename, new folder and delete work on the server" \
  "$V,type:hello,return,type:renamed,return,$W,cmd+shift+n,$W,escape,type:My,cmd+delete,wait,confirm:delete,$W,$W" \
  "rows: untitled folder|renamed.txt"

dav "webdav: duplicating on the server is done by the server" "$V,type:hello,cmd+d,$W,$W" \
  "rows: My Docs #1|hello copy.txt|hello.txt"
dav_holds "webdav: the duplicate has the same bytes" "$BASE/dav/hello copy.txt" "remote hello"

dav "webdav: a wrong password is explained" "connect:http://bob:wrong@127.0.0.1:PORT/,wait,wait,confirm:send anyway,wait,wait,wait,wait" \
  "conn: local" "~The server refused the user name or password."

check "webdav: a pasted URL splits into server, port and remote path (WD-01)" \
  "cmd+k,wait,connectfill:https://localhost:8443/dav/files/bob/" \
  "~connectsheet: protocol=WebDAV HTTPS server=localhost user= port=8443 path=/dav/files/bob/"

# --- S3 (M7), against the scripted server from the Rust tests ----------------
S3D="$ROOT/target/debug/examples/s3d"
# s3 <name> <keys with PORT for the port> <expected...>: a fresh server with
# bucket "photos" (hello.txt, docs/) and an empty bucket "backups".
S3_RAN=""
s3() {
  local name="$1" keys="$2"; shift 2
  S3_RAN=""
  wanted "$name" || return 0
  if [ ! -x "$S3D" ] && ! (cd "$ROOT" && cargo build -q -p rustyfinder-remote --example s3d 2>/dev/null); then
    echo "skip  $name (the test S3 server could not be built)"
    return 0
  fi
  fixture
  "$S3D" > "$BASE/s3.port" &
  local pid=$!
  disown "$pid" 2>/dev/null
  for _ in $(seq 1 50); do [ -s "$BASE/s3.port" ] && break; sleep 0.1; done
  local port; port="$(cat "$BASE/s3.port")"
  S3_RAN=1
  expect "$name" "$(run "${keys//PORT/$port}" "$DATA")" "$@"
  kill "$pid" 2>/dev/null
}
s3_holds() {
  [ -n "$S3_RAN" ] || return 0
  if [ "$(cat "$2" 2>/dev/null)" = "$3" ]; then
    pass=$((pass + 1)); echo "ok    $1"
  else
    echo "FAIL  $1: got '$(cat "$2" 2>&1)'"; fail=1
  fi
}
S="connect:s3://key:secret@127.0.0.1:PORT/,wait,wait,wait,wait,wait"
SB="connect:s3://key:secret@127.0.0.1:PORT/photos,wait,wait,wait,wait,wait"

s3 "s3: the top lists the buckets (S3-01)" "$S" \
  "conn: remote" "path: /" "rows: backups|photos" "problem: -"

s3 "s3: a bucket in the address opens inside it, folder markers are folders (S3-03)" "$SB" \
  "path: /photos" "rows: docs|hello.txt" "conn: remote"

s3 "s3: F5 uploads the selection into the bucket" "tab,$SB,tab,type:five,f5,$W,$W" \
  "other.rows: docs|five.bytes|hello.txt" "log.last: Copied 1 item [success]"

s3 "s3: F5 downloads to the local pane" "$SB,type:hello,f5,$W,$W" \
  "other.rows: Folder B|zzz|hello.txt|r.txt"
s3_holds "s3: the downloaded file has the right bytes" "$START/hello.txt" "remote hello"

s3 "s3: rename, new folder and delete work in a bucket" \
  "$SB,type:hello,return,type:renamed,return,$W,cmd+shift+n,$W,escape,type:docs,cmd+delete,wait,confirm:delete,$W,$W" \
  "rows: untitled folder|renamed.txt"

s3 "s3: a folder is uploaded with its contents, and renamed with them (S3-07)" \
  "tab,$SB,tab,down,f5,$W,$W,tab,type:Folder,return,type:Moved,return,$W,$W,cmd+down,$W" \
  "path: /photos/Moved" "rows: deep|inside.md|zeta.png"

check "s3: a pasted s3:// address picks the protocol, key and bucket" \
  "cmd+k,wait,connectfill:s3://AKIAEXAMPLE@s3.amazonaws.com/my-bucket" \
  "~connectsheet: protocol=Amazon S3 and compatible server=s3.amazonaws.com user=AKIAEXAMPLE port= path=/my-bucket"

# --- Settings and shortcuts (M8) ---------------------------------------------
SET="menu:Settings…,wait"

check "shortcuts: only the function keys are bound without a modifier" "" \
  "shortcuts.bare: Commands/Copy to=f5, Commands/Move to=f6" "shortcuts: "

check "settings: the window shows the current values (SD-01)" "$SET" \
  "~settingswindow: tab=General on=[restore_windows,treat_archives_as_folders,handle_archives,relative_dates] transfers=3 replace=ask start=${START/#$HOME/~}"

if wanted "settings: a change applies at once"; then
fixture
expect "settings: a change applies at once" "$(run "$SET,settings:toggle=always_show_tab_bar,wait" "$DATA")" \
  "tabbar: left=true right=true"
expect "settings: and is still there after a relaunch" "$(run "" "$DATA")" "tabbar: left=true right=true"
fi

check "settings: Restore Defaults puts a section back" \
  "$SET,settings:toggle=always_show_tab_bar,settings:toggle=play_sounds,settings:restore=general,wait" \
  "tabbar: left=false right=false" \
  "~on=[restore_windows,treat_archives_as_folders,handle_archives,play_sounds,relative_dates] transfers=3 replace=ask start=Home"

if wanted "settings: the transfer limit"; then
fixture
run "$SET,settings:transfers=1,wait" "$DATA" >/dev/null
expect "settings: the transfer limit is saved (SD-02)" "$(grep -o '"max_transfers": *[0-9]*' "$RUSTYFINDER_SUPPORT_DIR/settings.json")" \
  '"max_transfers": 1'
fi

if wanted "settings: a replace policy"; then
conflict_fixture
out="$(run "$SET,settings:replace=4,wait,type:five,f5,$W" "$DATA")"
expect "settings: a replace policy answers conflicts without asking" "$out" "prompt: -" "~replace=skip"
[ "$(cat "$START/five.bytes")" = "old" ] || { echo "FAIL  the Skip policy replaced the file"; fail=1; }
fi

check "settings: transfer retries, limits and the resume policy (settings §2)" \
  "$SET,settings:tab=Transfers,settings:resume=1,settings:retry=on,settings:retries=5,settings:limitdown=200,wait" \
  "~resume=resume retry=on/5 down=200 up=off"

check "settings: Restore Defaults puts the transfer settings back" \
  "$SET,settings:resume=2,settings:retry=on,settings:limitup=50,settings:restore=transfers,wait" \
  "~resume=ask retry=off/3 down=off up=off"

if wanted "terminal: the chosen Terminal application"; then
fixture
printf '{"starting_directory":"%s","play_sounds":false,"terminal_app":"/Applications/Ghostty.app"}' "$START" > "$RUSTYFINDER_SUPPORT_DIR/settings.json"
expect "terminal: the chosen Terminal application is used (SD-07)" "$(run "type:five,cmd+shift+r" "$DATA")" \
  "external: Ghostty.app <- $DATA"
fi

BIND="$SET,settings:tab=Shortcuts"
check "shortcuts: a taken combination names its holder and changes nothing (KS-02)" \
  "$BIND,settings:bind=File/New File=cmd+shift+n,wait" \
  "~conflict=File/New File wants cmd+shift+n held by File/New Folder" "shortcuts: "

check "shortcuts: Reassign moves the combination, and it then runs the new command (KS-02)" \
  "$BIND,settings:bind=File/New File=cmd+shift+n,wait,confirm:reassign,wait,cmd+shift+n,$W,escape" \
  "shortcuts: File/New Folder=, File/New File=shift+cmd+n" "~|untitled file.txt" "~conflict=-"

check "shortcuts: Cancel leaves both commands as they were" \
  "$BIND,settings:bind=File/New File=cmd+shift+n,wait,confirm:cancel,wait" "shortcuts: " "~conflict=-"

if wanted "shortcuts: a new shortcut"; then
fixture
run "$BIND,settings:bind=Commands/Compare…=ctrl+alt+c,wait" "$DATA" >/dev/null
expect "shortcuts: a new shortcut is kept across a relaunch" "$(run "" "$DATA")" \
  "shortcuts: Commands/Compare…=ctrl+alt+c"
expect "shortcuts: Restore Defaults clears every change (KS-07)" \
  "$(run "$BIND,settings:restore=shortcuts,wait" "$DATA")" "shortcuts: "
expect "shortcuts: and stays cleared" "$(run "" "$DATA")" "shortcuts: "
fi

if wanted "shortcuts: Move to Trash on Option-Delete"; then
fixture
out="$(run "$BIND,settings:bind=Commands/Move to Trash=alt+delete,wait,type:five,alt+delete,$W" "$DATA")"
expect "shortcuts: Move to Trash on Option-Delete still trashes (KS-03)" "$out" \
  "rows: folder a|Folder B|dangling|File1.txt|file2.txt|file10.txt" "confirm: -" "undo: Undo Move to Trash"
[ -f "$RUSTYFINDER_TRASH_DIR/five.bytes" ] || { echo "FAIL  the file is not in the Trash"; fail=1; }
fi

check "shortcuts: a bare Delete binding does not fire while renaming (KS-04)" \
  "$BIND,settings:bind=Commands/Delete Immediately…=delete,wait,type:five,return,wait,right,delete,delete,return,$W" \
  "~|fi.bytes|" "confirm: -"

check "shortcuts: a removed shortcut no longer runs its command" \
  "$BIND,settings:bind=Commands/Duplicate,wait,type:five,cmd+d,$W" "$LISTING"

# --- Workspaces (M7) ---------------------------------------------------------
SHAPE="cmd+t,down,cmd+down,alt+cmd+shift+p,tab,cmd+ctrl+6"
if wanted "workspace: a saved window"; then
fixture
out="$(run "$SHAPE,workspace:save=W,wait,tab,cmd+t,cmd+t,wait,workspace:open=W,$W" "$DATA")"
expect "workspace: a saved window comes back with its tabs, pins and view settings (FV-05)" "$out" \
  "left.tabs: left|*Folder B(pinned)" "right.tabs: *right" "active: right" \
  "sort: size asc=true foldersFirst=true hidden=false" "windows: 1" "workspaces: W from=1"
expect "workspace: workspaces are kept across a relaunch" "$(run "" "$DATA")" "~workspaces: W from=-"
fi

check "workspace: closing a window whose preview was never shown is safe" \
  "workspace:save=W,wait,workspace:new=W,$W,cmd+shift+w,wait" "~windows: "

check "workspace: opening in a new window leaves the current one (FV-06)" \
  "workspace:save=W,wait,workspace:new=W,$W" "windows: 2"

check "workspace: a name can be used once" "workspace:save=W,wait,workspace:save=W,wait" \
  "~problem: The name “W” is already taken."

check "workspace: Update Workspace needs a window that came from one" "menu:Update Workspace,wait" \
  "workspaces:  from=-"

# --- Network shares (M7): never mounted during these checks ------------------
check "shares: an smb address asks the system to mount the share" "connect:smb://guest@nas.local/Public,wait" \
  "external: mount smb://nas.local/Public" "~Can’t mount network drive"

check "shares: a vnc address is handed to Screen Sharing" "connect:vnc://10.0.0.5,wait" \
  "external: open vnc://10.0.0.5"

check "shares: a pasted smb address fills the Connect panel" "cmd+k,wait,connectfill:smb://bob@nas.local/Media" \
  "~connectsheet: protocol=SMB (Windows share) server=nas.local user=bob port= path=/Media"

# --- Navigation chrome: title and path bars, tab icons, tab drag and drop ---
if wanted "chrome: View menu hides the title bar and the path bar"; then
fixture
expect "chrome: View menu hides the title bar and the path bar (panes-tabs §2)" \
  "$(run "menu:Hide Title Bar,menu:Hide Path Bar" "$DATA")" "chrome: title=hidden pathbar=hidden fullscreen=false"
expect "chrome: the choice is restored with the window" "$(run "" )" "chrome: title=hidden pathbar=hidden fullscreen=false"
expect "chrome: Show Path Bar brings it back" "$(run "menu:Hide Path Bar,menu:Hide Title Bar" "$DATA")" \
  "chrome: title=shown pathbar=shown fullscreen=false"
fi

if wanted "chrome: full screen"; then
fixture
expect "chrome: full screen is saved with the window" \
  "$(run "menu:Enter Full Screen,wait,wait,wait,wait,wait,wait,wait,wait")" "~fullscreen=true"
expect "chrome: and comes back on the next launch (panes-tabs §7)" "$(run "wait,wait,wait,wait,wait,wait,wait,wait")" "~fullscreen=true"
fi

check "tabs: a tag listing shows the tag icon, a search the search icon" \
  "searchopt:subfolders,search:zeta,searchkey:return,$W,tab,showtag:rustyfinder-no-such-tag,$W" \
  "~tabicons: left=search right=tag"

check "tabs: a favorite with an icon gives its tab that icon (FV-08)" \
  "menu:New Favorite…,favsheet:name=Fav,favsheet:icon=film,favsheet:save,wait,cmd+up,openfav:Fav,$W" \
  "~tabicons: left=film " "~Favorites[" "~,Fav:path@film]"

check "tabs: a favorite's icon goes once the tab leaves its folder" \
  "menu:New Favorite…,favsheet:name=Fav,favsheet:icon=film,favsheet:save,wait,openfav:Fav,$W,cmd+up,$W" \
  "~tabicons: left=folder "

check "tabs: Open in New Window from the item menu (panes-tabs §3.1)" "type:Folder B,openwindow,$W" \
  "windows: 2"

check "tabs: dragging a tab reorders it" "cmd+t,down,alt+cmd+down,tabmove:2>left:0" \
  "left.tabs: *Folder B|left|left"

check "tabs: a tab dragged to the other pane moves there" "down,alt+cmd+down,tabmove:1>right:0" \
  "left.tabs: *left" "right.tabs: *Folder B|right"

check "tabs: files hovering over a tab open it (spring-loading)" "cmd+t,tabhover:0" \
  "left.tabs: *left|left"

if wanted "tabs: files dropped on a tab"; then
fixture
out="$(run "down,alt+cmd+down,ctrl+1,type:five,tabdrop:1,$W,$W" "$DATA")"
out+=$'\n'"in Folder B: $(ls "$DATA/Folder B" | paste -sd, -)"
expect "tabs: files dropped on a tab go into its folder" "$out" \
  "drop: move 1 -> Folder B" "in Folder B: deep,five.bytes,inside.md,zeta.png"
fi

check "tabs: folders dropped on + open one tab each (PT-08)" \
  "plusdrop:$DATA/folder a|$DATA/Folder B|$START/zzz" "left.tabs: left|folder a|Folder B|*zzz"

if wanted "path bar: items dropped on a segment"; then
fixture
out="$(run "type:Folder B,cmd+down,wait,wait,type:inside,pathdrop:-2,$W,$W" "$DATA")"
out+=$'\n'"moved up: $([ -f "$DATA/inside.md" ] && [ ! -e "$DATA/Folder B/inside.md" ] && echo yes)"
expect "path bar: items dropped on a segment go into that folder (panes-tabs §6)" "$out" \
  "drop: move 1 -> left" "moved up: yes"
fi

check "volumes: a vanished drive sends its panes to the starting directory (PT-15)" \
  "down,cmd+down,cmd+t,unmounted:$DATA,$W" \
  "path: $START" "left.tabs: right|*right" "problem: -" "~volume: left left: 2"

check "eject: a disk with two volumes asks this one or all (PT-13)" "eject:$DATA:2" \
  "~prompt=“left” is on a disk with 2 volumes."

check "eject: Option ejects all without asking (PT-13)" "eject:$DATA:alt:2" \
  "~volume: eject all left,left-other prompt=-"

check "eject: Control ejects only this one (PT-13)" "eject:$DATA:ctrl:2" \
  "~volume: eject one left prompt=-"

check "eject: panes leave the volume first (SM-05)" "down,cmd+down,eject:$DATA,$W" \
  "path: $START" "~volume: eject all left prompt=-"

# --- Opening from outside (PT-16) ---------------------------------------------
check "outside: the Reveal service selects the item in the tab already showing it (PT-16)" \
  "service:$DATA/file2.txt,wait,wait" "left.tabs: *left" "selected: file2.txt" "~outside: reveal("

check "outside: a folder not shown yet opens in a new tab" "openurl:rustyfinder://open?path=$START/zzz,$W" \
  "left.tabs: left|*zzz" "path: $START/zzz"

check "outside: a folder shown in the other pane activates that tab" "openurl:rustyfinder://open?path=$START,$W" \
  "active: right" "right.tabs: *right" "left.tabs: *left"

check "outside: a folder dropped on the Dock icon opens like a reveal" "openurl:$DATA/Folder B,$W" \
  "left.tabs: left|*Folder B"

check "outside: a favorite link opens the favorite" \
  "menu:New Favorite…,favsheet:name=Here,favsheet:save,wait,cmd+up,openurl:rustyfinder://favorite/1,wait,$W" \
  "~outside: favorite("

# --- Restored server tabs -------------------------------------------------------
if wanted "restore: a server tab"; then
fixture
printf '[{"left":{"tabs":[{"path":"/srv/www","server":"ftp://nobody@127.0.0.1:1"}],"selected":0},"right":{"tabs":[{"path":"%s"}],"selected":0},"ui":""}]' "$START" \
  > "$RUSTYFINDER_SUPPORT_DIR/session.json"
out="$(run "$W,$W")"
expect "restore: a server tab waits for its server and shows the failure with Retry (panes-tabs §7)" "$out" \
  "~pending: left=ftp://nobody@127.0.0.1:1 right=-" "~left.view=Could not reconnect to ftp://nobody@127.0.0.1:1" "~[Retry]" \
  "left.tabs: *www"
expect "restore: it is still a server tab after another relaunch" "$(run "")" "~pending: left=ftp://nobody@127.0.0.1:1 right=-"
fi

# --- Sidebar: hiding, editor, order, drops --------------------------------------
check "sidebar: Hide from Sidebar hides a group; the editor shows it again" \
  "sidebarhide:Recent Folders,sidebareditor,sidebareditor:Recent Folders,wait" \
  "~+Recent Folders" "~ Recent Folders["

if wanted "sidebar: a hidden group"; then
fixture
refute "sidebar: a hidden group stays hidden after a relaunch" \
  "$(run "sidebarhide:Recent Folders,wait" "$DATA"; run "" "$DATA")" "Recent Folders[left"
fi

if wanted "sidebar: Hide Other Groups"; then
fixture
out="$(run "sidebarhideothers:Recent Folders,wait" "$DATA")"
expect "sidebar: Hide Other Groups keeps only that group" "$out" "~sidebar: shown Recent Folders["
fi

check "sidebar: favorites can be dragged into a new order" \
  "menu:New Favorite…,favsheet:name=A,favsheet:group=G,favsheet:save,wait,menu:New Favorite…,favsheet:name=B,favsheet:group=G,favsheet:save,wait,sidebarmove:B>0,wait" \
  "~G[B:path,A:path]"

check "sidebar: dropping files on a tag tags them (TG-04)" "type:five,sidebardrop:Red,$W" \
  "tags: five.bytes=[Red]" "sidebar.drop: tag Red <- 1 editor=-"

if wanted "sidebar: drops on a folder favorite"; then
fixture
printf '{"starting_directory":"%s","play_sounds":false,"confirm_drag_operations":true}' "$START" > "$RUSTYFINDER_SUPPORT_DIR/settings.json"
expect "sidebar: drops on a folder favorite ask first when set (PT-18)" \
  "$(run "tab,menu:New Favorite…,favsheet:name=Dest,favsheet:save,wait,tab,type:File1,shift+down,shift+down,sidebardrop:Dest,wait" "$DATA")" \
  "~sheet: Move 3 items to “right”?"
fixture
out="$(run "tab,menu:New Favorite…,favsheet:name=Dest,favsheet:save,wait,tab,type:File1,shift+down,sidebardrop:Dest,wait,confirm:move,$W,$W" "$DATA")"
out+=$'\n'"moved: $(ls "$START/File1.txt" "$START/file2.txt" 2>/dev/null | wc -l | tr -d ' ')"
expect "sidebar: and move them once confirmed" "$out" "moved: 2"
fi

check "sidebar: items dropped on an application favorite open with it" \
  "cmd+t,openurl:rustyfinder://open?path=/System/Applications,$W,type:TextEdit,menu:Add to Favorites…,favsheet:save,wait,ctrl+1,type:five,sidebardrop:TextEdit,wait" \
  "~external: TextEdit.app <- $DATA/five.bytes"

# --- Favorites: the sheet, the manager, kinds ------------------------------------
check "favorites: New Favorite saves the folder in the chosen group (FV-01)" \
  "menu:New Favorite…,favsheet:name=P,favsheet:group=Work,favsheet:save,wait,cmd+up,sidebarclick:P,$W" \
  "~Work[P]" "path: $DATA"

check "favorites: a name can be used once in a group" \
  "menu:New Favorite…,favsheet:name=P,favsheet:group=Work,favsheet:save,wait,menu:New Favorite…,favsheet:name=p,favsheet:group=Work,favsheet:save,wait" \
  "~problem: The name “p” is already taken."

check "favorites: Add to Favorites takes the selected file" \
  "type:five,menu:Add to Favorites…,wait" "~favsheet: name=five" "~group=Favorites"

check "manager: Opt-Cmd-F opens the Favorite Manager" "alt+cmd+f,wait" "~favmanager: Favorites["

check "manager: a duplicate is selected and opens the same place (FV-03)" \
  "menu:New Favorite…,favsheet:name=P,favsheet:save,wait,fm:open,fm:select=P,fm:duplicate,wait,openfav:P copy,$W" \
  "~selected=P copy" "~P:path,P copy:path" "path: $DATA"

if wanted "manager: deleting a group"; then
fixture
G3="menu:New Favorite…,favsheet:name=a,favsheet:group=Doomed,favsheet:save,wait,menu:New Favorite…,favsheet:name=b,favsheet:group=Doomed,favsheet:save,wait,menu:New Favorite…,favsheet:name=c,favsheet:group=Doomed,favsheet:save,wait"
expect "manager: deleting a group names its contents first (FV-04)" \
  "$(run "$G3,fm:open,fm:select=Doomed,fm:delete,wait" "$DATA")" \
  "~confirm=Delete the group “Doomed”? Its 3 favorites are deleted too: a, b, c."
fixture
refute "manager: and then the group and its favorites are gone (FV-04)" \
  "$(run "$G3,fm:open,fm:select=Doomed,fm:delete,wait,confirm:delete,wait" "$DATA")" "Doomed["
fi

check "manager: editing changes the name and icon" \
  "menu:New Favorite…,favsheet:name=P,favsheet:save,wait,fm:open,fm:select=P,fm:name=Q,fm:icon=film,fm:save,wait" \
  "~,Q:path@film]"

check "manager: a favorite dragged to another group moves there" \
  "menu:New Favorite…,favsheet:name=P,favsheet:group=Work,favsheet:save,wait,fm:open,fm:move=P>Favorites:0,wait" \
  "~favorites: Favorites[P:path," "~Work[]"

check "manager: Reveal in Favorite Manager selects the item" \
  "menu:New Favorite…,favsheet:name=P,favsheet:save,wait,fmreveal:P,wait" "~selected=P "

check "workspaces: a saved workspace is a favorite in its group (workspaces §4)" "workspace:save=W,wait" \
  "~Workspaces[W:workspace@rectangle.split.2x1]"

check "workspaces: Quick Open lists workspaces" "workspace:save=Layout,wait,escape,wait,palette:layout" \
  "~top=*Layout"

check "searches: a search saved as a favorite runs again (search §5)" \
  "searchopt:subfolders,search:zeta,searchkey:return,$W,savesearch,favsheet:save,wait,searchkey:escape,$W,openfav:zeta,$W" \
  "~Searches[zeta:savedSearch]" "~search: zeta"

check "synclets: a sync saved as a favorite opens with its folders (SY-11)" \
  "alt+cmd+shift+s,wait,wait,sync:dir=left,syncfav,favsheet:name=Pair,favsheet:save,wait,openfav:Pair,wait,wait" \
  "~Synclets[Pair:synclet]" "~sync: dir=RightToLeft"

if wanted "presets: a rule list saved as a favorite"; then
fixture
rules "$BASE/r.json" "$(rule AddSequence "$SEQ")"
out="$(run "type:file2,shift+down,return,wait,mrrules:$BASE/r.json,wait,mrfav,favsheet:name=Numbered,favsheet:save,wait,escape,palette:numbered,wait" "$DATA")"
expect "presets: a rule list saved as a favorite shows in Quick Open with 2+ selected (MR-08)" "$out" \
  "~Rename Presets[Numbered:renamePreset]" "~top=*Multi-Rename › Numbered"
fi

# --- Quick Open and Quick Select with tags ----------------------------------------
check "qo: Assign Tag toggles a tag on the selection" \
  "type:five,escape,wait,palette:assign tag red,palettekey:return,$W" "tags: five.bytes=[Red]"

check "qo: Open With offers apps for several files" "type:File1,shift+down,escape,wait,palette:open with textedit" \
  "~top=*Open With › TextEdit"

if wanted "qo: no presets section"; then
fixture
refute "qo: no presets or tags without a selection (QO-03)" "$(run "alt+cmd+a,escape,wait,palette:assign tag" "$DATA")" "Assign Tag ›"
fi

check "qs: a tag picked in Quick Select selects the items carrying it" \
  "type:file2,tag:Red,down,tag:Red,wait,home,cmd+s,qs:tag:Red,qskey:return" \
  "selected: file2.txt|file10.txt"

# --- Settings: remote pane, tags, themes, welcome, Full Disk Access ------------
check "settings: Open remote connections in (SD-03 setting)" "$SET,settings:remote=right,wait" \
  "~remote=right"

sftp "settings: a server opens in the configured pane, even in single-pane mode (SD-03)" \
  "alt+cmd+p,$SET,settings:remote=right,wait,$C,alt+cmd+p,wait" \
  "right.tabs: *remote" "left.tabs: *left" "active: right"

sftp "recents: Recent Folders follow the active pane's server" "$C,type:docs,cmd+down,$W,cmd+up,$W" \
  "~Recent Folders[remote,docs]" "~volumesmenu: " "~,t"

check "themes: a theme applies to every window at once (SD-04)" "$SET,settings:theme=Dark,wait" \
  "~theme: dark appearance=NSAppearanceNameDarkAqua"

check "themes: the inactive pane uses the theme's inactive-selection colour" \
  "$SET,settings:theme=Harbor,settings:duplicatetheme,settings:themecolor=inactive_selection:#123456,wait" \
  "~otherrow: #123456" "~theme: custom-1 appearance=NSAppearanceNameDarkAqua background=#14202B inactive=#123456"

check "themes: deleting a theme asks first (SD-05)" \
  "$SET,settings:theme=Meadow,settings:duplicatetheme,settings:deletetheme,wait" \
  "~confirm=Delete the theme “Meadow Copy”?"

check "themes: after deleting it, windows fall back to Default (SD-05)" \
  "$SET,settings:theme=Meadow,settings:duplicatetheme,settings:deletetheme,wait,confirm:delete,wait" \
  "~theme: default appearance=system" "~theme=Default "

check "themes: a renamed theme keeps its colours" \
  "$SET,settings:theme=Ember,settings:duplicatetheme,settings:renametheme=Night,wait" "~|Night " "~theme=Night "

check "tags: renaming a tag in Settings renames it on items (TG-05)" \
  "type:five,tagsedit:Old,wait,$SET,settings:tab=Tags,settings:tagrename=Old>Archive,wait,wait" \
  "tags: five.bytes=[Archive]" "~Archive"

check "tags: deleting a tag can take it off every item" \
  "type:five,tagsedit:Old+Keep,wait,$SET,settings:tagdelete=Old!,wait,wait" "tags: five.bytes=[Keep]"

check "tags: hidden from the sidebar, reordered and coloured" \
  "type:five,tagsedit:Work,wait,$SET,settings:tagsidebar=Red,settings:tagcolor=Work=4,wait" \
  "~Work:4" "~Red:6(hidden)"

if wanted "tags: Finder's own tag list"; then
fixture
cat > "$BASE/finder.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>values</key><dict><key>FinderTagDict</key><dict><key>value</key><dict>
<key>FinderTags</key><array><dict><key>n</key><string>Project X</string><key>l</key><integer>4</integer></dict></array>
</dict></dict></dict></dict></plist>
PLIST
expect "tags: Finder's own tag list is read" "$(RUSTYFINDER_FINDER_TAGS_PLIST="$BASE/finder.plist" run "" "$DATA")" \
  "~,Project X" "~Tags[Red,Orange,Yellow,Green,Blue,Purple,Gray,Project X]"
fi

check "tags: a colour tag's shortcut toggles it on the selection (TG-06)" \
  "$SET,settings:tab=Shortcuts,settings:bind=Commands/Tags/Green=ctrl+alt+g,wait,type:file2,shift+down,ctrl+alt+g,$W" \
  "tags: file2.txt=[Green] file10.txt=[Green]"

check "tags: a sidebar tag can be hidden" "sidebarhide:Red,wait" "~Tags[Orange,"

check "settings: archives off hands extracting to Archive Utility" \
  "touch:$DATA/a.zip,$SET,settings:toggle=handle_archives,wait,type:a.zip,cmd+shift+x,wait" \
  "~external: Archive Utility.app <- $DATA/a.zip"

if wanted "welcome: first launch"; then
fixture
rm "$RUSTYFINDER_SUPPORT_DIR/settings.json"
expect "welcome: a fresh install shows the welcome (settings §8)" "$(RUSTYFINDER_FDA=denied run "wait" "$DATA")" \
  "welcome: theme=Default fda=Not granted yet continue=false shown=false"
expect "welcome: Start using RustyFinder ends it for good" \
  "$(RUSTYFINDER_FDA=denied run "welcome:continue,welcome:start,wait" "$DATA"; RUSTYFINDER_FDA=denied run "" "$DATA")" \
  "welcome: - shown=true" "fda: prompt=- dismissed=true"
fi

check "welcome: Help › Welcome to RustyFinder shows it again" "menu:Welcome to RustyFinder,wait" \
  "~welcome: theme=Default"

if wanted "fda: a privacy refusal"; then
fixture
expect "fda: a privacy refusal explains Full Disk Access (panes-tabs §9)" \
  "$(RUSTYFINDER_FDA=denied run "loadfail:$HOME/Library/Mail,wait" "$DATA")" \
  "fda: prompt=RustyFinder needs Full Disk Access to show some folders. dismissed=false"
expect "fda: Continue without Full Disk Access is remembered" \
  "$(RUSTYFINDER_FDA=denied run "loadfail:$HOME/Library/Mail,wait,confirm:continue without full disk access,wait" "$DATA"; RUSTYFINDER_FDA=denied run "loadfail:$HOME/Library/Mail,wait" "$DATA")" \
  "fda: prompt=- dismissed=true"
refute "fda: an ordinary folder never asks" "$(RUSTYFINDER_FDA=denied run "loadfail:/tmp/x,wait" "$DATA")" "prompt=RustyFinder needs"
fi

# --- Shares: Bonjour servers and the share picker (SM-02) ------------------------
if wanted "shares: the share picker"; then
fixture
printf 'Share                        Type    Comments\n-------------------------------\nMedia                        Disk\nBackup Drive                 Disk    Nightly\nIPC$                         Pipe    IPC Service\nADMIN$                       Disk    Remote Admin\n\n4 shares listed\n' > "$BASE/smbutil.txt"
expect "shares: the share picker lists disk shares without \$ shares (SM-02)" \
  "$(RUSTYFINDER_SMBUTIL_OUTPUT="$BASE/smbutil.txt" run "sharepicker:nas.local,wait" "$DATA")" \
  "sharepicker: Media,Backup Drive" "~sheet: Open a share on “nas.local”"
fi

check "shares: a share favorite mounts the share when opened (remote-smb-nfs §3)" \
  "favshare:smb://guest@nas.local/Public,wait,openfav:Share,wait" "external: mount smb://nas.local/Public"

if wanted "presets: items dropped on a preset"; then
fixture
rules "$BASE/r.json" "$(rule AddSequence "$SEQ")"
expect "presets: items dropped on a preset are renamed with it (MR-08)" \
  "$(run "type:file2,shift+down,return,wait,mrrules:$BASE/r.json,wait,mrfav,favsheet:name=Numbered,favsheet:save,wait,type:File1,sidebardrop:Numbered,wait,wait" "$DATA")" \
  "~File1.txt→001_File1.txt" "~sidebar.drop: preset Numbered <- 1"
fi

sftp "favorites: Add Path to Favorite saves a server path under its favorite (FV-02)" \
  "$C,alt+cmd+a,menu:Add to Favorites…,favsheet:name=Srv,favsheet:save,wait,type:docs,menu:Add to Favorites…,wait,cmd+up,$W,sidebarclick:docs,$W,$W" \
  "~Srv(docs)" "path: $REMOTE/docs" "conn: remote"

echo
echo "$pass passed, $([ $fail = 0 ] && echo 0 || echo some) failed"
rm -rf "$BASE"
exit $fail
