# Preview pane, Info, Quick Look (P0)

## 1. Preview pane

The preview pane sits at the right edge of the window. It is toggled with ⌘⇧P, and its width can be changed (up to at least 50 % of the window). Buttons at its top switch between three modes:

1. **Preview / Info:** the default mode, described below.
2. **Activities:** see file-ops-transfer-queue.md §5.
3. **Log:** see file-ops-transfer-queue.md §6.

Switching to Activities or Log and back to Preview shows the current selection's preview again; it must not come back blank.

The pane follows the **active pane's** selection. When the selection changes quickly (arrow-key scrolling), the preview updates after a short pause (*RustyFinder choice:* 150 ms), so expensive previews aren't built for items the user only passed over. If the pane is opened while an item is already selected, that item's preview appears right away.

### 1.1 Preview area (top)

| Content | Behavior |
|---|---|
| Images | Fitted to the pane. Animated formats animate. |
| PDF | Scrollable. Text can be selected. The context menu offers display options (single or continuous page, zoom), and those choices are remembered across items and launches. |
| Audio / video | Inline player with play/pause and scrubbing. If a system Quick Look extension adds codecs, previews use it. |
| Text and source code (local and remote) | Shown in an **editable** text area: "quick edit in place". A Save action (⌘S while the text area has focus) writes the file back. Remote files are written back by upload. The text encoding is auto-detected (§3) and can be changed from Format → Text Encoding. Changing the encoding re-reads the file and doesn't modify it. |
| Email (.eml), Office documents, other types | Whatever the system's Quick Look thumbnail or preview provides. |
| Folders | Folder icon. |
| Multiple items | A stacked icon and a summary (§1.2). |
| Aliases and symlinks | A preview of the target. |

**Remote items:** the preview downloads the file to a temporary cache. Files above a size threshold (*RustyFinder choice:* 10 MB) aren't downloaded automatically. Instead a download (cloud) button appears under the icon. ⌘R (Refresh) on a remote location also refreshes the preview of the selected item. **iCloud items** that aren't downloaded show the same cloud button, which triggers a download (see cloud.md). RustyFinder must never download them automatically just to build a preview.

### 1.2 Info area (bottom)

There is a divider between the preview and info areas. Its position is remembered.

**Single item:**
- Name, kind, and size. Size is shown human-readable **and** as an exact byte count. For folders, a Calculate button computes the recursive size. For local folders, the info also shows the number of items inside once the size is known.
- Created, Modified, Last Opened, and Added dates.
- Where: the full path, clickable.
- Tags: an editable field with autocompletion from the tags list in Settings → Tags. It expands to show every tag. Tab accepts a completion. Esc leaves the field.
- Comments: an editable Finder comment. Saved when the field loses focus. Must show up in Finder.
- Flags: "Hidden" (sets or clears the hidden flag), "Locked", "Hide extension".
- **Open with** (files): a menu of applications for this file. Choosing one changes it **for this file only**. "Change All…" asks for confirmation and then makes that app the default for every file with the same extension. The change must persist (re-select the file, and the menu shows the new app).
- **Original** (aliases and symlinks): the target path, plus target information (kind, size, dates). The alias target can be **edited**: pick a new target.
- **Permissions:** owner, group and others, each with read/write/execute checkboxes; owner and group names; and an **Octal** field kept in sync with the checkboxes in both directions. ACL presence is shown as "R ACL" / "W ACL" when present. "Apply to Enclosed Items" applies the current permissions recursively to every subitem and reports progress as an activity. On SFTP the same section works through remote setstat. On FTP it works through SITE CHMOD where the server supports it.
- **More Info:** metadata depends on type. Images: dimensions, color space, color profile, camera model, lens model, aperture value, exposure time, exposure program, F number, focal length, ISO speed, flash, metering mode, white balance, latitude and longitude. Audio and video: duration, codecs, sample rate, bits per sample, audio channels, title, album, authors and performers, genre, track number, year recorded, rating. Documents: pages, title, authors, encoding software, content creation date. Video dimensions are shown as positive numbers whatever the rotation.
- **SFTP** items also show Owner and Group.

**Several items selected:** "N items", the total size (calculated on demand for folders), and aggregated dates. A date is shown when it is the same for all items. Otherwise the earliest–latest range is shown, or it is marked as mixed. Tags and Comments can be edited for all selected items at once (adding a tag adds it to all of them).

