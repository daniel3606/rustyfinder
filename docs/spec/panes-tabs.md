# Panes, tabs, sidebar and navigation (P0)

## 1. Shared vocabulary

- **Window:** one RustyFinder browser window. Several windows may be open at once; each is independent.
- **Pane:** a file browser area inside a window. A window shows one or two panes.
- **Active pane:** the pane that has keyboard focus. Exactly one pane per window is active. Commands that act on "the selection" use the active pane. Commands that need a destination ("Copy to", "Move to", "Sync to") use the **other pane's** current location as the target.
- **Location:** what a tab shows: a local folder, a folder inside an archive, a remote folder (SFTP, S3 and so on), a search result, or a tag listing.
- **Selection:** the items selected in the active pane. If nothing is selected, item commands are disabled. Folder-level commands (New Folder, Calculate All Folder Sizes) still work.
- **Activity:** a long-running operation (copy, delete, compress, sync…) tracked in the Activities view (see file-ops-transfer-queue.md).

## 2. Window layout

From left to right: **sidebar**, **pane area**, **preview pane**. Above the panes, from top to bottom: **toolbar**, **tab bar** (one per pane), **path bar with status line** (one per pane).

Every part except the pane area can be shown or hidden from the View menu. The toggle label flips between "Show X" and "Hide X":

| Element | Default | Toggle |
|---|---|---|
| Toolbar | shown | ⌥⌘T |
| Title bar | shown | menu only |
| Tab bar | shown when a pane has 2+ tabs; can be set to always show | ⌘⇧T |
| Path bar | shown | menu only |
| Sidebar | shown | ⌥⌘S |
| Preview pane | hidden *(RustyFinder choice)* | ⌘⇧P |
| Activities (preview pane switched to the Activities view) | — | ⌘⇧U |
| Invisible (hidden) files | hidden | ⇧⌘. |

Visibility choices are remembered per window and restored for new windows *(RustyFinder choice: the last-changed state becomes the default for new windows)*.

### 2.1 Pane arrangement

- **Two panes, vertical split** (side by side, left and right). This is the default layout.
- **Two panes, horizontal split** (stacked: top acts as "left", bottom acts as "right"). View menu: "Show Horizontal Panes" / "Show Vertical Panes".
- **Single pane mode** (⌥⌘P toggles). The hidden pane keeps its tabs and locations. Switching back to two panes shows the same content again.
- The split divider can be dragged. Its position is remembered per window.

### 2.2 Pane commands

- **Switch active pane:** clicking anywhere in a pane activates it. RustyFinder MUST also provide a keyboard command for this (*RustyFinder choice:* Tab when the file list has focus, as is conventional in dual-pane managers). When panes switch, assistive technology announces which pane is now active ("Left pane" / "Right pane").
- **Swap Panes:** exchanges all tabs (with their locations and histories) between the left and right panes. The active pane follows its content.
- **Clone Pane (⌘⇧C):** the other pane navigates to the active pane's current location. If the active location is inside a package or bundle, the clone shows the same browsing location. It does not open the bundle in an application.
- **Sync Browsing (⌘⇧Y, toggle):** while on, navigating into a subfolder in one pane makes the other pane enter the subfolder with the same relative name, if it exists. Going to the parent folder does the same. If the matching subfolder doesn't exist on the other side, the other pane stays where it is and sync browsing stays on. Turning it off stops the coupling. It is meant for two folder trees with the same structure, for example local and remote copies of a site.

## 3. Tabs

Each pane has its own ordered list of tabs. Exactly one tab per pane is selected.

**Tab contents:** location, back/forward history, view mode and per-tab view settings, selection, scroll position.

**Tab label and icon:** the label is the folder name, or a search description such as `Search: "term"`. The icon is chosen like this:
- the folder icon for local folders, never a generic document icon
- for remote locations, the favorite's custom icon if the connection was opened from a favorite with one, otherwise the protocol icon
- for a tag listing, the tag icon
- for a search, a search icon

### 3.1 Tab commands

| Command | Default key | Behavior |
|---|---|---|
| New Tab | ⌘T | New tab in the active pane, next to the current tab. It opens the current location (*RustyFinder choice*; the reference duplicates the current location). |
| Close Tab | ⌘W | Closes the current tab. If it is the pane's last tab, RustyFinder closes the window instead (*RustyFinder choice*: same as the "Close Window" command). |
| Close Window | ⌘⇧W | Closes the window. If activities are running, confirmation is required (see file-ops). |
| Close Other Tabs | ⌥⌘W | Closes every unpinned tab in the active pane except the current one. |
| Select tab 1–5 | ⌃1 … ⌃5 | Selects that tab of the active pane, if it exists. |
| Next / previous tab | ⌃Tab / ⌃⇧Tab | Cycles tabs in the active pane, wrapping around. |
| Move Tab to Other Pane | ⌥⌘⇧T | Moves the current tab to the end of the other pane's tab list and selects it there. |
| Pin / Unpin Tab | ⌥⌘⇧P | Toggles pinned state. |
| Open in New Tab | ⌥⌘↓ | Opens the selected folder in a new tab in the same pane. |
| Open in New Window | context menu | Opens the selected folder, or the clicked tab, in a new window. |

