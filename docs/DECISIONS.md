# Decisions

Implementer's log. Newest milestone last. Each entry: what was decided and why.
Specs in `docs/spec/` win over anything here; entries marked **(no spec)** were
designed without one and should be revisited when a spec appears.

## M1: pipeline, navigation, keyboard

**Spec status:** `docs/spec/` was empty (no `INDEX.md`) at the start of M1.
Everything below is my own design.

### Build and toolchain

- **arm64 only for now.** This Mac has Homebrew Rust with no `rustup`, so only
  the host target's standard library exists. `scripts/build.sh` builds every
  Apple target whose std is installed and `lipo`s them, so installing rustup
  and adding `x86_64-apple-darwin` yields a universal app with no script change.
- **Deployment target is macOS 26.0.** Homebrew's Rust std is compiled for the
  host OS (26.0); a lower app target makes the linker warn on every std object.
  With a rustup toolchain this can drop to 14.0.
- **`uniffi-bindgen` is its own crate.** Putting the bindgen binary in
  `rustyfinder-ffi` (the usual layout) linked clap and friends into the static
  library: 61 MB vs 10 MB.
- **UniFFI 0.32, proc-macro mode, no UDL.** Bindings are generated from the
  host dylib in library mode.
- **Swift 5 language mode, minimal concurrency checking.** Keeps the generated
  bindings and AppKit delegate code free of Swift 6 isolation errors. UI types
  are still main-thread only by construction.
- **Ad-hoc signed, no sandbox, no hardened runtime.** Personal-use file
  manager; the sandbox would block most of the disk.
- **Generated files are not committed:** `RustyFinderApp/Generated/rustyfinder_ffi.swift`,
  `RustyFinderApp/RustyFinder.xcodeproj`, `RustyFinderApp/Support/Info.plist`, `build/`.

### Architecture

- **`Vfs` is a blocking trait.** Callers run it on worker threads. Network
  backends (M5+) will own a tokio runtime and `block_on` inside. This keeps
  the core, the transfer queue and the FFI free of async plumbing.
- **Paths are absolute '/'-separated UTF-8 strings** across the Vfs API, so the
  same code serves local and remote. Non-UTF-8 local names are shown lossily
  (APFS does not allow them; foreign volumes might).
- **File names are never normalised.** They are stored and passed back
  byte-for-byte as the file system reported them. NFC + lower-case folding is
  used only for comparing: sorting, filtering and type-to-select.
- **`Snapshot` is immutable and shared.** A re-sort or filter builds a new
  index order over the same `Arc`'d entries; nothing is re-read or copied.
- **`PaneController` (Rust) owns history, view settings and loading.** Each
  request takes a generation number; a result that arrives after a newer
  request is dropped. History is committed only when a load succeeds, so a
  failed navigation leaves the pane where it was.
- **Swift pulls rows in pages of 256** from the snapshot and keeps at most 48
  pages. 100k files: list + sort about 0.2 s, and Swift never holds the folder.
- **The Rust pane holds the Swift listener strongly; the Swift listener holds
  its model weakly.** Otherwise the pair would never be freed.

### Behaviour **(no spec)**

- Sort: folders first, natural (digit-aware), case-insensitive. Folders stay
  on top in both directions. Ties fall back to name.
- Hidden = dot-file or the BSD `UF_HIDDEN` flag. Off by default, Cmd-Shift-.
- Broken and looping symlinks are listed as "Broken Alias", never an error.
- Entries that vanish mid-listing are skipped instead of failing the folder.
- Keys: Return / Cmd-Down / Cmd-O open; Delete and Cmd-Up go up (and select
  the folder you left); Cmd-[ and Cmd-] history; Cmd-R reload; Cmd-Shift-G go
  to folder; Cmd-Ctrl-1..4 sort column (again to reverse); typing selects by
  prefix (1 s reset). **Return opens rather than renames**, commander-style;
  to be made a preference when rename lands in M3.
- Type-to-select is done in Rust; AppKit's built-in one walks every row.
- Opening several items opens the files and skips plain folders.
- Back/forward currently selects the first row; remembering the selection per
  history entry is planned for M2.

### Testing

- Rust: unit tests per module, temp dirs only.
- UI: `scripts/ui-smoke.sh` drives the real app through `--keys` / `--dump`
  (see `DebugSnapshot.swift`). It injects key events and dispatches menu
  shortcuts down the responder chain in-process, because the Mac is unattended:
  the display is asleep, `screencapture` returns black, and there is no
  Accessibility permission. `--snapshot file.png` renders the window offscreen
  (toolbar glass renders as white blobs there; not a real defect).
- `RustyFinder --selftest <dir>` lists a folder through the whole FFI path, no UI.

## M2: dual pane, tabs, sidebar, path bar, refresh

**Spec status:** all 24 specs arrived during M1. M2 follows `panes-tabs.md`,
`views.md`, `keyboard-shortcuts.md`, `settings-defaults.md`, `edge-cases.md`
and the sidebar parts of `workspaces-favorites.md`.

### Repository

- **The analyst commits to this same repository.** I never stage `docs/spec/`
  and never run `git add -A`; only my own paths are added.

### M1 behaviour changed to match the specs

- **Return no longer opens.** The spec binds Return to Rename (M3). Open is
  Cmd-O, Cmd-Down and double-click. Bare Delete no longer goes up (unbound in
  the spec; Cmd-Up does it).
- **Sort shortcuts** use the spec's numbering: Ctrl-Cmd-1 Name, 2 Kind,
  4 Date Modified, 5 Date Created, 6 Size. Opt-Cmd-O toggles descending. The
  other digits arrive with their columns. Picking a column in the menu no
  longer flips direction.