### 1.3 Get Info window (⌘I)
This window shows the same Info content in a separate window, one per item (*RustyFinder choice:* at most 10 windows; with more items, one combined window opens). It stays live: it updates when the item changes.

## 2. Quick Look

- **⌘Y** (and Space when the file list has focus *(RustyFinder choice; conventional)*) opens the system Quick Look panel for the selection. Arrow keys in the file list move through items while the panel stays open. Pressing ⌘Y or Space again, or Esc, closes it.
- **Slideshow** (⌥⌘Y) opens Quick Look full-screen and steps through the selection automatically.
- **Remote items** are downloaded to a temporary cache first, with a progress indicator in the panel. This works for Office, mail and PDF types as well as images.
- Aliases and symlinks preview their **target**.
- Edge cases that MUST work: names that begin with a space, names with emoji or combining characters, an item that has just been renamed, and items inside archives (extracted on demand).
- Quitting the app while Quick Look is open must not crash or leave the panel orphaned.

## 3. Text encoding detection (preview and quick edit)

1. A BOM, if present, decides the encoding (UTF-8, UTF-16 LE or BE, UTF-32).
2. If the bytes are valid UTF-8, the file is treated as UTF-8.
3. Otherwise a statistical charset detector picks the most likely legacy encoding (Japanese Shift JIS/EUC/ISO-2022-JP, Chinese GB/Big5/EUC, Korean EUC, Cyrillic, Greek, Hebrew, Arabic, Central European, Western Latin-1/Mac Roman, Thai, Turkish and so on).
4. If it can't decide, the file opens as Mac OS Roman *(RustyFinder choice)*, so every byte still displays.

The Format → Text Encoding menu lists at least: Unicode (UTF-8), Western (Mac OS Roman, ISO Latin 1, ISO Latin 3, ISO Latin 9, NextStep), Central European (ISO Latin 2, ISO Latin 4, Mac OS), Cyrillic (ISO 8859-5, Mac OS), Greek, Hebrew, Arabic (ISO and Mac OS), Baltic (ISO Latin 7), Nordic (ISO Latin 6), Romanian (ISO Latin 10), Turkish (ISO Latin 5, Mac OS), Icelandic (Mac OS), Thai (ISO 8859-11), Japanese (EUC, ISO 2022-JP, Mac OS, Shift JIS, Windows), Korean (EUC, Mac OS, Windows), Simplified Chinese (GB 2312, Mac OS, Windows), Traditional Chinese (EUC, Windows), Latin-US (DOS), and Non-lossy ASCII. Saving a quick edit writes in the encoding the file currently has.

## 4. Acceptance tests

- **PV-01** Given the preview pane is visible, when a JPEG is selected, then its image appears, and the Info area shows dimensions and camera model when EXIF has them.
- **PV-02** Given a text file is selected, when the user edits it in the preview and saves, then the file on disk has the new contents and its encoding is unchanged.
- **PV-03** Given a Shift-JIS text file without a BOM, then the preview shows readable Japanese.
- **PV-04** Given a remote 200 MB video is selected, then no download starts until the user clicks the download button.
- **PV-05** Given the user switches the preview pane to Log and back to Preview, then the selected item's preview is shown again.
- **PV-06** Given a file's permissions are 644 and the user ticks Group-write, then the Octal field shows 664 and the file mode becomes 664.
- **PV-07** Given folder F with nested files and "Apply to Enclosed Items" with 755, then every subitem gets mode 755.
- **PV-08** Given "Change All…" for `.md` files to app X, then any other `.md` file shows X in Open with.
- **PV-09** Given three files with tag Red and two of them also tagged Blue, then the multi-item Tags field shows Red as common. Adding Green tags all three.
- **PV-10** Given a symlink to an image, when Quick Look opens, then the image is shown.
- **PV-11** Given a file named ` leading space.txt`, when ⌘Y is pressed, then Quick Look shows it.
- **PV-12** Given a remote .docx file, when ⌘Y is pressed, then it is downloaded and previewed.
- **PV-13** Given a comment entered in the Info area, when the file is inspected in Finder's Get Info, then the same comment appears.
- **PV-14** Given the Hidden checkbox is ticked for `x.txt`, then `x.txt` disappears from the list while invisible files are hidden.
