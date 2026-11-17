# Cross-cutting edge cases (P0)

Rules here apply to every feature. Feature specs refer back to this file.

## 1. Names

- **Case-only rename** (`photo.jpg` → `Photo.jpg`) MUST work on every volume. A case-insensitive volume or server may refuse it, or treat it as a no-op. In that case RustyFinder renames through a temporary unique name (`photo.jpg` → `<temp>` → `Photo.jpg`). A case-only rename never raises a conflict with the item itself. The same applies to every row of a multi-rename (Change Case).
- **Unicode normalization:** names typed or pasted in NFD or NFC must find, match and sort the same item. Search, Quick Select and type-select are also diacritic-insensitive (typing `e` finds `é`).
- **Colon and slash:** a file whose on-disk name contains `:` is shown with `/` (the macOS display convention), and the other way round. Remote names keep their raw characters. A `/` in a remote name is impossible, and RustyFinder rejects it in rename fields with an inline message.
- **Leading space or dot, trailing space or dot, emoji, RTL characters, very long names (255 bytes UTF-8):** all must list, preview, Quick Look, copy, rename and delete correctly. A rename longer than the target's limit is refused before it is attempted.
- **Folder names containing a dot** (`project.v2`) are still folders. Their kind and icon must not be derived from the "extension". This matters on remote servers, which have no type metadata.
- **Undecodable remote names:** shown with the replacement character, but every operation (open, download, rename, delete) uses the original raw bytes, so it still succeeds.
- **Full-width spaces** and other Unicode whitespace are escaped properly in "Copy Path → Terminal".
- **Same folder name repeated in a path** (`/a/x/a/x`): navigation, path bar clicks and Sync Browsing must use whole path components, never substring matches.

## 2. Volumes and file systems

- **AppleDouble companions:** on file systems without native extended attributes (FAT/exFAT, some SMB), the hidden `._name` companions must never cause false conflicts, never be counted as separate items in the progress totals, and never be shown, even with hidden files visible *(RustyFinder choice)*.
- **Read-only volumes:** write commands are disabled or fail fast with "The target volume is read only".
- **Full volume:** see the free-space pre-check (file-ops §2). If space runs out mid-copy, the partial target file is removed. Earlier completed items stay.
- **Ejecting a busy volume:** reports which app is using it, if the system says. RustyFinder's own panes that show the volume must not block ejection: RustyFinder moves them to the starting directory first.
- **Sparse files, hard links, file clones:** copies keep sparseness (file-ops §4). Hard links are copied as independent files. Same-volume APFS copies may use clones.
- **Package bundles** count as a single item for counts, selection and Quick Look, unless "Treat packages as folders" is on or the user chooses Show Package Contents.

## 3. Links

- **Symlinks** are never followed during recursive operations (copy, delete, size calculation, search, sync). A symlink to a folder is listed as a folder for navigation (double-click enters the target), but it is copied or deleted as a link.
- **Symlink loops** never cause infinite recursion.
- Listing a folder of symlinks that point to slow or unmounted volumes must not block. Resolve link targets lazily and with a timeout (*RustyFinder choice:* 2 s each, off the UI path).
- **Broken links** show a broken-alias badge. Opening one offers "Show Original" with the missing path in an error.
- **Aliases** are resolved when opened. If resolution fails, RustyFinder says "The item can't be resolved".

## 4. Permissions and protection

- An unreadable folder shows an inline error in the pane (not a modal) with an "Authorize" action when admin elevation could help. Navigating away clears it.
- **Locked** items: rename, move and delete prompt to unlock first (or fail with a clear message).
- macOS privacy-protected folders without Full Disk Access: an inline error with a link to System Settings (panes-tabs §9).
- A folder without write permission can still be moved by renaming within its parent if the parent is writable.
- **iCloud Drive busy:** if the system reports iCloud as busy for an item, RustyFinder shows an alert explaining why the action can't run right now, instead of failing silently.

## 5. Concurrency and responsiveness

- The UI never blocks on disk or network. Listing a large folder (100,000 items) shows a progress indicator and streams in rows. Sorting happens without freezing input.
- Closing a tab or window while a search or listing is running cancels it cleanly and must not hang.
- A remote server that stops responding must not hang the app. Every network operation has a timeout (protocol specs), and the user can Stop it at any time.
- After **sleep/wake**, stale remote sessions are detected on the next operation and reconnected transparently, at most once per operation, before an error is shown.
- Background commands (keyboard shortcuts) must not act on a pane's selection while a modal pop-up (Quick Open, Go to Folder, a conflict prompt) has focus. In particular ⌘⌫ never deletes behind an open pop-up.
- Commands that only work on one item (Show Original, Show Package Contents, Show Archive Contents, Copy to as…) are disabled when several items are selected.

## 6. Dates and times

- Items with **no** modification date (some servers) are displayed with an empty date. In sorting they compare as older than every dated item. In Replace Older they are skipped (conflicts.md).
- All date grouping and display uses the local time zone. Comparisons for sync use absolute instants (UTC) plus the configured time offset (sync-compare.md).
- Rotated media report positive dimensions.

## 7. Clipboard and drag interop

- Copying files puts both file URLs and plain-text paths on the pasteboard, so pasting in a text editor gives paths.
- Drops from apps that offer file promises (Photos, Mail, browsers) are accepted, and the files are written into the drop location.
- A remote item dropped on Terminal inserts its remote path text. Dropped on another app, it is downloaded first.

## 8. Acceptance tests

- **EC-01** Given `photo.jpg` on a case-insensitive APFS volume, when it is renamed to `Photo.jpg`, then a listing shows `Photo.jpg` and there is no conflict prompt.
- **EC-02** Given the same case-only rename on an SMB share that refuses direct case-only renames, then the result is still `Photo.jpg`.
- **EC-03** Given a file named with NFD `é`, when the user types `e` in Quick Select, then the file matches.
- **EC-04** Given folder `a.b` on an SFTP server, then it is shown with a folder icon, kind Folder, and double-click enters it.
- **EC-05** Given a symlink `loop → .` inside folder F, when F is copied, then the copy finishes and contains `loop` as a symlink.
- **EC-06** Given a copy to an exFAT drive of a folder whose files carry tags, then no conflict is reported on a second, identical copy except for the real files, and no `._` items are listed.
- **EC-07** Given a remote server that stops responding during listing, then within the protocol timeout the pane shows an error and the app stays responsive the whole time.
- **EC-08** Given the Quick Open pop-up is open and the user presses ⌘⌫, then no file is moved to the Trash.
- **EC-09** Given two items are selected, then "Show Original" is disabled.
- **EC-10** Given a remote file whose name contains invalid UTF-8, when it is downloaded, then the local copy holds the same content, and its name is shown with the replacement character.
