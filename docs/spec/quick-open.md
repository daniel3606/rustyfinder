# Quick Open, Go to Folder, Quick Select, Devices pop-up (P1)

These four features are keyboard-driven pop-ups. Shared rules:
- They appear centered over the window, take keyboard focus, and close on Esc or on a click outside.
- ↑/↓ move the highlighted row. Return activates it. **Double-click** activates a row too.
- While one is open, no file command is sent to the panes underneath (edge-cases §5).
- Long paths in rows are truncated in the middle on a single line, never wrapped.

## 1. Quick Open (default key: Esc in the file list)

A single search field over a sectioned result list.

### 1.1 Sources (sections)
| Section | Rows | Activation |
|---|---|---|
| Open Favorite | All favorites (folders, files, remote connections, workspaces, synclets) | Opens it, the same as clicking it in the sidebar |
| Devices | Mounted volumes and shares | Opens it in the active pane |
| Execute Command | Every menu command, shown with its **menu path** (for example "View › Sort by › Size") and its **current key binding**; custom tools | Runs the command. Disabled commands are hidden. |
| Open With | Applications that can open the selected file(s) (only when files are selected) | Opens the selection with that app |
| Multi-Rename presets | Saved rename presets (only when 2+ items are selected) | Opens Multi-Rename with that preset loaded |
| Assign Tag | Tags (only when items are selected) | Toggles the tag on the selection |

Rules:
- Tag favorites (sidebar tag entries) are **not** listed under Open Favorite.
- Tools are listed **only** under Execute Command, never under Open Favorite.
- Rows show the item's real icon (the Applications folder shows the Applications icon, and so on).

### 1.2 Matching and ranking
- Matching is case-insensitive and diacritic-insensitive. It is **subsequence fuzzy**: all typed characters appear in order. Contiguous matches, matches at word starts and prefix matches rank higher *(RustyFinder choice for the exact scoring)*.
- With an empty query, each section shows its first entries: recent first for favorites *(RustyFinder choice)*, alphabetical for commands.
- At most 50 rows in total; extra rows are cut from the lowest-ranked sections first *(RustyFinder choice)*.
- Matched characters are highlighted.

### 1.3 Quick Open Devices
A variant reachable from the menu that shows only the Devices section.

## 2. Show Devices pop-up (⌃D)
This pop-up lists mounted volumes, shares and open remote connections, with free space shown for volumes. Activating a row opens it in the active pane.

## 3. Go to Folder (⌘⇧G)

A path field with an autocompletion dropdown. It works for the **current data source**: local, or the remote server of the active tab (paths are server paths).

- `~` and `~/x` expand to the home folder (local) or the login folder (remote). On S3, `~` means the bucket list root and must not crash.
- Typed paths can be absolute or relative to the current folder.
- A path wrapped in **single quotes at both ends** (`'/tmp/my dir'`) is accepted with the quotes removed. So are Terminal-escaped paths (`/tmp/my\ dir`).
- **Autocomplete:** as the user types, the dropdown lists entries of the deepest typed folder that start with the last component (case-insensitive). Aliases and symlinks to folders are included.
  - **Tab** or **→** accepts the highlighted completion into the field, without selecting the whole field afterwards. When the completion is a folder, a single trailing `/` is added. If the field already ends with `/`, accepting must not add a second one.
  - Return opens the path as typed or completed.
- **History:** with an empty field, or with ↓ in an empty field, the dropdown shows previously visited Go-to-Folder destinations, most recent first, without duplicates. At most 20 *(RustyFinder choice)*.
- **Result:** if the path is a folder, the active pane navigates there. If it is a file, the pane navigates to the parent and **selects** the file. It never opens the file. A non-existent path shows an inline error in the pop-up, which stays open.

## 4. Quick Select (⌘S)

Selects items in the current view by pattern, without moving the cursor.

- A small field with a mode menu: **Select Matches** (replace the selection), **Add to Selection**, **Remove from Selection**.
- Pattern types (auto-detected or picked from a menu):
  - plain text: matches names containing the text, case- and diacritic-insensitive (`é` matches `e`)
  - an extension written as `.ext` or `*.ext`: matches that extension
  - wildcards `*` and `?`
  - tags: a tag name picked from the tag list matches items carrying that tag
- Matches are highlighted live as the user types. Return applies and closes. Esc cancels and keeps the original selection.
- Shows the count "N matches".

## 5. Acceptance tests

- **QO-01** Given a favorite named "Projects", when Quick Open runs and the user types `prj`, then "Projects" is among the top 3 rows and Return opens it.
- **QO-02** Given Quick Open with query `size`, then a row "View › Sort by › Size" appears with its key binding ⌃⌘6.
- **QO-03** Given no selection, then Quick Open shows no "Assign Tag" or "Open With" section.
- **QO-04** Given Go to Folder with `'/Users/me/My Docs'` (quotes included), when Return is pressed, then the pane opens `/Users/me/My Docs`.
- **QO-05** Given Go to Folder with `/Us` typed, when Tab is pressed, then the field reads `/Users/` with exactly one trailing slash.
- **QO-06** Given Go to Folder with the path of a file, when Return is pressed, then the parent folder is shown with the file selected, and the file is not opened.
- **QO-07** Given the user went to /a then /b through Go to Folder, then opening it again with an empty field lists /b before /a, each once.
- **QO-08** Given a folder with `résumé.pdf` and `notes.txt`, when Quick Select gets `resume`, then only `résumé.pdf` is selected.
- **QO-09** Given 3 `.jpg` files are selected and Quick Select in "Remove from Selection" mode gets `.jpg`, then nothing is selected.
- **QO-10** Given an S3 connection, when Go to Folder gets `~`, then the bucket list is shown and the app does not crash.
