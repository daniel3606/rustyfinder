# Folder sync and file compare (P1)

Sync Browsing (coupled navigation) is specified in panes-tabs.md §2.2.

## 1. Starting a sync

- In two-pane layout, the active pane's location is the **source** and the other pane's location is the **target**. "Sync to…" (⌥⌘⇧S, toolbar Sync button) opens the Sync window for this pair. Any pair of locations works: local/local, local/remote, remote/remote.
- "Sync to…" is disabled while either pane shows search results, or when only one pane is visible.
- Saved sync configurations (**synclets**, §6) open the Sync window directly.

## 2. Sync window

- **Header:** left path and right path, plus a **direction** selector: **left → right**, **right → left**, or **both ways**. The initial direction is source → target.
- **Options sidebar:**

| Option | Meaning | Default |
|---|---|---|
| Synchronize subfolders | Recurse into folders. When off, only top-level files are compared. | on |
| Synchronize hidden items | Include invisible items | off |
| Add items | Copy items that don't exist on the target side | on |
| Don't add empty folders | Don't create folders that would end up empty after filters | off |
| Update items | Copy items that exist on both sides but differ (§3) | on |
| Delete orphaned items | Delete items on the target that don't exist on the source (one-way directions only) | off |
| Time offset correction | A number of minutes plus a direction ("left is N minute(s) ahead" / "right is N minute(s) ahead") | 0 |
| Exclude items | A list of exclude rules (§4) | empty |

- Turning **on** "Delete orphaned items" first asks for confirmation, warning that matching target items will be permanently deleted and that this can't be undone.
- **Results list:** one row per planned action. Each row shows the relative path, the action (Add, Update, Delete), a direction arrow, and a checkbox. **Hovering** over a row expands it to show both sides' full path, modification date and size. Rows can be unchecked to leave them out, singly or with a multi-selection plus Space *(RustyFinder choice)*.
- **Footer:** counts, each with total size: **All: n (size)**, **Added: n (size)**, **Updated: n (size)**, **Deleted: n (size)**. Each count is clickable and filters the list to that category (All shows everything). The buttons show a hover state in both light and dark themes.
- **Buttons:** Add to Favorites (save a synclet), Cancel, **Synchronize**. Synchronize is disabled while analysis is running, when the plan is empty, or when every row is unchecked.
- **Analysis** starts when the window opens, and again whenever an option, filter, direction or the time offset changes. Progress shows "N folders analyzed". It always starts on the first attempt, whatever folders are involved. Analysis must be fast: listing in parallel per folder, with no content reads.

## 3. Comparison rules

For a path present on both sides (matched by relative path, with case folding when either side is case-insensitive):

1. **Kind mismatch** (file on one side, folder on the other): reported as Update in the sync direction (the target item is replaced). In both-ways mode the row is flagged "conflict" and unchecked by default *(RustyFinder choice)*.
2. **Folders** are never "updated" themselves. They are recursed into when Synchronize subfolders is on.
3. **Files are equal** when the size is the same **and** the modification times are equal within tolerance, after applying the time offset correction: the offset is subtracted from the mtimes of the side marked "ahead" before comparing (for example, with "left is 60 minute(s) ahead", a left file at 13:00 counts as 12:00).
   - Tolerance: **2 seconds** by default *(RustyFinder choice; covers FAT 2-second resolution)*. When either side is FTP without MLSD (listing gives only minute precision), the tolerance is **60 seconds**.
4. **Different** (size differs, or mtime differs beyond tolerance):
   - one-way: Update from source to target, whichever is newer *(the reference syncs "different size or modification date" one-way, so an older source still overwrites)*.
   - both ways: the **newer** file wins and is copied over the older one. If the mtimes are equal within tolerance but the sizes differ, the row is flagged "conflict" and unchecked by default *(RustyFinder choice)*.
5. **Only on source side:** Add (if Add items is on).
6. **Only on target side:** one-way: Delete (if Delete orphaned items is on), otherwise no action. Both ways: Add to the other side. Both-ways mode never deletes.
7. Items that match an exclude rule are left out entirely on **both** sides: never added, updated or deleted.
8. Hidden items are left out unless "Synchronize hidden items" is on.
9. Symlinks are compared as links (target path string) and never followed.