- **Date and size columns sort descending on the first header click.**
- **Matching ignores accents** (typing `e` finds `é`): names are folded with
  NFD, combining marks dropped, lower-cased. The same key drives sorting, with
  the raw name as tie-break.
- **Load errors are inline** (a banner above the list with Retry and Dismiss),
  never a modal sheet.
- Alternating rows default off; relative dates default on; empty cell for a
  missing date.
- On-disk `:` in a name is displayed as `/`.

### Architecture

- **`WindowModel` (Rust) owns tabs and pane rules:** pinning, close outcomes,
  swap, clone, sync browsing, moving tabs. Swift renders `WindowLayout` and
  forwards commands, so the spec's PT tests are Rust unit tests.
- **One `FileListViewController` per tab, kept alive** while the tab exists,
  so selection and scroll position survive tab switches for free.
- **`Store` (Rust) persists** settings, favorites, recents and the session as
  separate JSON files in `~/Library/Application Support/RustyFinder`, written via
  temp file + rename. Unknown or missing fields load fine; a corrupt file
  falls back to defaults.
- **Folder watching uses two sources.** FSEvents (via `notify`) for changes to
  children, filtered to direct children so watching `~` does not reload on
  every write under `~/Library`. A kqueue vnode watch on the folder itself,
  because FSEvents says nothing when the watched folder is deleted, renamed
  or unmounted (found by test). Changes are debounced 200 ms.
- **The watcher never cancels a user navigation:** it skips its reload while
  any load is in flight.
- **A folder that disappears** sends the tab to its nearest existing ancestor,
  else home, with no error. (Spec PT-15 says "starting directory" for an
  unplugged drive: the walk-up ends there in that case, since nothing under
  the mount point survives.)
- **AppKit window restoration is off** (`NSQuitAlwaysKeepsWindows` false,
  windows not restorable). RustyFinder restores from its own session; AppKit's
  version also blocked launch with a modal "reopen windows?" alert after an
  unclean exit (found by test).
- **Hidden files is one global switch** (spec: same as the setting), applied
  to every tab of every window.

### RustyFinder choices (spec left open)

- Tab switches panes; it does nothing in single-pane mode.
- New tabs open beside the current tab at the same location, same view.
- Closing a pane's last tab closes the window (as the spec suggests).
- Moving a pane's only tab to the other pane leaves a fresh tab at the same
  location behind, so a pane is never empty.
- Navigating in a pinned tab opens a new tab beside it. Back/Forward do
  nothing in a pinned tab.
- Sync browsing mirrors only a one-step move (into a child, up to the parent),
  matched on whole path components. A pinned follower tab does not move.
- Status line sits at the bottom of each pane; path bar on top.
- The path bar is an `NSPathControl` (native truncation and icons).
- Sidebar: Devices, favorite groups, Recent Folders. Option-click opens in
  the other pane, or the visible one in single-pane mode.

### Not done yet in M2 scope (tracked in PROGRESS.md)

- Dragging tabs to reorder or move them (model supports it: `move_tab`).
- Dragging favorites to reorder (model supports it: `move_favorite`).
- Drops onto tabs, sidebar and path bar (needs M3 file operations).
- Sidebar Editor, Connections/Shares/Tags groups, multi-volume eject prompt.
- Full Disk Access prompt (inline error only for now).
- Symlink targets are resolved during listing, not lazily with a timeout.

## M3: file operations, transfer queue, conflicts, undo

Follows `file-ops-transfer-queue.md`, `conflicts.md`, `edge-cases.md`.

### Architecture

- **The engine talks to the world only through `OpContext`** (progress out;
  conflict answers, error answers, pause and stop in). The same engine runs
  under the activity manager and under a scripted context in tests, so the
  FO/CF/EC spec cases are plain unit tests on real temp folders.
- **Local copies use `copyfile(3)`** with `COPYFILE_CLONE | COPYFILE_DATA_SPARSE`:
  an instant clone on the same APFS volume, otherwise a real copy that keeps
  holes, dates, permissions, flags, ACLs and extended attributes (Finder tags,
  comments, resource forks live there). The creation date is set explicitly
  afterwards, because a non-clone copy gets "now".
- **Folders are recursed by the engine, not by `copyfile`**, so every item
  gets conflict handling, progress and a stop point. A folder's own dates and
  attributes are applied after its children are in place.
- **Cross-backend copies stream** through `open_read` / `create`, then set
  dates and mode. `create` is exclusive: nothing is overwritten by accident.
- **Replace is staged.** The replacement is written beside the target under a
  temporary name and swapped in at the end. Stopping or failing mid-way
  removes the half-built copy and leaves the original untouched (CF-08).
  File-over-file swaps are one atomic rename.
- **Replace takes the source's capitalisation** (CF-09): after the atomic
  swap the entry is respelled, using `F_GETPATH` to learn the old spelling.
- **"Do not overwrite" is atomic** on local volumes (`renamex_np` with
  `RENAME_EXCL`), with a check-then-rename fallback where the file system
  lacks it.
- **Trash is done by Swift** (`FileManager.trashItem`) behind the
  `PlatformServices` callback, so "Put Back" data and per-volume Trashes are
  the system's. The core only records where the item went, for undo.
- **Undo is built from recorded effects** (Created, Moved, Trashed). Each has
  an exact inverse, so one mechanism gives undo and redo for copy, move,
  rename, duplicate, new items and trash. Undoing a create moves the item to
  the Trash; it never deletes.

### RustyFinder choices (spec left open)

- **Free-space pre-check only across volumes.** A same-volume copy on APFS is
  a clone and needs no space, so refusing it would be wrong.
