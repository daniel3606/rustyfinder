# Progress

## Done

### M1: pipeline, navigation, keyboard (2026-11-16)
- Rust workspace: `rustyfinder-core` (Vfs, local backend, snapshot, natural sort,
  history, pane controller), `rustyfinder-ffi` (UniFFI), `uniffi-bindgen`.
- `scripts/build.sh`: cargo → lipo → XCFramework → Swift bindings → XcodeGen →
  xcodebuild → `build/RustyFinder.app`. `--check` adds clippy and tests.
- AppKit shell: window, toolbar, NSTableView (Name / Date Modified / Size /
  Kind), status line, main menu, Go to Folder.
- Navigation and keyboard: open, up, back, forward, reload, hidden files,
  sort by column, type-to-select.
- Checks: 35 Rust tests, clippy clean, `scripts/ui-smoke.sh` 10/10.
- 100k-file folder lists and sorts in about 0.2 s.

### M2: dual pane, tabs, sidebar, path bar, refresh (2026-11-16)
- Rust: `window` (two panes of tabs: pin, swap, clone, sync browsing, move,
  close rules), `store` (settings, favorites, recents, session), `watch`
  (FSEvents + kqueue), pane auto-refresh and per-folder selection memory.
- Swift: sidebar (Devices with free space, Favorites, Recent Folders), two
  panes (vertical/horizontal/single), tab bars, path bar with volumes menu,
  status line with selection size and free space, inline error banner,
  spec-aligned menus and shortcuts, mouse buttons 4/5, session restore.
- Spec tests covered: PT-01..06, 09, 11, 12, 14, 15(partial), VW-01, 03,
  SD-01, FV-01, plus edge cases for names and symlinks.

### M3: file operations, queue, conflicts, undo (2026-11-16)
- Rust: copy/move/delete engine (preflight checks, conflicts with
  apply-to-all and policies, staged replace, symlink-safe), activity manager
  (transfer queue, pause/resume/stop, prompts, per-window undo/redo, log),
  rename / new folder / new file / symlink, drop rule, shell-safe paths.
- Swift: F5 copy to, F6 move to, Copy to as, Cmd-C/X/V, Move Items Here,
  Duplicate, New Folder / File / Folder with Selection, inline rename
  (Return), Move to Trash, Delete Immediately (confirmed), Empty Trash, Make
  Symlink, Undo/Redo, conflict and error sheets, Activities pop-up with
  pause/stop, close/quit warnings, right-click menu (Open With, Copy Path in
  four formats, Show Original, Show Package Contents, Paste into folder).
- Spec tests covered: FO-01, 03, 04, 05, 06, 07, 08, 09, 10, 12, 13, 16, 19,
  20, 21, 22; CF-01..09, 12; EC-01, 05, 08; KS-04; VW-12; SD-02.

### M4: Quick Look, preview, Info, drag and drop, icon view (2026-11-16)
- Quick Look (Space, Cmd-Y), full-screen slideshow.
- Preview pane (Cmd-Shift-P): Preview/Info, Activities (Cmd-Shift-U), Log.
  Editable text with detected encoding and an encoding menu; Quick Look view
  for other types; folder icon; summary for several items.
- Info: kind, exact size, folder size on request, where, dates, tags,
  Finder comment, flags, Open With / Change All, permissions with octal,
  Apply to Enclosed Items, type-specific details. Get Info windows (Cmd-I).
- Drag and drop on list and grid with the spec's rules; file promises.
- Icon view (Cmd-1) with thumbnails; Cmd-plus / Cmd-minus sizes.
- Spec tests covered: PV-01, 02, 03, 05, 06, 07, 14; VW-10 (core), 11;
  FO-20, 21.

### M5: SFTP and the remote pane (2026-11-16)
- Core: connection registry; a tab location is (connection, path); file
  operations between connections; servers have no Trash.
- `rustyfinder-remote`: SFTP over the system `ssh` with `russh-sftp`; askpass
  relay for password, passphrase, verification code and unknown host key;
  explained connection errors.
- App: Connect sheet (Cmd-K) with URL paste, keychain passwords, prompt
  dialogs; remote path bar, tab titles, free space; upload/download with
  F5/F6, paste and drop; rename, new folder/file, delete (confirmed);
  open-and-edit with upload on save; sidebar Connections, Disconnect.
- Tests: 14 Rust integration tests and 17 UI checks against a throwaway
  local sshd. Spec tests covered: SF-01, 02, 03, 04, 06, 11, 12, 13; FO-11.

### M6: Quick Open, Multi-Rename, Sync, Compare (2026-11-16)
- Core: `fuzzy` (best-alignment matcher, patterns), `goto` (typed paths,
  completion, history), `rename` (rules, preview, safe ordering, versioned
  presets), `sync` (analysis into a plan; options, excludes, clock offset);
  MultiRename and Sync run as activities with undo and log entries.
