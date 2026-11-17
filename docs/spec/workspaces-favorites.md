# Favorites, favorite groups, workspaces, favorites sync (P2)

## 1. Favorite kinds

| Kind | Opens as |
|---|---|
| Local folder | The folder in the active pane |
| Local file | Opens the file with its default app *(and Option-click reveals it)* |
| Remote connection | Connects (credentials from the keychain) and shows its remote path |
| Favorite path (inside a remote favorite) | Connects and opens that sub-path |
| Network share | Mounts if needed, then shows it (remote-smb-nfs.md §3) |
| Synclet | Opens the Sync window (sync-compare.md §6) |
| Multi-Rename preset | Opens Multi-Rename (multi-rename.md §6) |
| Workspace | Restores the workspace (§4) |
| Tag | A tag listing (tags.md). Not editable as a favorite. |

## 2. Creating and editing favorites

- **Add to Favorites:** context menu on a folder or file, the toolbar, or the Favorites menu ("New Favorite…" for the current location). A sheet asks for a **name** (prefilled) and a **favorite group** (popup, default "Favorites"), and optionally an **icon**: the default icon, a set of built-in symbols (home, folder, server, cloud, phone, film, developer and so on), or a custom image file. The icon shows in the sidebar, in tabs opened from the favorite, and in Quick Open.
- **Remote favorites** come from the Connect panel ("Add to Favorites") or from the path bar ("Add Path to Favorite"). Add Path to Favorite stores a **favorite path** under the current remote favorite. Remote favorites show these paths as children ("favorites within favorites"). The path must be saved exactly as shown in the path bar.
- Remote items can't become favorites by dropping them on the sidebar.
- **Edit** (context menu, or in the manager): change the name, icon, group, and for remote favorites every connection field. File favorites are editable too.
- Duplicate names inside a group are rejected ("The name … is already taken").

## 3. Favorite manager (⌥⌘F)

- A window (or panel inside a pane) showing all groups and favorites as a tree, with a detail editor for the selection.
- Actions: New Favorite, New Favorite Group, Duplicate (the duplicate is selected after creation and is fully functional, not a dead link), Delete (button and the Delete key; confirmation "delete favorite …?"; deleting a group warns that its content is deleted too), and drag to reorder or move between groups. The bottom buttons have tooltips.
- Sidebar context menu items for favorites and groups include **Reveal in Favorite Manager**, which opens the manager if needed and selects the item.
- Favorite groups can be created, renamed, deleted (with content, after confirmation) and hidden in the sidebar.
- Favorites can be reordered by dragging in the sidebar, including remote favorites and several favorites that point to the same server (each keeps its own identity).

## 4. Workspaces

- **Save Workspace…** (Favorites or File menu) asks for a name and group and stores: the **window frame** and **full-screen state**, the layout (one or two panes, split orientation, divider position), sidebar and preview visibility, and for each pane **every tab** with its location, pinned state and **full view settings** (mode, sort, grouping, columns). It doesn't store the Connect panel or the favorite manager.
- **Update Workspace** saves the current window over the workspace it was opened from (enabled only when the window came from a workspace).
- **Opening a workspace** from the sidebar or Quick Open replaces the front window's layout and tabs *(RustyFinder choice)*. The context menu offers **Open in New Window** and **Reveal in Favorite Manager**. A workspace saved in full screen reopens in full screen.
- Remote tabs in a workspace connect lazily, when they are first shown.

## 5. Favorites sync via iCloud

- When "Synchronize favorites via iCloud" is on, all favorites, groups, favorite paths, synclets, rename presets, workspaces and custom icons sync through the user's iCloud account (CloudKit private database) to every Mac where the setting is on.
- Passwords are **not** in the synced favorites. They go through iCloud Keychain if the user enabled it. Otherwise the first connect on another Mac asks for the password.
- Conflicts: last writer wins per favorite *(RustyFinder choice)*. Deletions sync.
- Turning sync off keeps the local copy. Turning it on merges local and cloud favorites by identifier, never duplicating identical ones.
- Without an iCloud account, the toggle explains why it can't be enabled.

## 6. Acceptance tests

- **FV-01** Given the current folder /Projects, when New Favorite is saved with the name "P" in group "Work", then "P" appears under "Work" in the sidebar and clicking it opens /Projects.
- **FV-02** Given an SFTP favorite at /var/www, when "Add Path to Favorite" runs on /var/www/site/logs, then a child path "logs" appears under the favorite and opens that exact path.
- **FV-03** Given a favorite is duplicated in the manager, then the copy is selected and opens the same location.
- **FV-04** Given a group with 3 favorites is deleted, then a confirmation mentions its contents. Afterwards the group and the 3 favorites are gone.
- **FV-05** Given a window with left tabs [/a (pinned, List by Size), /b] and right [/c (Icon)], when it is saved as workspace W, the window changed, and W opened, then the tabs, pinned state and view settings are restored exactly.
- **FV-06** Given workspace W opened with "Open in New Window", then a new window shows W and the existing window is unchanged.
- **FV-07** Given iCloud favorites sync on two Macs, when a favorite is added on Mac A, then it appears on Mac B without restarting the app *(manual test)*.
- **FV-08** Given a favorite with a custom icon, when it is opened, then the tab shows that icon.