- **An operation that replaced or merged anything is not undoable.** What was
  overwritten cannot be brought back, and a partial undo would mislead.
- **Undo skips past permanent deletes** rather than disabling the menu.
- **Undo never overwrites.** If the original name was taken in the meantime,
  the normal conflict prompt appears.
- **Pasting or copying an item into the folder it is already in** makes
  `name copy` instead of raising a conflict with itself. Moving it there does
  nothing.
- **Conflict sheet button order:** Skip (Return), Replace, Merge, Replace
  Older, Keep Both, Stop (Esc). The default changes nothing on disk.
- **Delete Immediately:** Cancel answers both Return and Esc, so a stray key
  never deletes.
- **F5 / F6 are disabled in single-pane mode**, where the target pane cannot
  be seen.
- **Cut** (Cmd-X) is Copy that moves on Paste; the spec lists the key but not
  the behaviour.
- **Completion sound** only for transfers, deletes, trash and undo, not for
  quick edits like rename or New Folder.
- **A rename ending by loss of focus commits**, as in Finder. Esc cancels.
- **A folder listing that arrives during an inline rename is held back**
  until editing ends, so the field is not pulled away mid-typing.
- **Symlinks made by Make Symlink** are relative when created beside their
  target, absolute otherwise.
- Queue membership: copies and duplicates queue; deletes, trash, rename, new
  items and same-volume moves run at once (spec §5.2).

### Scripted UI checks

- `RUSTYFINDER_TRASH_DIR` redirects the Trash to a scratch folder so checks never
  put test files in the real Trash.
- The harness dispatches menu shortcuts the way AppKit does when a sheet is
  up (sheet's responder chain, then the main window's), so the "nothing acts
  behind a prompt" rule (EC-08) is really exercised.

### Not done yet in M3 scope (tracked in PROGRESS.md)

- Retry with administrator rights, locked-item unlock prompt.
- Auto-retry, bandwidth limits, resume of partial files (need remotes: M5).
- Make Alias (needs bookmark data), Print, Share, Edit.
- Log view UI and reordering the queue by drag (both exist in the core).
- System notification when a long activity ends in the background.

## M4: Quick Look, preview pane, Info, drag and drop, icon view

Follows `preview-quicklook.md`, `views.md`, the drag-and-drop part of
`file-ops-transfer-queue.md`.

### Architecture

- **One controller per tab, two layouts.** The list (`NSTableView`) and the
  icon grid (`NSCollectionView`) sit in the same controller and share one
  selection API (`selection`, `setSelection`, `scrollToVisible`). Every
  command, rename, drag and drop, context menu and Quick Look therefore works
  in both without a second code path.
- **Layout is part of the tab's `ViewSpec` in Rust**, so it is saved with the
  session and will ride along with per-folder view settings later.
- **Text and icon sizes are settings** (`settings.json`), separate per layout,
  shared by all tabs and windows.
- **Text encoding: Rust guesses, Foundation converts.** The guess follows the
  spec's order (byte-order mark, valid UTF-8, statistical detector, Mac OS
  Roman). Decoding and encoding use `NSString` encodings, because the spec's
  menu lists Apple-specific encodings the Rust detector's library cannot
  write (NextStep, DOS Latin-US, the Mac OS ones).
- **Quick edit saves by replacing the file in one step** (`replaceItemAt`),
  in the encoding the file has, keeping its byte-order mark and permissions.
- **The drop rule is a Rust function** (`drop_mode`): Option copies, Command
  moves, otherwise move within a volume and copy across. Pointless drops (a
  folder into itself, a move to where the item already is) are refused while
  dragging, not after.
- **Preview uses `QLPreviewView`** for everything that is not a small text
  file or a folder, so it shows whatever the system (and installed Quick Look
  extensions) can.

### RustyFinder choices (spec left open)

- Preview pane hidden by default; its mode and visibility are saved per window.
- Text files up to 2 MB open in quick edit; larger ones go to Quick Look.
- Finder comments are set by asking Finder (AppleScript), because only then do
  they show in Finder's own Get Info. Needs Automation permission on first use.
- "Open with" for one file uses `NSWorkspace.setDefaultApplication(at:toOpenFileAt:)`;
  "Change All" uses the content-type variant, after a confirmation.
- Permissions are applied as an activity (`SetMode`); "Apply to Enclosed
  Items" never follows symlinks.
- Multi-selection Info shows count, total size of files, and the date or date
  range. Editing tags or comments for several items at once is not built.
- Icon view: Left/Right walk items in index order across rows; thumbnails
  replace generic icons for files; sizes 32 to 256 pt.
- Slideshow opens Quick Look full screen; the system offers no public way to
  auto-advance, so the arrow keys step.

### Not done yet in M4 scope (tracked in PROGRESS.md)

- **Column view.** Cmd-3 is accepted and stored, but shows the list.
- Grouping, extra list columns (toggled from the header), View Options panel,
  per-folder view settings, Calculate Folder Size in the Size column,
  expandable folders in the list.
- Spring-loaded folders; drops on tabs, sidebar items and path bar segments.
- Remote previews and the download button (need M5).

## M5: SFTP and the generic remote pane (design, written before coding)

Follows `remote-sftp.md`.

### The stack conflict, and how it is resolved

- The build brief says: SFTP with `russh` + `russh-sftp`.
- The spec says: RustyFinder uses the **system OpenSSH client**, so `~/.ssh/config`
  (Host aliases, ProxyJump, Include, IdentityFile...), `known_hosts`, the
  agent and its fixed option list all apply (SF-01, SF-02, SF-03).
