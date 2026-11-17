# Search and filter (P1)

## 1. Entry point

- A search field in the toolbar. ⌘F focuses it. If the user removed the search field from the toolbar, ⌘F and Find are disabled.
- A magnifying-glass menu in the field sets the **criteria** (§2), the **engine** (§3) and **Search in Subfolders**.
- **Filter-as-you-type** (no subfolders): typing filters the current folder's listing live, by name. Return isn't needed. The status line shows "Filtered: N".
- **Search** (subfolders on, Spotlight, or content): starts when the user presses Return. The active tab switches to a **search result** location with the label `Search: "term"`, and its back history returns to the folder. Results stream in while the search runs. A spinner and a Stop button show while it runs.
- Esc in the field, or clearing it, ends filtering or search and goes back to the folder.

## 2. Criteria

Search by one of:
- **Name** (default): case-insensitive, diacritic-insensitive substring. Wildcards `*` and `?` are supported.
- **Extension**: matches the extension exactly (case-insensitive). Comma-separated lists are allowed.
- **Kind**: Folder, Image, Movie, Audio, Document, Archive, Application, Text, Other.
- **Tags**: items carrying the named tag.
- **Content**: files whose text contains the term (§4).
- Options: **Case sensitive** (off), **Regular Expression** for name (off).

## 3. Engines

| Engine | Where it works | How |
|---|---|---|
| **RustyFinder search** (default) | Local and **remote** locations, and inside archives | Walks the folder tree itself, item by item. Slower, but exhaustive and works anywhere. |
| **Spotlight** | Local, indexed volumes only | Queries the system metadata index scoped to the current folder. Near-instant. Name and content. |

- With Spotlight, a remote location falls back to RustyFinder search automatically and says so in the field *(RustyFinder choice)*.
- The local and remote engine choice and the subfolder toggle are remembered separately for local and remote locations.
- Quote characters in the term must be escaped for Spotlight queries. Searching `he said "hi"` must not crash or error.

## 4. Content search (RustyFinder engine)

- Reads files as text. Encoding is detected as in preview-quicklook §3. Binary files (for example a NUL byte in the first 8 KB) are skipped.
- Works with and without "Search in Subfolders". Without it, only files in the current folder are read.
- Remote content search downloads each candidate. A size cap applies (*RustyFinder choice:* skip files over 10 MB remotely, and say so).

## 5. Result list behavior

- Results show a **Path** (or "Where") column in List view by default. Columns, sorting and grouping work as in views.md.
- Every file operation works on results (copy, move, delete, rename, Quick Look, multi-rename, tags), including results inside archives and remote results. Copying several results with the same name into one folder is refused (file-ops §2.3).
- "Open in New Tab" or "Show in Enclosing Folder" on a result opens its real parent folder, with the correct path and icon.
- A result location can be saved as a favorite ("saved search"). It re-runs the search when opened *(RustyFinder choice; must work, or the option is hidden)*.
- Info for a result shows the full Info content, Permissions included.
- Closing the tab or window during a search cancels it without hanging.

## 6. Tags as a saved search

Clicking a tag in the sidebar runs a Tags search across the user's home folder (Spotlight). The tab shows the tag icon and the tag name. See tags.md.

## 7. Acceptance tests

- **SE-01** Given a folder with `alpha.txt, beta.txt`, when the user types `alp` (subfolders off), then only `alpha.txt` is shown and the status line says Filtered: 1.
- **SE-02** Given "Search in Subfolders" and term `report`, when Return is pressed, then files named `*report*` at any depth are listed with their paths, and Back returns to the folder.
- **SE-03** Given an SFTP location and Spotlight selected, when a search runs, then results come from the RustyFinder engine.
- **SE-04** Given Content search for `needle` with subfolders **off**, then a file in the current folder containing `needle` is found.
- **SE-05** Given Extension search `jpg,png`, then only files with those extensions are listed.
- **SE-06** Given a Spotlight search for a term with a double quote, then no crash and no error dialog.
- **SE-07** Given a search running over a large tree, when the tab is closed, then the app stays responsive and the search stops.
- **SE-08** Given a search result inside a .zip, when it is copied to a local folder, then the file appears there with the right contents.