Executing:
- Runs as one activity, "Synchronizing N items", which shows "x of N" progress. Adds and updates go through the transfer queue rules. Deletes run after all copies finish *(RustyFinder choice; safer)*.
- Copies keep modification times (file-ops §4), so a second analysis right after a sync gives an **empty plan**.
- Conflicts never prompt during sync: the plan already decided. Errors follow the error-handling policy.
- If administrator rights are needed, the privileged-helper prompt appears. While it waits, the app stays responsive.
- The log records the number of items actually synchronized, with the breakdown.

## 4. Exclude rules

- Each rule has a field (**Name**, **Extension**, **Kind**, **Size**, **Date Modified**, **Tags**), an operator, and a value:
  - Name: is, contains, begins with, ends with, matches wildcard, matches regex
  - Extension: is (comma-separated list, no dots)
  - Kind: is Folder, Image, Movie, Audio, Document, Archive, Application, Text
  - Size: greater than or less than a value with a unit (bytes, KB, MB, GB)
  - Date Modified: before, after, within the last N days
  - Tags: has tag, doesn't have tag
- An item is excluded if **any** rule matches. A rule on a folder excludes its whole subtree.
- Rules are saved with synclets. Changing rules re-runs the analysis. A partly typed value never crashes the window (for example an empty Size value means the rule is inactive).

## 5. Behavior with a time offset field
The field accepts integers from 0 to 1440. Deleting every digit leaves the field empty, which counts as 0. It must be possible to clear the last digit.

## 6. Synclets (saved syncs)

- "Add to Favorites" saves: both locations (remote locations as favorite references, so credentials stay in the keychain), direction, all options, and exclude rules.
- Synclets appear in the sidebar. Clicking one opens the Sync window with the saved settings and starts analysis. It does not run automatically *(RustyFinder choice; safe)*.
- Context menu: **Edit**, Rename, Delete, **Hide from Sidebar**.

## 7. Compare (two files)

- Command "Compare…" (Commands menu or toolbar). It needs exactly two files: one selected in each pane, or two selected in the same pane.
- RustyFinder launches the external compare tool chosen in Settings: FileMerge (needs Xcode), Kaleidoscope (needs its command-line tool), Beyond Compare (needs its command-line tool), or Araxis Merge. If the tool isn't installed, an alert names what to install.
- Remote files are downloaded to temporary copies first. The activity shows what is being fetched.
- Folders can't be compared with this command. Folder comparison is the Sync window.

## 8. Acceptance tests

- **SY-01** Given left/a.txt (10 B, mtime T) and right/a.txt (10 B, mtime T+1 s), then the plan is empty.
- **SY-02** Given left/a.txt (10 B, T) and right/a.txt (12 B, T) with direction left→right, then the plan has one Update row for a.txt.
- **SY-03** Given right has orphan z.txt, direction left→right, and "Delete orphaned items" off, then no Delete row. When it is turned on (after confirming), then a Delete row for z.txt appears.
- **SY-04** Given both-ways, left/n.txt newer than right/n.txt, and l.txt only on the left and r.txt only on the right, then the plan is: Update n.txt left→right, Add l.txt to the right, Add r.txt to the left, and no deletions.
- **SY-05** Given right's clock is 60 minutes ahead (all right mtimes +3600 s) and the offset is set to "right is 60 minute(s) ahead", then identical files produce an empty plan.
- **SY-06** Given the exclude rule Extension is `log`, then no `.log` file appears in any category.
- **SY-07** Given a completed sync, when the window re-analyzes, then the plan is empty.
- **SY-08** Given the plan has 5 rows and the user unchecks all of them, then Synchronize is disabled.
- **SY-09** Given the footer "Added" is clicked, then only Add rows are listed. Clicking "All" restores the full list.
- **SY-10** Given "Don't add empty folders" and a source folder whose only file is excluded, then that folder is not created on the target.
- **SY-11** Given a synclet is saved and the app relaunched, when it is clicked in the sidebar, then the Sync window opens with the same paths, direction, options and filters.
- **SY-12** Given one pane shows search results, then "Sync to…" is disabled.
- **SY-13** Given two files selected in the same pane and Compare with FileMerge not installed, then an alert explains that Xcode is required.
