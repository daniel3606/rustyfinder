# File views: List, Column, Icon (P0)

## 1. View modes

Each tab shows its location in one of three modes. The keys apply to the active pane:

| Mode | Key | Summary |
|---|---|---|
| Icon | ⌘1 | Grid of icons or thumbnails with names below |
| List | ⌘2 | Table with sortable columns and expandable folders. **Default mode** *(RustyFinder choice)* |
| Column | ⌘3 | Miller columns, one column per folder level |

The View menu shows a checkmark next to the current mode only.

Switching modes keeps the location and selection. Switching from Column view to another mode shows the **deepest folder whose contents were being shown** in Column view, never a parent or a sibling.

## 2. Shared behavior (all modes)

### 2.1 Selection
- Click selects one item. ⌘-click toggles an item. ⇧-click selects a range from the anchor (all modes, Icon view included). Dragging a rectangle on empty space selects the items it touches (Icon view; List view may do the same).
- ⌘A Select All. ⌥⌘A Deselect All. "Invert Selection" (menu; a modifier variant of Deselect All). "Select All Files" and "Select All Folders" (menu).
- Arrow keys move the selection. ⇧+arrow extends it. Home/End jump to the first/last item.
- **Type-select:** typing printable characters quickly (each within about 1 s *(RustyFinder choice)* of the last) selects the first item whose name starts with the typed text, ignoring case and diacritics. Option+letter is never used for type-select, because those combinations may be bound to commands.
- After an operation that creates or reveals items (duplicate, create archive, new folder, paste), the new items are selected and scrolled into view.

### 2.2 Name display
- Hidden items (dot-files and items with the hidden flag) are shown only when "Show Invisible Files" is on, and then in a lighter text color.
- File extensions follow the system "hidden extension" flag per file. After an inline rename, that flag still applies.
- Names that can't be decoded on remote servers show the Unicode replacement character (�) for the bad bytes (see remote-ftp/sftp).
- Locked items show a lock badge on folders. Aliases and symlinks show an alias arrow badge.
- Items in cloud storage that aren't downloaded (iCloud Drive) show a cloud badge (see cloud.md).
- Up to **three** color-tag dots appear next to the name. With more than three tags, the first three are shown in tag order. Two tags with the same color appear as separate dots.
- Folders with a custom color or custom icon (set in Finder) are shown with it. RustyFinder itself doesn't need to offer custom folder icons.

### 2.3 Text size
⌘+ and ⌘− change the text size in List and Column view, and the icon size in Icon view. Each mode stores its own value.

### 2.4 Inline rename
Return (or a slow second click on the name) starts inline rename of a **single** selected item. With several items selected, Return opens Multi-Rename (multi-rename.md). The base name is pre-selected and the extension is not. Return commits, Esc cancels. Renaming to an existing name reports a conflict (conflicts.md). Delete or Backspace inside the editor edits text and is never sent to the "delete item" command, even when that command is bound to a bare key.

### 2.5 Folder sizes
"Calculate Folder Size" (⌘⇧S) computes the recursive size of the selected folders and shows it in the Size column, the Info area and the status line. The command is disabled when only files are selected. "Calculate All Folder Sizes" (File menu, available with no selection) does it for every folder in the current view. Calculated sizes stay until the folder changes or the view reloads. Calculating never changes the sizes shown for files.

## 3. List view

### 3.1 Columns
Name is always present and always first. Other columns can be toggled by right-clicking any column header, and also in View Options:

Date Modified, Date Created, Date Added, Date Last Opened, Size, Kind, Tags, Owner, Group, Permissions, Comments, Duration, Dimensions, Title, Authors, Album, Musical Genre, Track Number, Year Recorded, Rating, Codecs, Version, SCM Status.

Default visible columns: **Name, Date Modified, Size, Kind** *(RustyFinder choice; Date Last Opened is explicitly not a default)*.

