# Name conflicts (P0)

A **conflict** happens when an operation would create an item at a path where an item already exists. It applies to copy, move, paste, drag and drop, extract, download, upload, sync (when it has to ask), duplicate (never asks; it auto-numbers) and rename.

## 1. Conflict prompt

When a conflict needs a decision, the activity pauses and shows a prompt that names the item and the target folder. It shows both items side by side: icon or thumbnail, size, and modification date. The newer of the two is visibly marked.

Buttons for a **file** vs file conflict:

| Choice | Effect |
|---|---|
| **Replace** | The target is replaced by the source. |
| **Replace Older** | Replace only if the source's modification date is **newer** than the target's. Otherwise skip. If either item has **no** modification date (some servers), the item is skipped. |
| **Keep Both** | The source is written under a new free name: `<name> 2[.ext]`, then `3`, … The target is not touched. |
| **Skip** | Leave the target and don't transfer this item. |
| **Stop** | Abort the whole activity. Items already done stay done. |

For a **folder** vs folder conflict, the same choices apply plus:

| Choice | Effect |
|---|---|
| **Merge** | Combine the contents. Children that don't exist in the target are added. Children that clash raise their own conflicts, recursively. |
| **Replace** (folder) | The target folder is removed **completely** and the source folder copied in its place. RustyFinder confirms this when the target has items that aren't in the source *(RustyFinder choice)*. |

**File vs folder, or folder vs file:** Merge is not offered. Replace, Keep Both, Skip and Stop are.

**"Apply to all"** checkbox: when it is checked, the choice is reused for every later conflict of the **same kind** (file/file or folder/folder) in this activity without asking.

Esc and the window close button mean **Stop**.

## 2. Default policies (Settings → Transfers)

Each policy decides whether RustyFinder asks or acts on its own:

| Policy | Options | Default |
|---|---|---|
| Replace policy (file conflicts) | Ask, Replace, Replace Older, Keep Both, Skip | **Ask** |
| Merge policy (folder conflicts) | Ask, Merge, Replace, Keep Both, Skip | **Ask** |
| Resume policy (partial target exists, see §6) | Ask, Resume, Replace, Skip | **Ask** |

When a policy isn't Ask, conflicts of that kind resolve silently, and each one is recorded in the log entry for the operation.

## 3. Name matching rules

- On **case-insensitive** targets (the default on macOS volumes, and most SMB shares), `Report.txt` and `report.txt` conflict.
- On **case-sensitive** targets (most SFTP servers, S3, case-sensitive APFS), they don't conflict and both can exist.
- Unicode normalization: `é` written precomposed (NFC) and decomposed (NFD) count as the **same** name on local volumes, so they conflict. On remote targets, names are compared after normalizing both to NFC *(RustyFinder choice)*.
- When a file **replaces** a target on a case-insensitive volume, the result **takes the source's capitalization**. Replacing `report.txt` with `Report.txt` leaves `Report.txt`.

## 4. Replace safety

- A replace never deletes the target before the new content is fully written. For local targets, write to a temporary sibling name, then swap it in atomically. For remote targets, upload to a temporary name and rename over the target when the protocol supports it (SFTP: rename with overwrite, or delete then rename; S3: plain PUT is atomic).
- If the transfer fails or is stopped, the original target stays intact.
- Replacing an item with an item that **contains** it (for example replacing `/a` with `/a/x/a`) is refused (file-ops §2).

## 5. Conflicts within the source set

If two source items have the same name and the same target folder (possible from search results or multi-location selections), the operation doesn't start at all (file-ops §2.3). This is checked before any prompt.

Multi-Rename: if the new names would clash with each other, or with existing items that aren't being renamed, the window flags the clashing rows and the Rename button is disabled. A clash that only appears at run time (for example an item created in the meantime) raises the normal conflict prompt, **never** a silent overwrite.

## 6. Partial files and resume

A **partial target** is a file at the target path whose size is smaller than the source and that RustyFinder left behind from an earlier stopped or failed transfer of the same source (*RustyFinder choice:* treat any smaller target as partial only if the earlier activity recorded it as incomplete. Otherwise it is a normal conflict).

- With resume policy **Ask**, the conflict prompt adds **Resume**, which continues writing from the target's current size.
- Resume needs protocol support (SFTP: open for write at an offset; FTP: REST; WebDAV/S3: not supported, so Resume is not offered there).
- Resume also works with SFTP servers whose implementation doesn't advertise it by default, by writing at an explicit offset.
- After a resumed transfer, the target size must equal the source size. If it doesn't, the transfer is reported as failed.

## 7. Acceptance tests

- **CF-01** Given the target has `a.txt` (older) and the source has `a.txt` (newer), when the user picks Replace Older, then the target is replaced.
- **CF-02** Given the target `a.txt` is newer than the source, when the user picks Replace Older, then the target is unchanged and the log says skipped.
- **CF-03** Given a target item with no modification date (remote) and the user picks Replace Older, then the item is skipped.
- **CF-04** Given Keep Both on `a.txt` where `a 2.txt` already exists, then the source is written as `a 3.txt`.
- **CF-05** Given folder `d` exists on both sides with `d/x` (same) and `d/y` (only in source), when the user picks Merge, then `d/y` is added and a prompt is raised for `d/x`.
- **CF-06** Given 5 file conflicts and the user checks "Apply to all" with Skip on the first, then no more prompts appear and all 5 are skipped.
- **CF-07** Given the Replace policy set to Replace in Settings, when a copy hits a conflict, then no prompt appears and the target is replaced.
- **CF-08** Given a copy is stopped halfway through replacing `big.iso`, then the original `big.iso` at the target is still intact.
- **CF-09** Given a case-insensitive local target with `report.txt` and source `Report.txt`, when the user picks Replace, then the target folder holds exactly one file named `Report.txt`.
- **CF-10** Given an SFTP (case-sensitive) target with `report.txt`, when `Report.txt` is uploaded, then no conflict is raised and both files exist.
- **CF-11** Given an SFTP download was stopped at 40 %, when it is restarted and the user picks Resume, then only the remaining 60 % is transferred and the final file matches the source byte for byte.
- **CF-12** Given Esc is pressed in a conflict prompt, then the activity stops and items already transferred stay.
- **CF-13** Given a multi-rename that would produce two files named `x.jpg`, then the Rename button is disabled and both rows are flagged.