- App: Quick Open (Cmd-O), Show Devices, Go to Folder with completion and
  history (Cmd-Shift-G), Quick Select (Cmd-S); Multi-Rename window (Return
  with several items); Sync window (Opt-Cmd-Shift-S) with options, exclude
  rules, clickable tallies and per-row checkboxes; Compare… with FileMerge.
- Spec tests covered: QO-05..09; MR-01..07, 09, 10 (core), MR-01, 03, 04,
  07, 09 (UI); SY-01..10 (core), SY-02, 03, 04, 06, 07, 08, 09, 13 (UI).

### M7: search, archives, tools, tags, servers, shares, workspaces (2026-11-16)
- **Search and filter (done):** toolbar search field (Cmd-F) with a menu
  for criterion (Name, Extension, Kind, Tags, Content), engine (RustyFinder or
  Spotlight), subfolders, case and regex. Filter as you type; Return
  searches; results list with a Where column; Back / Esc return to the
  folder. Works on servers. Spec tests: SE-01..06 (UI), SE-02, 04, 05, 06
  (core), SY-12.

- **Archives (done):** browse zip (read in place), tar.*, 7z, rar and
  more as read-only folders, on this Mac and on servers; Cmd-Up leaves
  the archive; Extract (Cmd-Shift-X), Extract to (Opt-Cmd-Shift-X),
  Compress (Cmd-Shift-M), Compress with Options (format, password, level,
  metadata); password prompts; legacy name encodings; traversal blocked
  and logged. Spec tests: AR-01..10 (AR-06, 08 with zip passwords).

- **Tools (done):** custom tools with safely quoted placeholders, run as
  activities with an output window; Tools editor (drafts, duplicate,
  reorder, delete with confirmation); Checksum window (MD5, SHA-1,
  SHA-256, SHA-512, compare, CSV copy); Open in Terminal (Cmd-Shift-R);
  git status column and Add / Add All / Commit / Pull / Push. Spec tests:
  TL-01, 02, 04..10 (TL-03 by design: login shell).

- **Tags (done):** Finder tags shown as dots, toggled from colour
  swatches in the context menu and Commands > Tags, edited in a Tags
  pop-over with completion, listed per tag from the sidebar. Spec tests:
  TG-01, 02, 03, 08; VW-13.

- **FTP / FTP with TLS (done, TLS unverified):** connect from the
  Connect panel (protocol pop-up, Passive Mode, Text Encoding) or a
  pasted `ftp://` / `ftps://` URL; browse, upload, download, rename,
  new folder, delete. Spec tests: FT-01, 04, 05, 07, 09.

- **WebDAV / WebDAV HTTPS (done, HTTPS unverified):** connect from the
  Connect panel or a pasted URL (server, port and remote path split
  apart); Basic and Digest sign-in, anonymous shares; browse, streamed
  upload, download, rename with conflict detection, new folder, delete,
  server-side duplicate. Spec tests: WD-01..06.

- **Amazon S3 and compatible (done against a scripted server):** buckets
  at the top, prefixes as folders, upload (multipart for large files,
  aborted if stopped), download, rename (also of folders), new folder /
  bucket, delete, server-side duplicate. Spec tests: S3-01..07.

- **Network shares (done, mounting unverified):** SMB, AFP, NFS through
  the system, VNC handed to Screen Sharing.
- **Workspaces (done):** Save Workspace…, Update Workspace, a Workspaces
  menu and sidebar group; open in place or in a new window. Spec tests:
  FV-05, FV-06.

### M8: Settings, shortcuts, README (2026-11-16)
- Settings window (Cmd-,): General (starting directory, the on/off
  options, Terminal application), Transfers (simultaneous transfers,
  replace / merge / error policies), Shortcuts (every menu command, record
  a new combination, conflict with Reassign, Restore Defaults).
- README.md.
- Spec tests: SD-01, 02, 07; KS-02, 03, 04, 07.

## Spec completion (2026-11-17)

Everything the specs ask for is built, apart from what the local-first rule
excludes and what needs outside services to verify. The item-by-item status,
with reasons for each exclusion, is in [docs/GAPS.md](docs/GAPS.md); design
choices are in [docs/DECISIONS.md](docs/DECISIONS.md).

Checks at the end of this pass: rustfmt and clippy (`-D warnings`) clean,
535 Rust tests, 550+ scripted UI checks against the Release build, and
`examples/bench.rs` on 100,000 files (list about 0.2 s, sort about 0.05 s,
first sort by date added about 0.3 s).

The "leftovers" lists that used to follow here are superseded by GAPS.md.