- **Decision: both.** Transport and authentication are the system `ssh`
  (`ssh <options> host -s sftp`), spawned by Rust. The SFTP protocol itself is
  spoken by `russh-sftp` over that process's stdin/stdout. `russh` (the SSH
  transport crate) is not used: reimplementing ssh_config and ProxyJump on top
  of it would break the spec's contract.

### Prompts (password, passphrase, verification code, host key)

- `ssh` is started with `SSH_ASKPASS` pointing at the RustyFinder binary itself and
  `SSH_ASKPASS_REQUIRE=force`. In that mode the binary only relays: it sends
  the prompt text to a Unix socket owned by the running app and prints the
  answer. The app answers from the password the user typed, or shows a dialog.
- A password prompt is answered automatically once from the Connect panel's
  password; anything else (passphrase, "Verification code", unknown host key)
  goes to the user.

### Several file systems at once

- A registry maps a connection id to its backend; id 0 is the local disk.
- A tab's location is (connection id, path). History entries carry both, so
  Back from a server returns to the local folder.
- Operations name a source and a target connection. The engine already
  streams between two backends and renames within one.
- Remote targets have no Trash: Move to Trash becomes Delete, which always
  asks (FO-11).

### Tests

- A throwaway `sshd` on 127.0.0.1 and a high port, run as the current user
  with its own host key, config and `authorized_keys` in a temp folder
  (the fixture the spec describes). The client is pointed at a temp
  `ssh_config` and `known_hosts`, so the real `~/.ssh` is never read or
  written. Tests skip themselves if `sshd` cannot start.

### M5 as built

- **Connect is a sheet on the window** (Cmd-K), not a tab-like panel inside
  the pane as the spec describes. The connection opens in the active pane.
  Errors show inside the sheet and the fields stay editable.
- **Protocol popup lists SFTP only** until the other backends exist (M7).
- **Passwords: keychain only.** An empty Password field falls back to a
  saved one; "Remember password" stores the typed one after a successful
  connect. Service "RustyFinder", account `sftp://user@host:port`.
- **Changed host key: refused, with ssh's own warning text.** There is no
  override button yet; the user has to fix `known_hosts` (the spec wants an
  explicit override with Cancel as default).
- **Remote panes refresh after every finished operation** (servers send no
  change events), and on Cmd-R.
- **Opening a remote file** downloads it to a per-launch temp folder, opens
  it, and watches the copy; each save uploads it back with `Put` (replace
  without asking, staged). Temp copies are deleted at quit.
- **Off on a server for now:** Quick Look, preview pane, Get Info, Open
  With, Reveal in Finder, dragging items out, favorites, Recent Folders.
- **Copy/Cut on a server** puts the paths on the pasteboard as text and
  remembers the items inside RustyFinder, so Paste in another pane transfers them.
- **Drops onto a remote pane upload** (copy; Command moves).
- **Replace on a server is delete-then-rename.** The SFTP library has no
  atomic rename-over, so SF-10 ("the listing never lacks the file") is not
  met during the swap. The new content is complete before the old is removed.
- **Names that are not valid UTF-8** are shown lossily and cannot be
  addressed (SF-09 not met): the library exposes names as Strings.
- **Owner and group names** are not shown yet.
- **Reconnect after sleep and remote-tab restore on launch** are not built:
  a dead session shows an error; a saved remote tab reopens at the starting
  directory.
- **A remote tab is not undoable for creations** (upload, new folder): undo
  would need a Trash. Moves and renames on a server can be undone.

## M6: Quick Open, Multi-Rename, Sync, Compare

- **Fuzzy matching finds the best alignment, not the first one.** A greedy
  left-to-right match ranked "Folder B" below worse candidates, so the
  matcher scores every alignment (dynamic programming) and keeps the best.
  Word starts and consecutive letters score higher. Lives in `fuzzy.rs`.
- **Quick Open lists commands, favorites, devices, recent folders and open
  connections.** Applications, tools, presets and synclets join it when
  those features exist (M7/M8).
- **Go to Folder completes from the Rust side** (`goto.rs`): `~`, relative
  paths, Tab completion with one trailing slash, a file path selects the
  file in its folder, and a history of earlier destinations (`goto.json`).
- **Quick Select** runs the pattern in Rust against the snapshot (plain
  text, `.ext`, wildcards; accents and case ignored). Esc restores the
  selection that was there before.
- **Multi-Rename rules cross the FFI as JSON** in the core's own serde
  shape. The window never computes a name: every keystroke asks the core
  for a fresh preview (names, invalid and conflict flags, summary, whether
  Rename is allowed). One rule format serves the window and saved presets.
- **Rename order is planned in Rust.** Swaps and chains go through
  temporary names; a target that exists and is not part of the set stops
  that pair and is reported, never overwritten. One undo restores all names.
- **Multi-Rename rule types built:** Replace Text (plain or regex, all /
  first / last, entire text), Add Text, Change Case, Add Sequence, Add Date.
  The engine also has Add Metadata and "at index" positions; the window
  does not offer them yet.
- **Sync: the left pane is always the left side.** The active pane is the
  source, so opening Sync from the right pane starts with direction ←.
- **Sync plan rows are listed in the panes' natural order** (file2 before
  file10), deletions last.
- **Switching on "Delete orphaned items" asks first** and stays off unless
  confirmed. It is disabled in both-ways mode, which never deletes.
- **The Sync window closes when Synchronize is pressed.** The plan runs as
  one activity in the usual queue. Reopening the window analyses again.