### 3.2 Tab interaction

- The close (X) button sits on the **left** of the tab and appears on hover, in place of the tab icon.
- Middle-clicking a tab closes it.
- Tabs can be reordered by dragging within a pane, and dragged to the other pane.
- Dragging file items over a tab and pausing makes that tab active (spring-loading), so the drop can land in its location.
- Dropping one or more folders on the tab bar's "+" button opens **each** folder in its own new tab.
- Dragging a tab onto the file list does nothing. In particular it never creates an item.

### 3.3 Pinned tabs

- A pinned tab can't be closed by the X button, middle-click, ⌘W or "Close Other Tabs" until it is unpinned.
- Its location stays fixed. Switching view modes (including in and out of Column view) never moves the pinned location to a subfolder.
- Navigating away from the pinned location while in a pinned tab opens the new location in a new tab and leaves the pinned tab where it is *(RustyFinder choice)*.
- Pinned tabs are restored with the window (see §7).

## 4. Navigation

| Action | Default key / gesture |
|---|---|
| Open selected folder (enter) or open file with its default app | double-click, ⌘↓ |
| Enclosing (parent) folder | ⌘↑ |
| Back / Forward | ⌘[ / ⌘], toolbar buttons, mouse buttons 4 / 5 |
| Open in other pane | ⌥⇧⌘↓; Option-click a sidebar item |
| Go to Folder | ⌘⇧G (see quick-open.md) |
| Connect to server | ⌘K (opens the Connect panel; see remote-*.md) |
| Home / Desktop / Documents / Downloads / Library / Applications / Utilities | ⌘⇧H / ⌘⇧D / ⌘⇧O / ⌥⌘L / ⌥⌘⇧L / ⌘⇧A / ⌥⌘U |
| Refresh | ⌘R |

