# File operations, transfer queue, activities, logs, undo (P0)

Conflict resolution during these operations is specified in conflicts.md.

## 1. Operations

Every operation works on local and remote locations alike, unless a cell says otherwise. "Other pane" means the inactive pane's current location.

| Command | Default key | Behavior |
|---|---|---|
| Copy to | F5, toolbar | Copies the selection into the other pane's location. |
| Move to | F6, toolbar | Moves the selection into the other pane's location. |
| Copy to as… | menu | One selected item: asks for a new name (prefilled with the current name; Esc cancels), then copies it to the other pane under that name. |
| Copy / Paste | ⌘C, then ⌘V | ⌘C puts the selected items on the pasteboard (as file references, plus their paths as text). ⌘V in a pane copies them into that location. If ⌘V is chosen from a **folder's** context menu, the items go into that folder. |
| Move Items Here | ⌥⌘V | Like Paste, but moves. |
| Duplicate | ⌘D | Copies each selected item next to itself as `<name> copy[.ext]`. If that name is taken: `<name> copy 2`, `copy 3`, … |
| New Folder | ⌘⇧N | Creates `untitled folder` (or `untitled folder 2`, …) in the current location and starts inline rename on it. |
| New Folder with Selection | ⌃⌘N | Creates `New Folder With Items` (numbered if taken) in the current location, moves the selection into it, then starts inline rename. Disabled when the selection spans several parent folders (search results). |
| New File | ⌥⌘⇧N | Creates an empty `untitled file.txt` (numbered if taken) and starts inline rename. Works on remote locations. |
| Make Alias / Make Symlink | ⌃⌘A / ⌃⌘⇧A | Creates `<name> alias` or `<name> symlink` next to each item. Symlinks use a relative target when both are on the same volume *(RustyFinder choice)*. Symlinks also work on SFTP. |
| Make Alias to / Make Symlink to | menu | Same as above, but created in the other pane's location. |
| Rename | Return | One item: inline rename. Several items: Multi-Rename. |
| Move to Trash | ⌘⌫, toolbar Delete | Moves the selection to the Trash of its volume. No confirmation. Undoable. |
| Delete (immediately) | ⌥⌘⌫; Option + toolbar Delete; context menu with Option held | Asks for confirmation every time (states the count and that it can't be undone), then deletes permanently. |
| Empty Trash | ⇧⌘⌫ | Asks for confirmation, then empties the user's Trash. |
| Open / Open With | ⌘O / submenu | Opens with the default or chosen application. "Open With" lists apps that can open the type, with icons. Remote files are downloaded to a temporary folder first. |
| Edit | ⌘E | Opens the file in the configured editor for its type. For remote files, see §7. |
| Get Info | ⌘I | Opens a standalone info window (same content as the Info section of preview-quicklook.md). |
| Show Original | menu | Reveals an alias's or symlink's target. Single selection only. |
| Show Package Contents | menu | Browses a bundle as a folder. Single selection only. |
| Print | ⌘P | Sends the selected documents to the system print flow. |
| Share… | menu | The system share sheet for the selected items. |
| Copy Path | context menu, path bar | A submenu of formats: POSIX path; Terminal-escaped path (spaces, quotes and full-width spaces escaped so a shell takes them as one argument); file URL; name only. Remote: the full URL (no password) and the server path. With several items, one path per line. |
| Reveal in Finder | menu | Asks Finder to reveal the item. |

**Remote rules:** Trash doesn't exist on remote volumes. There, "Move to Trash" is replaced by "Delete", which always asks for confirmation.

**Privileged operations:** if an operation fails with "permission denied" on a local item the user could manage as an administrator, RustyFinder offers to retry with administrator rights (a one-time authorization through a privileged helper). The user can Skip, Stop or Cancel. Declining is reported as a failure, not a success.

## 2. Preconditions checked before an operation starts

These checks happen during **preparation**, before any byte is written:

1. **Into itself:** copying or moving a folder into itself or one of its descendants is refused with a clear error.
2. **Item replaced by its own container:** refused ("An item can't be replaced by an item it contains").
3. **Duplicate source names:** if two or more source items with the same name (for example from search results across folders) would land in the same target folder, the **whole operation does not start**. RustyFinder lists the clashing names. This prevents one source from silently overwriting another.
4. **Read-only target:** a target volume mounted read-only is reported before starting.
5. **Free space:** if the total size of the source is known and is larger than the free space on a local target volume, RustyFinder reports "not enough free space" before starting. Remote targets are not pre-checked.
6. **Preparation progress:** while it enumerates large trees, the activity shows "Preparing…" and a growing count of found subitems. Preparation can be cancelled.

## 3. Drag and drop

| Source → target | Plain drag | With ⌥ | With ⌘ |
|---|---|---|---|
| Local → local, same volume | Move | Copy | Move |
| Local → local, different volume | Copy | Copy | Move |
| Anything involving a remote location | Copy | Copy | Move |

- The cursor badge shows the operation: a "+" for copy, the plain arrow for move.
- Dropping on a folder (in a list, on a sidebar favorite, on a path bar segment or on a tab) puts the items inside it. Dropping on empty space puts them in the pane's location.
- If "Ask for confirmation for drag operations" is on (default off *(RustyFinder choice)*), a dialog confirms count, operation and destination.
- Items dragged out of RustyFinder into other apps are file references. Remote items are downloaded to a temporary folder when the drop happens. Dragging a remote item into Terminal inserts its remote path as text.
- Items dragged into RustyFinder from other apps (Photos, mail clients, browsers) are accepted, including file promises.
- Dragging onto a folder without opening it updates that folder's modification date. This happens naturally when a child is added.

## 4. Copy semantics (what must be preserved)

For local→local copies, RustyFinder MUST keep:
- file contents, including sparse regions (holes stay holes where the file system supports it)
- modification date and creation date (**never** reset to "now", including on external drives and network shares)
- POSIX permissions (subject to the target file system), extended attributes, Finder tags, Finder comments (these must still show in Finder afterwards), the hidden flag, the locked flag *(RustyFinder choice: the target is not locked if that would block the rest of the operation)*, and resource forks
- symlinks are copied as symlinks, never followed. Aliases are copied as files.
- packages and bundles are copied as whole directories

For transfers to and from remote locations, the modification time is set on the target when the protocol allows it (SFTP setstat, FTP MFMT, S3 metadata *(RustyFinder choice)*). Permissions are carried over only on SFTP.

A **local move** on the same volume is an atomic rename and is **not queued** (it runs at once, even with other transfers waiting). A move across volumes or to or from a remote location is a copy followed by deleting each source item **after** it copied successfully. If copying an item fails, its source is kept.

Moving a folder that the user can't write to (but whose parent they can) MUST work as a rename on the same volume.

## 5. Activities and the transfer queue

### 5.1 Model
Every operation becomes an **activity** with a title (`Copying "report.pdf"`, `Copying 12 items`, `Deleting 3 items`, `Compressing 5 items`, `Synchronizing 40 items`, `Renaming 20 items`), a state, progress (items done/total and bytes done/total), current item, speed (bytes/sec) and an estimate of remaining time.

States: **Queued → Preparing → Running ⇄ Paused → Completed | Failed | Stopped**.

### 5.2 Concurrency
- Transfers that touch a remote location, and local copies, run through a **transfer queue**. At most *N* transfer activities run at once. *N* is set by "Max. simultaneous transfers" (default **3** *(RustyFinder choice)*, range 1–10). Extra activities wait as Queued, in FIFO order.
- When a running transfer finishes, the next queued one starts right away. Transfers started from search results follow the same queue rules.
- Queued activities can be **reordered** by dragging them in the Activities list.
- Deletes, renames, new folder and local same-volume moves don't wait in the queue.
- A queued duplicate shows as `Queued` and has a correct title.

### 5.3 Controls
Each activity has Pause / Resume and Stop. "Stop All" stops every activity in the window. A paused activity keeps its place and resumes from the current item. For remote downloads, it resumes from the byte offset where possible.

### 5.4 Where activities appear
- A toolbar **Activities** button shows a badge or progress ring while anything is running. Clicking it opens a pop-up listing running and queued activities. The pop-up shrinks as activities finish.
- The preview pane can switch to an **Activities** view (⌘⇧U) with the same list.
- Hovering over an activity's detail line shows its full text in a tooltip.
- During sync, the activity shows the number of items processed out of the total.

### 5.5 Retries, errors and bandwidth
- "Automatically retry failed transfers" (default **off** *(RustyFinder choice)*) and "Max. number of retries" (default **3**). A failed item is retried after a short back-off (2 s, 4 s, 8 s… capped at 30 s *(RustyFinder choice)*) before the error policy applies.
- **Error handling policy:** *Ask* (default) shows an error prompt with Retry, Skip, Skip All and Stop. *Skip* logs the error and continues. *Stop* ends the activity.
- **Bandwidth limit:** "Limit downloads" and "Limit uploads", each a checkbox plus a value in KB/s (off by default). The limit applies to the **total** across all running remote transfers of that direction, not to each one.
- Remote servers that answer "rate limited" or "retry after N seconds" (cloud APIs) are retried after the stated delay. This doesn't count against the retry limit.

### 5.6 Completion feedback
- When an activity finishes successfully, a short completion sound plays (setting, default on). It also plays for transfers to remote locations.
- No success sound plays if the activity failed or was stopped, skipped or cancelled through any prompt.
- If the app isn't frontmost when an activity of more than ~10 s *(RustyFinder choice)* finishes, a system notification is posted (title and result).

### 5.7 Quitting and closing
Quitting the app, or closing a window, while that scope has running or queued activities shows a warning ("activities will be aborted") with Cancel and Quit/Close. Choosing Quit/Close stops them, and partial files are left as described in conflicts.md §6.

## 6. Log

The preview pane can switch to a **Log** view (toggle "Show Logs"). Each finished operation adds one entry:
- date and time, operation, outcome (success / failed / partially failed / stopped), and counts (for example "Renamed 20 items", "Synchronized 37 items", "Deleted 3 items")
- for a rename: old name → new name
- for failures: the failed paths and error messages
- the entry is truthful: a failed remote operation, a failed folder or bucket creation, or a failed rename is **never** logged as success
- the text can be selected and copied (for example a failed path). Hovering shows a tooltip with the full entry.
- undo and redo actions are logged as such, with a description that names what was undone (for example "Undo Move to Trash of 3 items")
- for sync, the logged count is the number of items actually synchronized, not the number analyzed

The log is kept for the session at least *(RustyFinder choice: keep the last 1000 entries across launches)*.

## 7. Remote editing

"Edit" (⌘E) or "Open" on a remote file:
1. downloads it to a per-session temporary folder (a cloud icon or progress shows for large files)
2. opens it in the editor set for that extension (Settings → Editing). If none is set, the default app opens it.
3. watches the temporary file. Each time it is saved, it is uploaded back to the same remote path (an activity "Uploading …" appears). If the upload fails, a "Remote Editing Error" alert offers Retry.
4. stops watching when the connection closes or the app quits. Temporary files are deleted at quit.

The editing settings let users add editors (any app) and map them to extensions. Removing an editor from the list removes it permanently.

## 8. Undo / Redo

⌘Z / ⌘⇧Z undo and redo the most recent **file** operation of the window when no text field has focus. The Edit menu shows "Undo <action>" or "Redo <action>".

| Operation | Undo effect |
|---|---|
| Rename / multi-rename | Restores the old names (multi-rename is one undo step) |
| Move (local or remote) | Moves the items back |
| Copy / duplicate / new folder / new file / alias / symlink | Moves the created items to the Trash (local) or deletes them (remote, after confirming) |
| Move to Trash | Puts the items back where they were |
| Tags / comments change | Restores the previous tags or comment |
| Delete immediately, Empty Trash, Sync | Can't be undone. The menu item is disabled after these, or skips past them *(RustyFinder choice)* |

Undo and redo work on remote locations, refresh the affected panes and play the standard sound.

## 9. Acceptance tests

- **FO-01** Given left = /src with file a.txt selected and right = /dst, when F5 is pressed, then /dst/a.txt exists with the same bytes and the same mtime and ctime as the source.
- **FO-02** Given a copy to an external drive, then the target's modification date equals the source's, not the current time.
- **FO-03** Given a sparse 1 GB file with 1 MB of data, when copied locally on APFS, then the target uses about 1 MB of disk.
- **FO-04** Given folder /a selected, when Copy to targets /a/sub, then nothing is copied and an "into itself" error is shown.
- **FO-05** Given search results with /x/r.txt and /y/r.txt both selected, when Copy to /dst runs, then nothing is copied and RustyFinder reports the clash on `r.txt`.
- **FO-06** Given "Max. simultaneous transfers" = 2 and 4 uploads started, then 2 are Running and 2 Queued. When one finishes, exactly one queued upload starts.
- **FO-07** Given 3 queued uploads, when the user drags the third above the first queued one, then it starts before it.
- **FO-08** Given a local move on the same volume while 3 uploads are running and the limit is 2, then the move finishes at once without being queued.
- **FO-09** Given ⌥⌘⌫ on 3 files, then a confirmation appears. On confirm the files are gone and not in the Trash, and Undo is not offered for this action.
- **FO-10** Given ⌘⌫ on file f, then f is in the Trash without a prompt. After ⌘Z, f is back in its original folder.
- **FO-11** Given a remote SFTP location, then the menu shows "Delete" instead of "Move to Trash", and ⌘⌫ asks for confirmation.
- **FO-12** Given ⌘D on `photo.jpg`, then `photo copy.jpg` is created. A second ⌘D on `photo.jpg` gives `photo copy 2.jpg`.
- **FO-13** Given ⌘⇧N twice in an empty folder, then `untitled folder` and `untitled folder 2` exist, and the second is in inline rename.
- **FO-14** Given "Limit uploads" = 100 KB/s and two uploads running, then their combined rate averaged over 10 s is ≤ 110 KB/s.
- **FO-15** Given "Automatically retry" on with 3 retries and a server that fails twice then succeeds, then the item transfers without a prompt and the log says success.
- **FO-16** Given an upload fails and the user clicks Skip in the error prompt, then no success sound plays and the log entry says failed/skipped.
- **FO-17** Given an SFTP upload running, when the user quits, then a warning appears. Cancel keeps the app running and the upload continues.
- **FO-18** Given a remote file edited with ⌘E, when the user saves in the editor, then within a few seconds the remote file's contents match.
- **FO-19** Given a folder renamed from `a` to `b`, then the log shows `a → b`. After ⌘Z, the folder is `a` again and the log has an undo entry.
- **FO-20** Given a local file dragged onto the other pane on the same volume without modifiers, then it is moved. With ⌥ held, it is copied.
- **FO-21** Given a remote file dragged to a local pane without modifiers, then it is copied (the remote file still exists).
- **FO-22** Given Copy Path → Terminal format on `/tmp/my file's.txt`, then the pasteboard holds a string that `ls` takes as exactly one existing path.
- **FO-23** Given Paste chosen from folder F's context menu, then the items are pasted into F, not into the current location.