- **Exclude rules: one line each, a kind plus a typed value** ("Size
  greater than" + `10 MB`, "Extension is" + `log, tmp`). A value that is
  empty or not valid yet makes the rule inactive rather than an error.
  Tags are not offered (tags arrive in M7).
- **Row details are a tooltip**, not an expanding row.
- **Compare uses FileMerge through `opendiff`**, and only if the selected
  Xcode really contains FileMerge (`/usr/bin/opendiff` alone is a stub).
  Other tools (Kaleidoscope, Beyond Compare, Araxis) come with the Settings
  window in M8. Remote files are downloaded to temporary copies first.
- **Not built in M6:** rename presets and synclets as favorites (they need
  the favorite kinds from M7), preset drop on a selection (MR-08), Tags
  exclude rule, the toolbar Sync and Compare buttons.

## M7: Search and filter

- **Search results are an ordinary listing whose names are relative
  paths** ("Folder B/zeta.png" below the folder searched). The tab keeps
  its connection and folder, so every command that works on paths (copy,
  move, rename, delete, Quick Look, Info, Multi-Rename) works on results
  unchanged, on this Mac and on servers. The list shows the last part as
  the name and the rest in a "Where" column; sorting, filtering and
  type-to-select go by the last part.
- **A search is not a history entry.** Back (or Up, or Esc in the field)
  leaves the results for the folder that was searched.
- **Filter as you type only for plain name matching in this folder**
  (RustyFinder engine, no subfolders, no regex, not case-sensitive). Every other
  combination is a search and starts with Return.
- **A filter belongs to the folder it was typed in**: navigating clears it.
- **Results arrive in batches** (every 250 ms) while the search runs; the
  status line says "searching…". There is no separate spinner or Stop
  button: Esc or leaving the tab stops it.
- **Spotlight is asked through `mdfind`** with the term passed as one
  argument and quoted for the query language. Kinds, regular expressions
  and case-sensitive terms cannot be asked of the index, and servers are
  not indexed: those fall back to RustyFinder's own search and the status line
  says so.
- **Content search** reads each file whole, detects its encoding like the
  preview does, and skips binary files (a NUL in the first 8 KB). On a
  server, files over 10 MB are skipped and counted in the status line.
- **Engine and "Search in Subfolders" are remembered separately** for this
  Mac and for servers (in the app's preferences).
- **Not built yet:** saved searches as favorites, searching inside
  archives, a Tags search from the sidebar (the engine supports the tag
  criterion through Spotlight).

## M7: Archives

- **A plain zip is read in place.** RustyFinder reads the zip's directory
  itself (including zip64 and exact timestamps), so even a huge zip lists
  at once and nothing is unpacked; entries are inflated when read.
- **Every other format goes through the system's `bsdtar`** (libarchive,
  part of macOS): encrypted zips, tar and its compressed forms, 7z, rar,
  cpio, iso, xar/pkg, lha, cab. The archive is unpacked once into a
  per-launch temporary folder and shown from there; the copy is deleted
  when the archive is closed and at quit. This trades disk space for a
  wide format list without a dozen parsing libraries. Stuffit and lone
  xz / lzma / zstd files are not supported ("Unrecognized archive format").
- **An open archive is a connection**, like a server: a read-only file
  system registered in the core. Browsing, sorting, searching and copying
  out are the ordinary code paths, and **Extract is a copy out of the
  archive**, so conflicts, progress, Stop and undo work as for any copy.
- **Path traversal cannot happen by construction.** Entries whose path
  contains `..` never enter the archive's tree (zip), or are refused by
  bsdtar (others, which also refuses to write through symlinks). Either
  way they are listed in the log when the archive is extracted. A leading
  `/` is dropped, keeping the item inside.
- **Zip names without the UTF-8 flag:** valid UTF-8 is taken as UTF-8;
  otherwise the encoding is guessed from all names together (Japanese,
  Cyrillic…), and Western text falls back to code page 437.
- **Passwords** are asked for when an archive needs one, again after a
  wrong one, and are not stored. They reach bsdtar as a command-line
  argument, visible to other processes of this user for that moment.
- **Extract:** one top-level item comes out as itself; several go into a
  folder named after the archive. An archive on a server is downloaded
  first; extracting to a server uploads from the archive.
- **Compress** always goes through bsdtar: zip (AES-256 with a password),
  tar.gz, tar.bz2, tar.xz, 7z (no password: libarchive cannot write
  encrypted 7z). Names are stored as UTF-8. Finder metadata (`.DS_Store`,
  `._` companions) is left out unless the option is switched off. On a
  server, items are downloaded to a scratch folder, packed, and the
  archive uploaded.
- **Inside an archive nothing can be changed**: the write commands are
  disabled. Quick Look, the preview pane and Info are off there for now,
  as on servers.
- **"Treat archives as folders" is a setting, on by default.** "Show
  Archive Contents" browses regardless of it. "Handle archives with
  RustyFinder" has no off position: RustyFinder always uses its own engine.

## M7: Tools, Checksum, Terminal, git

- **Placeholders are substituted already shell-quoted**, one argument per
  path or name, by the same escaping as "Copy Path for Terminal". Only
  whole placeholder names count (`$SOURCE_PATHS` is left for the shell).
- **Tools run through `$SHELL -l -c`** in their own process group, with
  standard output and error on one pipe so the order is kept. Stop sends
  SIGTERM to the group, then SIGKILL after 5 seconds. Output is kept up
  to 4 MB (the tail).
- **A tool run is an activity** ("Running “name”"). The finish report
  carries the output; a non-zero exit adds "Task exit code: N" and counts
  as failed. With "Show output" off, the failure is the usual alert, with
  the last lines the command printed. The log keeps the first 200 lines.
- **A new tool is a draft until it has a name and a command.** Esc
  discards a draft and nothing else. Tools are saved on every edit, in
  list order, in `tools.json`.
- **Tools live in Commands > Tools and in the context menu** ("Tools"
  submenu). Shortcuts are typed as text ("ctrl+alt+t") for now; conflict
  checks arrive with the shortcuts editor (M8). No toolbar items and no
  placeholder drag yet (a pop-up inserts placeholders).
- **Open in Terminal** opens the Terminal app at the selected folder if
  exactly one folder is selected, else at the pane's folder. The app is
  Apple's Terminal until Settings offers a choice (M8). Disabled on
  servers (no ssh hand-off yet).
- **Checksum** streams each file through the core (so it works on
  servers and inside archives), one file after the other; closing the
  window cancels. Copy gives CSV (`name,algorithm,digest`).
- **git status comes from `git status --porcelain`** run off the main
  thread after each listing; a folder shows the most urgent status found
  inside it. It appears as an "SCM" column (M, A, ?, !, U) when "Show SCM
  Status" is on and the folder is in a working tree. No badges in icon
  view yet.
- **git commands are shell activities** ("SCM Operation: Add"…) run with
  the user's own git setup. Commit takes a message; the button stays off
  while it is empty.
- **Scripted checks never launch another app**: under the harness,
  "open with app" requests are recorded instead of carried out.

## M7: Tags

- **The system reads and writes the tags** (Foundation's tag-names
  resource value), so Finder and Spotlight see the same thing. The list
  logic is in the core (`tags.rs`): no duplicates, toggling over a
  selection, the tags a selection has in common, completion.
- **Tag names compare without regard to case** and keep the spelling
  they were first given, as in Finder.
- **Toggling over a selection:** if every item has the tag it comes off
  all of them, otherwise it goes on those that lack it.
- **The Tags pop-over shows the tags every selected item has.** Removing
  one removes it from all; adding one adds it to all; tags only some
  items carry are left alone.
- **Dots** (up to three, colour tags only, in tag order) follow the name
  in list view. An item with only an old-style colour label shows that
  colour. Icon view has no dots yet.
- **Known tags** are the seven colour tags plus custom tags RustyFinder has
  seen assigned; they feed completion and the sidebar's Tags group.
- **A sidebar tag lists everything with that tag** in the home folder
  through Spotlight, as search results.
- **Tags are local only**: the commands are disabled on servers and in
  archives.
- **Not built:** undo for tag changes, the tag list in Settings (rename
  or delete a tag everywhere, reorder, hide), dropping items on a
  sidebar tag (TG-04), a Tags column and sorting or grouping by tags,
  default shortcuts for colour tags (they are menu commands, so the
  shortcuts editor can bind them).

## M7: FTP and FTP with TLS

- **Built on `suppaftp`** (blocking API, which suits the blocking `Vfs`),
  with the system TLS library (`native-tls`), so certificates are checked
  against the system trust store.
- **A pool of sessions.** A control connection does one thing at a time
  and a transfer occupies it, so each call borrows a logged-in session
  and a transfer keeps its own until it ends. A session that turns out
  stale is dropped and the command is tried once more on a fresh login
  (the "reconnect silently" rule). No keep-alive `NOOP` is sent.
- **Passive mode by default**, `EPSV` first; a server that refuses it is
  asked with `PASV` from then on. An address in the `PASV` reply is
  ignored in favour of the control connection's (servers behind NAT).
  With Passive Mode off the server connects back (active mode).
- **Listings:** `MLSD` when the server has it, else `LIST -a` after
  changing into the folder (a path argument to `LIST` is unreliable),
  parsed as Unix or Windows lines; lines that fit neither are skipped.
- **Names:** `OPTS UTF8 ON` when offered. Otherwise bytes that are valid
  UTF-8 are UTF-8, and the first name that is not decides the server's
  encoding (statistical guess, or the encoding chosen in Advanced) for
  the rest of the session, both ways. So a name is sent back as the
  bytes it came in. A name the encoding cannot hold is refused.
- **Uploads** end when the engine flushes the writer; that is when the
  server's verdict (for example "quota exceeded") is reported. The
  modification time is then set with `MFMT` where offered, else left.
- **A download dropped halfway closes its session** rather than sending
  `ABOR`: servers differ in how many replies an abort produces, and a
  miscounted reply would garble later commands.
- **Certificate problems:** the Connect panel shows the warning and a
  "Continue Anyway" button (Cancel stays the default). Trusting a
  certificate permanently is not built.
- **Not built / not verified:** implicit FTPS (port 990), resume
  (`REST`), TLS session reuse on the data channel and the whole TLS path
  (no TLS test server here: FT-02, FT-03 unverified), FT-06, FT-08
  (favorites for servers do not exist yet).
- **Tests** run against a scripted FTP server written for this
  (`crates/rustyfinder-remote/tests/support/ftpd.rs`), also runnable as
  `cargo run -p rustyfinder-remote --example ftpd -- <folder>` for the
  app-level checks. Docker was not running, so no real server was used.

## M7: WebDAV

- **Built on `ureq` (blocking HTTP) with the system TLS library**, so
  certificates are checked against the system trust store. XML answers
  are read with `roxmltree`.
- **The sign-in scheme is learned once, at connect**, from the server's
  challenge to a first `PROPFIND` (Basic or Digest), and sent with every
  later request. A streamed upload therefore never has to be sent twice.
  Basic over plain HTTP is not warned about yet.
- **Listing is `PROPFIND` with `Depth: 1`** (never `infinity`); the
  folder's own entry is dropped; a missing `creationdate` or
  `getlastmodified` just means "no date".
- **Uploads are streamed**: the copy engine writes into a pipe whose
  other end is the `PUT` body, sent from a helper thread. Nothing is
  held whole in memory. The body goes out chunked, since the size is not
  known up front.
- **Rename is `MOVE` with `Overwrite: F`**; a 412 (or 405) answer is
  "the name is taken", which raises the usual conflict prompt, and
  Replace repeats it with `Overwrite: T`. **Duplicate on one server is
  `COPY`**, done by the server.
- **Collections are addressed with a trailing slash**, files without.
- **No resume and no modification time on upload**: `PROPPATCH` of
  last-modified is not sent (most servers ignore it). A sync compares
  size and the server's own time.
- **Statuses:** 423 reads "locked by another user", 507 is "no space",
  409 on upload means the parent folder is missing.
- **Tests** run against a scripted WebDAV server
  (`crates/rustyfinder-remote/tests/support/davd.rs`; also
  `cargo run -p rustyfinder-remote --example davd -- <folder>`), with Basic,
  Digest and anonymous modes. HTTPS itself is not exercised.

## M7: Amazon S3 and compatible services

- **Built on `aws-sdk-s3`** (as the brief asks), driven from a small
  tokio runtime of its own inside the backend; the blocking `Vfs` calls
  `block_on`. The SDK's TLS stack is rustls with aws-lc, which builds
  here without cmake.
- **Path model:** `/` lists buckets, `/bucket/prefix/key` is an object.
  Folders are key prefixes; a zero-byte `prefix/` object is shown as the
  folder itself and never as a file. A folder that has nothing under it
  and no marker does not exist (listing it is "not found").
- **Compatible services use path-style addressing** (`host/bucket/key`);
  Amazon itself uses the SDK's default (virtual-hosted). An endpoint on
  `localhost` / `127.*` without a scheme is plain HTTP (a local MinIO).
- **Checksums only when the service requires them**, since several
  compatible services reject the newer trailing-checksum uploads.
- **Uploads:** a file smaller than one part (16 MB) goes up in a single
  request when the engine flushes; a larger one goes up as a multipart
  upload, part by part as it is written, and is completed on flush. A
  writer dropped before that (Stop, an error) aborts the multipart
  upload, so no orphaned parts remain. Content-Type comes from the file
  extension in both cases.
- **Rename is copy then delete**, for a folder object by object under
  its prefix (batch delete where the service has it). **Duplicate within
  the service is a server-side copy.**
- **New Folder at the top creates a bucket** (in the connection's
  region); elsewhere it creates a `name/` marker.
- **Not built:** MFA / session tokens, storage classes, cross-region
  bucket redirects (S3-08), "Copy URL", parallel part uploads, resume,
  Backblaze B2's native API and Rackspace Cloud Files (B2 works through
  its S3-compatible endpoint).
