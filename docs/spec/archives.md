# Archives: browse, extract, compress (P1)

## 1. Browsing archives as folders

- When "Treat archives as folders" is on (settings-defaults §1), double-clicking or ⌘↓ on a supported archive opens it **as a folder location**, read-only. "Show Archive Contents" (single selection only) does this whatever the setting is.
- This works for archives stored locally **and** on remote servers. A remote archive is downloaded to a temporary cache first, with progress shown.
- Inside an archive: all views, sorting, grouping, filtering, search (RustyFinder engine), Quick Look, preview and Info work. Copy **out** (to the other pane, by drag, or by Copy/Paste) extracts only the needed entries. Icon previews of entries work in Icon view.
- Write operations inside an archive (rename, delete, new folder, copy in) are disabled *(RustyFinder choice; the reference shows archives read-only)*.
- The path bar shows the archive as a path segment (`…/backup.zip/docs`). ⌘↑ at the archive root leaves the archive and selects it in its parent folder.
- Nested archives (a zip inside a zip) can be opened the same way.
- **Supported read formats** (at least): zip (including zip64 and legacy encodings), 7z, rar (v4 and v5), tar, tar.gz/tgz, tar.bz2/tbz, tar.xz/txz, gz, bz2, xz, lzma, zstd, cpio, ISO 9660, xar/pkg, lha/lzh, StuffIt (sit/sitx), cab. An unsupported or corrupt file gives "Unrecognized archive format" and is not opened.
- **Name encoding inside zip:** if entry names aren't flagged as UTF-8, detect the encoding (Japanese CP932, Cyrillic CP866, Western CP437 and so on) with the same statistical detector as text preview.

## 2. Encrypted archives

When an archive or entry needs a password, a prompt asks for it ("Enter password for the archive …"). A wrong password asks again. Cancel aborts. The password is remembered for that archive until the location is closed (not stored on disk).

## 3. Extract

| Command | Key | Result |
|---|---|---|
| Extract | ⌘⇧X | Extracts each selected archive into the **same folder** as the archive |
| Extract to | ⌥⌘⇧X | Extracts into the **other pane's** location |

Rules:
- Archive with a single top-level item: that item is extracted directly. Several top-level items: they go into a new folder named after the archive (without extension), so the destination isn't scattered *(RustyFinder choice; matches the system Archive Utility)*.
- Name clashes with existing items follow conflicts.md (the extracted folder clashing with an existing folder raises a folder conflict).
- Modification times, POSIX permissions and symlinks stored in the archive are restored. Entries with absolute paths or `..` components are extracted safely inside the destination: path traversal is **blocked** and logged.
- Extraction is an activity, "Extracting …", with progress and stop. A stopped extraction removes only the partially written current file.
- Extracting from a remote archive downloads it first. Extracting **to** a remote location extracts to a temporary local folder, then uploads.
- If the archive can't be extracted, the error names it ("The archive … can't be extracted") and gives the reason (corrupt, wrong password, unsupported).

## 4. Compress

| Command | Key | Result |
|---|---|---|
| Compress | ⌘⇧M | Creates a zip next to the items with no further questions |
| Compress with Options… | ⌥⌘⇧M | A dialog: archive name, format, optional password, then create |

Default archive name:
- one item `report` (file or folder) → `report.zip`. A file's extension is replaced: `notes.txt` → `notes.zip` *(RustyFinder choice; the reference keeps names with trailing numbers intact, e.g. `scan 2.pdf` → `scan 2.zip`)*
- several items → `Archive.zip`
- if the name is taken → `Archive 2.zip`, `Archive 3.zip`, …

Options dialog:
- **Archive name** (prefilled as above; the extension updates with the format)
- **Format:** zip (default), tar.gz, tar.bz2, tar.xz, 7z *(RustyFinder choice: 7z is optional)*
- **Password** (zip AES-256 and 7z only; the field is disabled for tar formats)
- **Compression level** (store / fast / normal / best; default normal *(RustyFinder choice)*)
- Exclude macOS metadata (`.DS_Store`, `__MACOSX` resource forks) (default on *(RustyFinder choice)*)

Rules:
- When several archives are selected, Compress makes **one** archive containing them. It doesn't compress them one by one.
- Zip entries are stored with UTF-8 names and the UTF-8 flag set, with POSIX permissions and mtimes.
- Compressing on a remote location: the items are downloaded to a temporary folder, the archive is built there and uploaded to the remote folder *(RustyFinder choice; compute happens locally)*.
- When it finishes, the new archive is selected in the view.
- Progress activity: "Compressing N items". The toolbar Compress button is enabled whenever there is a selection, on Intel and Apple Silicon alike.

## 5. Acceptance tests

- **AR-01** Given `a.zip` containing `docs/readme.txt`, when it is double-clicked with "Treat archives as folders" on, then the pane shows `docs` inside `a.zip`, and ⌘↑ leaves to the parent with `a.zip` selected.
- **AR-02** Given a.zip is browsed, when `docs/readme.txt` is copied to the other pane, then the file appears there with the archived mtime.
- **AR-03** Given a.zip has 3 top-level files, when Extract runs, then a folder `a/` with the 3 files is created next to a.zip.
- **AR-04** Given an archive with a `../evil` entry, when it is extracted, then no file is written outside the destination and the log mentions the blocked entry.
- **AR-05** Given 2 files selected, when Compress runs, then `Archive.zip` is created and selected. A second Compress creates `Archive 2.zip`.
- **AR-06** Given Compress with Options with password `pw` and zip, then the archive can't be listed without the password and extracts correctly with it.
- **AR-07** Given `scan 2.pdf` is compressed, then the archive is named `scan 2.zip`.
- **AR-08** Given an encrypted zip and a wrong password, then the prompt comes back. Cancel aborts with no files written.
- **AR-09** Given a zip with CP932 (Shift-JIS) names and no UTF-8 flag, when browsed, then the names show readable Japanese.
- **AR-10** Given a remote `.tar.gz` on SFTP, when it is opened as a folder, then its contents are listed after a download activity.