- Columns can be reordered by dragging headers. Name stays first. The order is remembered.
- Columns can be resized. **Double-clicking a header divider** sizes the column to fit its widest content.
- "Always resize name column": the Name column absorbs any change in window width while the other columns keep their widths. Column order is still respected when this is on.
- "Resize columns to fit": all columns are fitted to their content. After a manual resize, auto-fit doesn't snap back immediately, only on the next content change.
- Metadata columns (Duration, Dimensions…) are empty for items that don't have that metadata, and are filled lazily for local items.

### 3.2 Rows
- Icon size choices: 16 px and 32 px, at least *(the reference also has intermediate sizes)*. "Show icon preview" draws thumbnails instead of generic icons. A sub-option controls thumbnails for remote files (default off, to save bandwidth *(RustyFinder choice)*).
- "Use alternating background" (default off *(RustyFinder choice)*).
- "Bold folder names" (default off). Auto-fit width accounts for bold text.
- Font family and text size are configurable. Option-clicking the selected font in View Options goes back to the system font.
- "Use relative dates": dates show as "Today, 14:03", "Yesterday, 09:10", and otherwise in the normal date format.
- **Expandable folders:** each folder row has a disclosure triangle. ⌘→ expands the selected folders and ⌘← collapses them (⌥ variants act recursively *(RustyFinder choice)*). Expanded rows are indented. Selection, copy and delete work on items inside expanded folders. The status line counts visible rows.

### 3.3 Sorting
- Clicking a header sorts by that column. Clicking it again reverses the direction. The header shows the direction.
- Menu sort keys (⌃⌘ + digit): 1 Name, 2 Kind, 3 Date Added, 4 Date Modified, 5 Date Created, 6 Size, 7 Tags, 8 Owner, 9 Group, 0 Permissions. ⌥⌘O toggles "Descending Order".
- First click on a **date or size** column sorts **descending** (newest or largest first). Name and text columns sort ascending first. The last choice is remembered for that tab.
- Name ordering is **natural and localized**, like the system file browser: case-insensitive, diacritic-aware, and numeric runs compared by value ("file2" < "file10").
- "Show folders on top" (default **on** *(RustyFinder choice)*) keeps folders before files whatever the sort key. "Always sort folders alphabetically" (default off) sorts the folder block by name even when files are sorted by another key.
- Ties are broken by name ascending.
- Sorting and grouping stay stable after rename and delete. The view doesn't jump. The renamed item stays selected and visible.

### 3.4 Grouping
"Group by" (View menu, toolbar button; ⌃⌥⌘ + digit): 0 None, 1 Kind, 2 Date Added, 3 Date Modified, 4 Date Created, 5 Size, 6 Tags. Option-clicking the toolbar Group-by button opens the sort-key menu instead.

- Groups have headers and are ordered sensibly: date groups newest first; size groups smallest first *(RustyFinder choice)*.
- Date buckets, in the user's **local time zone**: Today, Yesterday, Previous 7 Days, Previous 30 Days, then by month for the current year, then by year.
- Size buckets: Less than 10 KB, 10 KB to 1 MB, 1 MB to 100 MB, 100 MB to 10 GB, More than 10 GB.
- Kind groups: when "Show folders on top" is on, the Folder group comes first.
- Tags groups: an item is placed by its first tag. Untagged items go in a "No Tags" group, which comes last. Aliases and symlinks are grouped by their **own** tags.
- Inside each group, items follow the current sort key.

## 4. Column view

