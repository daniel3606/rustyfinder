# Multi-Rename (P1)

## 1. Opening

- Select 2+ items and press Return, or choose Rename from the menu or context menu, to open the Multi-Rename window for those items. It works for local and remote items and for items in search results (mixed folders).
- Clicking a saved **rename preset** in the sidebar opens the window with the preset loaded and the current selection as input. **Dropping** items on a preset in the sidebar renames them at once without showing the window. That drop is still undoable and still stops for conflicts.
- The window size and position are remembered.

## 2. Layout

- **Left: rules list.** An ordered list of rules. "+" adds a rule (pick its type) and "−" removes the selected one. Rules can be reordered by dragging. Each rule can be turned on or off with a checkbox *(RustyFinder choice)*. Rules apply top to bottom, each one to the result of the previous.
- **Right: preview table.** Columns are Original name and New name. Rows that change are highlighted. Rows whose new name is invalid or clashes are flagged with a reason. Rows can be **reordered by dragging**, and the order drives the Sequence rule. Initial order is the order of the file view.
- **Footer:** "N of M items will be renamed" or "No item will be renamed", plus "K items have conflicting or invalid names" when there are any. Buttons: Add to Favorites (save as preset), Cancel, **Rename**.
- The Rename button is disabled when no item would change, or when any row is conflicting or invalid.
- Tab moves between text fields and never lands on pickers by accident.

## 3. Rule types

Each rule has an **Apply to** target: Name, Extension, or Name with Extension *(the text rules)*. It also has a file/folder filter: Apply to files, Apply to folders (both on by default).

### 3.1 Change Case
Modes: **Lower Case**, **Upper Case**, **Title Case** (first letter of each word upper, the rest lower; words split on spaces, `-`, `_`, `.`; small words are still capitalized *(RustyFinder choice)*), **Sentence Case** (first letter of the name upper, the rest lower).
A case-only change of a name is never a conflict with itself (edge-cases §1).

### 3.2 Replace Text
- Fields: Find, Replace with. Options: Case sensitive (default off), **Regular Expression** (default off).
- Occurrences: All (default), First, Last.
- With a regex, the replacement supports capture group references (`$1`). An invalid regex marks the rule red and changes nothing.
- "Entire Text" mode replaces the whole target text with the replacement.

### 3.3 Add Text
- Text field. Position: **Beginning**, **End**, or **Specific Index** (0-based character index, counted from the start, or from the end with a "from end" toggle *(RustyFinder choice)*). An index beyond the length clamps to the end.

### 3.4 Add Date
- Date source: **Now** (time of preview), **Created**, **Modified**, **Custom Date** (a date picker), **Metadata** (a date tag from file metadata such as photo capture date "Original Date" or media "Content created"). Items without that metadata leave this rule unapplied, and the row shows a note.
- Format: preset formats plus a **custom format** field using Unicode date patterns (`yyyy-MM-dd`, `HHmmss`). The year uses calendar year (`y`), **not** week-based year (`Y`), so 2024-12-30 formats as 2024.
- Position: Beginning, End, Specific Index. Optional separator text.

### 3.5 Add Sequence
- Start number (default 1), Step value (default 1), Padding (minimum digits, default 1; for example 3 → `001`), Position (Beginning, End, Specific Index), optional prefix and suffix.
- Numbers follow the current row order in the preview.

### 3.6 Add Metadata
- Inserts a metadata field value (for example image dimensions, camera model, audio artist, title, track number) at a position. Missing values leave the name unchanged for that row.

## 4. Validation (live)

A row is **invalid** if its new name:
- is empty, or is `.` or `..`
- contains `/` (or `:` on local volumes, see edge-cases §1)
- is longer than 255 bytes in UTF-8

A row is **conflicting** if its new name:
- equals another row's new name (compared case-insensitively on case-insensitive targets), or
- equals an existing item in the same folder that isn't being renamed

Renames are applied in an order that avoids temporary clashes inside the set (for example `a→b, b→a` swaps go through temporary names). Such swaps must succeed and are not counted as conflicts.

## 5. Running

- Rename runs as one activity, "Renaming N items", with progress. The log records "Renamed N items" and every old → new pair.
- If a clash appears at run time (an item was created in the meantime), the conflict prompt is shown (conflicts.md). The rename **never** overwrites silently.
- Undo restores all names in one step.

## 6. Presets (favorites)

- "Add to Favorites" saves the rule list (types, options and order) under a name, in a favorite group.
- Presets appear in the sidebar and in Quick Open. Their context menu offers **Edit** (opens the window with the preset, so changes can be saved back), Rename, Delete, and Hide from Sidebar.
- Presets saved by older RustyFinder versions keep working after upgrades. The preset format must be versioned.

## 7. Acceptance tests

- **MR-01** Given `a.txt, b.txt` and the rule Add Sequence (start 1, padding 3, position Beginning, suffix `_`), then the new names are `001_a.txt, 002_b.txt`.
- **MR-02** Given `IMG_0001.JPG` with Replace Text (regex `IMG_(\d+)` → `photo-$1`, apply to Name) plus Change Case Lower on Extension, then the result is `photo-0001.jpg`.
- **MR-03** Given `x1.txt, x2.txt` with Replace Text `x\d` → `y` (regex), then both become `y.txt`, the rows are flagged conflicting and Rename is disabled.
- **MR-04** Given `a` and `b` with rules producing `b` and `a`, when Rename runs, then the names are swapped and no conflict prompt appears.
- **MR-05** Given `report.txt` with Change Case Upper on Name, then `REPORT.txt` results on a case-insensitive volume with no conflict.
- **MR-06** Given Add Date (Modified, format `yyyy-MM-dd`, Beginning, separator ` `) on a file modified 2024-12-30, then the name starts with `2024-12-30 `.
- **MR-07** Given the rows are dragged into a different order, then the sequence numbers follow the new order.
- **MR-08** Given a preset is dropped onto 5 selected items in the sidebar, then they are renamed without a window, and ⌘Z restores all 5.
- **MR-09** Given a new name containing `/`, then that row is invalid and Rename is disabled.
- **MR-10** Given Add Text "X" at Specific Index 2 on `abcd.txt`, then the result is `abXcd.txt`.