Rules:
- Back/forward history is per tab. Navigating somewhere new after going back drops the forward history. Moving forward repeatedly after going back several steps MUST revisit each step in order.
- Double-clicking **empty space** in a pane does nothing. It never goes to the parent folder.
- Going to the enclosing folder selects the folder you just left.
- The **Go** menu also lists **Recent Folders** (see §5) and **Connections** (open remote sessions). Both are reachable even when the sidebar or those sidebar groups are hidden.
- **Starting directory:** new windows and tabs with no inherited location open at the configured starting directory (default: the user's home folder *(RustyFinder choice)*). After disconnecting from a remote location, a pane showing it returns to the starting directory.
- Local folders refresh automatically when their contents change on disk. Remote locations refresh on ⌘R, and also after RustyFinder's own operations on them.
- If an external drive is unplugged without being ejected, panes showing it go to the starting directory and the sidebar removes the device.

## 5. Sidebar

The sidebar is a vertical list of groups, each collapsible by clicking its header. Collapsed state persists across launches.

| Group | Contents |
|---|---|
| Devices | This Mac, mounted volumes and disk images, with free space shown under or next to each (updated right after operations). Order can be changed by dragging. |
| Connections | Open remote sessions. Context menu: Disconnect. |
| Shares | Mounted network shares and discovered servers (Bonjour). Context menu: Eject. |
| Favorites | User favorites, optionally in named groups (see workspaces-favorites.md). |
| Recent Folders | The last 10 distinct locations visited on the active pane's data source (local or the current remote server), most recent first. Deleted folders are removed from the list. Tags and searches are never added. |
| Tags | Tags chosen in Settings → Tags. Clicking one lists all items carrying it (see tags.md). |

Rules:
- A **Sidebar Editor** (show/hide toggle) lets the user hide or show any group and any individual device, share or favorite group. "Hide from Sidebar" is also in each item's context menu. The context menu of a group header can hide all remaining groups.
- Clicking a sidebar item opens it in the active pane. **Option-click** opens it in the other pane. If only one pane is visible, Option-click opens it in the active pane and MUST NOT hang or fail.
- **Dropping items** on a sidebar folder or folder favorite copies or moves them into it, using the same Option/⌘ modifier rules as dropping onto a pane (file-ops §3). If "Ask for confirmation for drag operations" is on, a confirmation names the count and destination first.
- Dropping items on an **application** in the sidebar opens them with that application (remote items are downloaded to a temporary place first).
- Dropping items on a **tag** assigns that tag.
- Dropping a remote item on the Favorites group never offers to make it a favorite. Remote favorites are created from the Connect panel or the path bar.
- **Eject:** if the volume belongs to a disk with several volumes, RustyFinder asks whether to eject just this one or all of them. Holding Option while ejecting ejects all without asking. Holding Control ejects only this one without asking. A single-volume disk is ejected without that question.

## 6. Path bar and status line

- The path bar shows the current location as clickable segments, root first. Clicking a segment navigates there. Dropping items on a segment copies or moves them into that folder.
- Long paths truncate middle segments. A truncated segment expands to its full name on hover or while something is dragged over it.
- The leftmost icon opens a menu of all mounted volumes, network shares and open remote locations. This works even when only one share is connected.
- Context menu on a segment: Copy Path, Open in New Tab, Add Path to Favorite.
- The **status line** shows `N items` (or `x of N selected` with the total size of the selection) and the free space on the current volume. It updates right after selection changes, folder expand/collapse, filtering and file operations.

## 7. Window and tab restoration

- Setting "Restore windows and tabs on startup" (default **on** *(RustyFinder choice)*). When on, every window comes back with its frame, full-screen state, layout, tabs (pinned included), locations and view modes. Remote tabs reconnect when they are first shown (credentials from the keychain). If a reconnect fails, the tab shows an error with a Retry action and is never silently replaced.
- Transient panels (the Connect panel and the favorite manager) are never restored as tabs.
- When it is off, the app starts with one window at the starting directory in both panes.
- The app MUST NOT reopen windows that were closed before quitting.

## 8. Opening locations from outside

- A system service "Reveal in RustyFinder" accepts files, folders, URLs and path strings and reveals them (opens the parent, selects the item).
- If RustyFinder is the default file viewer (configured outside the app), "Reveal in Finder" from other apps opens RustyFinder. If the requested location is **already open** in a tab of the frontmost window, that tab is activated instead of opening a new one.
- Dropping a folder on the Dock icon opens it the same way.
- RustyFinder SHOULD register a URL scheme for opening a saved favorite by identifier (used by favorites and workspaces).

## 9. Full Disk Access

On first launch, and whenever access to a protected location fails with a privacy error, RustyFinder explains that Full Disk Access is needed, offers to open System Settings → Privacy & Security, and offers "Continue without Full Disk Access". Choosing continue stores that choice so the general prompt doesn't come back. Later privacy failures still show an inline error on the location.

## 10. Acceptance tests

- **PT-01** Given a fresh install, when RustyFinder launches, then one window shows two side-by-side panes, both at the home folder, with the left pane active.
- **PT-02** Given two panes, when the user presses ⌥⌘P, then one pane is shown. When they press ⌥⌘P again, the second pane comes back with the same tabs and locations it had before.
- **PT-03** Given left = /A and right = /B, when Swap Panes runs, then left = /B and right = /A, and each pane's back history moves with it.
- **PT-04** Given the left pane is active at /A, when Clone Pane (⌘⇧C) runs, then the right pane shows /A.
- **PT-05** Given Sync Browsing is on, left = /x and right = /y, and both contain subfolder `s`, when the user opens `s` in the left pane, then the right pane shows /y/s. If /y has no `s`, the right pane stays at /y.
- **PT-06** Given a pinned tab, when the user presses ⌘W, middle-clicks it, or chooses Close Other Tabs from another tab, then the pinned tab remains.
- **PT-07** Given a pinned tab at /p, when the user switches it from Column view to List view and back, then its location is still /p.
- **PT-08** Given three folders are dragged onto the tab bar "+" button, then three new tabs open, one per folder.
- **PT-09** Given history /a → /b → /c, when the user goes Back twice and then Forward twice, then the pane is at /c.
- **PT-10** Given a folder view, when the user double-clicks empty space, then the location does not change.
- **PT-11** Given /a/b is shown, when ⌘↑ is pressed, then /a is shown with `b` selected.
- **PT-12** Given the user visits 12 distinct folders, then Recent Folders lists the last 10, newest first. After one of them is deleted, it disappears from the list.
- **PT-13** Given a disk with two volumes, when the user ejects one from the sidebar, then they are asked "eject this one or all". With Option held, all are ejected without asking.
- **PT-14** Given "Restore windows and tabs on startup" is on and two windows with pinned tabs are open, when RustyFinder quits and relaunches, then both windows and their tabs (pinned state included) come back. The Connect panel does not come back.
- **PT-15** Given the left pane shows /Volumes/USB and the drive is unplugged without ejecting, then the pane moves to the starting directory without an error dialog.
- **PT-16** Given a folder is open in a tab of the front window, when another app asks RustyFinder to reveal that folder, then that tab is activated and no new tab is created.
- **PT-17** Given only one pane is visible, when the user Option-clicks a sidebar favorite, then it opens in the visible pane and the app stays responsive.
- **PT-18** Given ten files are dropped on a sidebar folder favorite with "Ask for confirmation for drag operations" on, then a confirmation naming 10 items and the destination appears before anything is transferred.