- **Tests** run against a scripted S3 server
  (`crates/rustyfinder-remote/tests/support/s3d.rs`; also
  `cargo run -p rustyfinder-remote --example s3d`) that keeps objects in
  memory, pages listings and implements multipart uploads. It does not
  check signatures, and no real S3 or MinIO was contacted.

## M7: Network shares and workspaces

- **SMB, AFP and NFS are mounted by the system** (NetFS), then browsed
  as local folders, so every local feature applies. With a share in the
  address nothing is shown while mounting; without one the system's own
  picker chooses the share (RustyFinder does not list shares itself). A `vnc://`
  address is handed to Screen Sharing.
- **Mount failures** read "Can’t mount network drive: (code)" plus a
  plain reason for the common codes.
- **Scripted checks never mount anything**: there the request is only
  recorded. So mounting itself is unverified (SM-01..06).
- **A workspace is a saved window**: the same record the session restore
  uses (frame, panes, split, tabs with pins and view settings), under a
  name. Opening one opens it as a window and closes the window it
  replaces; Option (or the sidebar's Option-click) opens it beside.
  "Update Workspace" writes the current window over the workspace it
  came from.
- **Names are unique** (compared without case or accents); remote tabs
  in a workspace reopen at the starting directory, like the session.
- **Not built:** favorite kinds beyond folders (files, servers, favorite
  paths, synclets, rename presets), the Favorite manager, custom icons,
  favorite groups UI, iCloud sync (FV-02, 03, 04, 07, 08).

## M8: Settings, shortcuts, README

- **Settings is one AppKit window** with General, Transfers and
  Shortcuts. Every control applies and saves at once; each section has
  Restore Defaults. Editing (editor apps), Tags and Themes are not built;
  Tools has its own window, reachable from General.
- **The default shortcuts are whatever the menus were built with.** At
  launch the shell hands the core every command (named by its menu path)
  and its default; the core lays the saved changes over them, and the
  result is written back onto the menu items. So the menus always show
  the binding in force, and only differences are saved
  (`shortcuts.json`).
- **Conflicts** are found in the core: binding a taken combination names
  the holder and changes nothing until Reassign, which leaves the other
  command without a shortcut.
- **The Globe (fn) key is a modifier** in bindings, because macOS binds
  Full Screen to it. (Leaving it out turned that binding into a bare "F"
  that swallowed typing: caught by the type-to-select checks.)
- **Bare-key and Option bindings only fire when their command can run.**
  During a rename a bare Delete erases a character.
- **Menus that fill themselves** (Tools, Workspaces, Recent Folders,
  Connections) are not in the list; tools keep their own shortcut field.
- **Fixed on the way:** closing a window aborted inside Quick Look when
  the preview pane had never been shown (its view was created only to
  be closed). Found by the workspace checks, which close a window.
- **Fixed after looking at an offscreen render:** the text preview had
  no height (the split between preview and Info started at zero). It now
  starts with the upper 40 % and cannot be left collapsed. The scripted
  checks had missed it because they read the text, not the layout.
- **P2 items not built:** cloud drives, App Deleter, app extensions,
  themes, the welcome flow, iCloud sync of favorites.

## Views: per-folder settings (2026-11-16)

- views.md §6 says view settings are saved per folder and reapplied when
  the folder opens. A folder without settings of its own opens with the
  defaults ("Use as Defaults"), not with whatever the tab showed last, so
  VW-07's "sibling folders are unaffected" holds. Finder instead lets an
  unset folder inherit the window's current view; we follow the spec.
- Every change of layout, sort or grouping made in a folder is remembered
  for that folder at once (no separate "always open in" checkbox).
  "This Folder and Subfolders" stores a recursive entry and clears the
  subtree's own entries.
- Column view keeps its layout while browsing inside it; otherwise every
  subfolder without settings would drop back to the default layout.
- A tab restored from a saved window keeps its saved view on the first
  load.
- Stored in `folder_views.json`, keyed by source (`local` or the server's
  id) and path. Hidden files stay a global setting; the filter is per visit.
- Display options (bold folders, font, previews, item info, grid spacing,
  column fitting) apply to every folder and live with the app's other small
  preferences, not per folder.

## Servers, transfers and resume (spec gaps)

- **Partial files are recorded, not guessed.** A stopped or broken
  streamed copy keeps its partial target only where both sides can resume
  (`Vfs::resume_support`), and records it in `partials.json` (target and
  source file system ids, paths, the source's size and date). A smaller
  file at the target is "partial" only with a matching record; anything
  else is an ordinary conflict (conflicts §6). A full disk or a refusal
  removes the partial file as before. Resume is checked: the final size
  must equal the source's.
- **Resume per protocol:** SFTP reads and writes at offsets (every
  server); FTP downloads with `REST`, uploads with `REST`+`STOR` or `APPE`;
  WebDAV and S3 downloads with an HTTP Range (a server that ignores it
  sends everything and the start is skipped); WebDAV and S3 uploads cannot
  resume.
- **Retries:** automatic retries (Settings → Transfers) only for
  transient failures (I/O, "busy"), 2 s doubling to 30 s, and a retry
  continues from the partial file. "Retry after N s" answers (WebDAV 429/503
  with Retry-After, S3 SlowDown) are waited out up to 5 min, at most 10
  times per item, without counting as retries.
- **Bandwidth limits** are two token buckets in the activity manager, one
  per direction; a transfer to a server counts as upload, one from a
  server to this Mac as download. Archives are not servers.
- **Folder Replace that would delete target-only items is always asked**,
  even under a Replace policy or "apply to all": the prompt names the
  items, and Replace there is confirmed once more.
- **Names on servers are matched after NFC** when no byte-exact match
  exists (one extra listing, only for non-ASCII names).
- **russh-sftp is vendored** (`vendor/russh-sftp`, Apache-2.0, see its
  VENDORED.md): names that are not UTF-8 become one private-use character
  per byte (U+10FF00 + byte) and go back as the same bytes, so such files
  can be opened, renamed and deleted (SF-09, EC-10), and a per-connection
  encoding can decode them. It also gains `posix-rename@openssh.com`, so a
  replacing upload never leaves the target missing (SF-10), and owner and
  group names from each entry's longname. APFS refuses non-UTF-8 names, so
  the round trip is tested at the protocol layer, not with a real file.
  The row presenter shows those characters as U+FFFD; paths keep them.
- **A dead SFTP session is opened again once**, on the first request that
  finds it dead (I/O error, timeout, closed channel, or the ssh process
  gone), then the request is repeated. Open file handles are not carried
  over: a transfer that breaks is retried (and resumed) by the engine.
- **Changed host key:** ssh refuses and prints the new fingerprint. "Connect
  Anyway" removes the old record with `ssh-keygen -R` (which keeps a .old
  copy) and accepts exactly the key whose fingerprint was shown; a
  different key is asked about again.
- **Certificate trust is pinned on the connection itself.** FTP uses its
  own TLS connector and WebDAV a ureq connector chain that set the system's
  verdict aside and accept only the certificate with the trusted SHA-256
  fingerprint, checked on every connection (FTP data channels included).
  Trusting a certificate as a root does not work on macOS for many
  self-signed ones (longer than 825 days is refused). Trusted certificates
  live in `trusted-certificates.json` (host, port, DER), not the keychain:
  they are public data. "Always Trust" stores the certificate only if the
  server still presents the one the user saw.
- **WebDAV Basic over plain HTTP is a question**, asked before any password
  is sent; Digest needs none.
- **S3 buckets in another region** are found from `x-amz-bucket-region`
  and remembered per bucket with a client for that region.
- **FTP keep-alive** is a NOOP on sessions idle 60 s (a thread per
  connection that holds only a weak reference); data connections time out
  after 30 s of silence; a stopped download closes its control connection
  first so dropping it never waits on the server.
- **15 s guard on network volumes:** `LocalFs` list and stat on SMB, NFS,
  AFP and WebDAV mounts run on a helper thread with a deadline. The volume
  type comes from the kernel's cached mount table (`MNT_NOWAIT`).
- **Saved connections** (`connections.json`) are the store behind "Add to
  Favorites" in the Connect panel; passwords stay in the keychain. Favorite
  kinds and the favorites UI that use them are built elsewhere.
- **Connect decisions** (host key changed, certificate, unencrypted
  password) are one alert flow in `ConnectionCenter`, Cancel first and
  default, and the attempt is repeated once with the user's choice.