- Each column lists one folder level. Selecting a folder opens its contents in the next column. Selecting a file shows a preview column with a thumbnail and basic info *(RustyFinder choice; may be omitted when the preview pane is visible)*.
- ← / → move between columns. ⌘↑ goes to the enclosing folder and keeps the column chain consistent (the parent's column becomes the last one, with the child still selected).
- Column widths can be resized individually or all at once (⌥-drag). Double-clicking the divider under a column fits it to its widest name. Widths persist across restarts.
- "Resize columns to fit" fits each column to its content, leaving room for the scroll bar and bold names.
- ⌘+ / ⌘− change the font size.
- Right-click on an item opens its context menu without navigating.
- Dragging items out of Column view works for both local and remote items. Remote items are downloaded to a temporary place for the drop.

## 5. Icon view

- Icon size slider (16–512 px, *RustyFinder choice*). ⌘+ / ⌘− step through sizes. Grid spacing, text size and font are configurable in View Options.
- "Show item info" puts a second line under the name: file size for files, item count for folders, pixel dimensions for images *(RustyFinder choice for the image case)*.
- "Show icon preview" shows thumbnails, including for items inside archives.
- Items are arranged in sort order and stay aligned to a grid. Free positioning is not required.
- Keyboard: arrows move in 2-D. → at the end of a row goes to the first item of the next row, and ← at the start of a row goes to the last item of the previous row. Home/End go to the first/last item. Moving the selection with the keyboard scrolls only as much as needed and never re-centers the item.
- Dragging items over a folder icon and pausing spring-loads it (opens it) with a visible highlight. Dropping without pausing transfers into the folder and **does not** open it.

## 6. View settings scope (View Options, ⌘J)

- View settings (mode, sort, grouping, columns, icon or text size, options above) are saved **per folder**. They are reapplied whenever that folder is opened in any tab or window.
- The View Options panel has an "Apply to" choice: **This Folder** or **This Folder and Subfolders**. The second stores the settings recursively for the subtree.
- "Use as Defaults" (also View menu → "Save View Settings as Defaults") makes the current settings the default for folders without their own.
- "Restore Default View Settings" (⌥⌘R) removes the folder's own settings and applies the defaults.
- Remote locations use the same mechanism, keyed by server and path.

## 7. Acceptance tests

- **VW-01** Given a folder with `a2.txt, a10.txt, A1.txt`, when sorted by Name ascending, then the order is A1, a2, a10.
- **VW-02** Given a List view sorted by Name, when the user first clicks the Size header, then the largest file comes first.
- **VW-03** Given "Show folders on top" is on and the view is sorted by Size descending, then every folder comes before every file.
- **VW-04** Given Group by Date Modified, a file modified today and one modified 3 days ago, then the first is under "Today" and the second under "Previous 7 Days". This holds when the machine's time zone is not UTC.
- **VW-05** Given a 5 KB file and a 2 GB file grouped by Size, then they fall into "Less than 10 KB" and "100 MB to 10 GB".
- **VW-06** Given Column view shows /a/b/c with the c column open, when the user switches to List view, then the location is /a/b/c.
- **VW-07** Given a folder set to Icon view with "This Folder", when the user leaves and comes back, then Icon view is restored there and sibling folders are unaffected.
- **VW-08** Given "This Folder and Subfolders" is applied at /p with List view sorted by Size, when /p/q is opened, then it shows List view sorted by Size.
- **VW-09** Given a List view, when the user right-clicks a header and enables "Comments", then the Comments column appears and is still there after relaunch.
- **VW-10** Given only files are selected, then "Calculate Folder Size" is disabled. Given a folder of 3 MB is selected, when it runs, then the Size column shows 3 MB for that folder.
- **VW-11** Given Icon view with 4 items per row and the last item of row 1 selected, when → is pressed, then the first item of row 2 is selected.
- **VW-12** Given inline rename is active, when the user presses Backspace, then a character is deleted from the name and no file is deleted, even if delete is bound to Backspace.
- **VW-13** Given a file with 5 color tags, then exactly 3 dots are shown.
- **VW-14** Given hidden files are shown, then `.zshrc` is drawn in a lighter color than `notes.txt`.
- **VW-15** Given grouping by Tags, then an alias carrying tag Red is grouped under Red even if its original has no tags.
