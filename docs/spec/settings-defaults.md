# Settings and defaults (P0)

Settings open with ⌘, as a window with sections. Changes apply right away (no Save button) and persist across launches. Every setting has a "Restore Defaults" path (per section).

Legend: *(ref)* = matches the reference product's default. *(RustyFinder choice)* = could not be confirmed, reasonable pick.

## 1. General

| Setting | Values | Default |
|---|---|---|
| Starting directory | Home, or any folder or favorite | Home *(RustyFinder choice)* |
| Restore windows and tabs on startup | on/off | on *(RustyFinder choice)* |
| Open remote connections in | Active pane / Left pane / Right pane | Active pane *(RustyFinder choice)* |
| Terminal application | Terminal, iTerm, Hyper, Kitty, Warp, Ghostty, or any app | Terminal *(ref)* |
| Show hidden files | on/off (same as ⇧⌘.) | off *(ref)* |
| Treat packages as folders | on: double-click enters bundles; off: opens them | off *(ref)* |
| Treat archives as folders | on: double-click browses an archive like a folder; off: opens it with the default app | on *(RustyFinder choice)* |
| Handle archives with RustyFinder | on: extracting and compressing use RustyFinder's own engine, with progress as activities | on *(RustyFinder choice)* |
| Ask for confirmation for drag operations | on/off | off *(RustyFinder choice)* |
| Detect application when deleting | on: moving an .app to the Trash opens App Deleter (app-deleter.md) | off *(RustyFinder choice)* |
| Play sounds (completion, undo) | on/off | on *(ref)* |
| Use relative dates | on/off | on *(RustyFinder choice)* |
| Always show tab bar | on/off | off *(RustyFinder choice)* |
| Synchronize favorites via iCloud | on/off | off (asked in the welcome flow) |
| Full Disk Access reminder | "Continue without Full Disk Access" suppresses it | prompt shown |

When the user picks "Open remote connections in: Left/Right", the Connect panel and newly opened remote favorites always go to that side, even in single-pane layout (where they replace the visible pane). Switching to two panes later keeps them on the configured side.

## 2. Transfers

| Setting | Values | Default |
|---|---|---|
| Max. simultaneous transfers | 1–10 | 3 *(RustyFinder choice)* |
| Automatically retry failed transfers | on/off | off *(RustyFinder choice)* |
| Max. number of retries | 1–10 | 3 *(RustyFinder choice)* |
| Limit downloads | on/off + KB/s | off |
| Limit uploads | on/off + KB/s | off |
| Replace policy | Ask / Replace / Replace Older / Keep Both / Skip | Ask |
| Merge policy | Ask / Merge / Replace / Keep Both / Skip | Ask |
| Resume policy | Ask / Resume / Replace / Skip | Ask |
| Error handling | Ask / Skip / Stop | Ask |

## 3. Editing

- A list of editor applications. Add (+) with an app picker, remove (−). Removal is permanent.
- Each editor has a list of extensions it is used for (comma-separated, without dots). One editor can be marked the **default** for extensions that aren't listed. If none is marked, the system default app is used.
- The settings are used by "Edit" (⌘E) and remote editing (file-ops §7).

## 4. Tools
See tools.md. A list of custom tools with add, remove, duplicate, edit, multi-select and drag reordering. "Inherit $PATH from the login shell" is a per-tool or global option *(RustyFinder choice: global, default on)*.

## 5. Tags
See tags.md. The list of tags (name and color), whether each shows in the sidebar, rename and delete via the context menu, and drag reordering.

## 6. Shortcuts
See keyboard-shortcuts.md §2.

## 7. Themes

- Built-in themes: **Default** (follows macOS Light/Dark), **Light**, **Dark**, plus a few predefined color themes (RustyFinder picks its own names and palettes; it must not copy the reference product's palettes).
- Users can **duplicate** any theme and edit it in a theme editor. Editable values: base color, background, alternate row background, text color, secondary text color, highlight (selection) color, inactive-selection color, sidebar background, sidebar text, sidebar selection, and a "dark style" flag (whether system controls render dark). Custom themes can be renamed and deleted. Deleting asks for confirmation.
- Selecting a theme applies it right away to every open window and tab, inactive tabs included (no leftover white backgrounds after a Dark switch).
- When the theme is Default and macOS switches appearance, all windows follow right away.
- The sidebar selection style should follow the current macOS style in the Light, Dark and Default themes.
- Themes affect selected items in the **inactive** pane too, which use the theme's inactive-selection color.

## 8. First-run welcome flow

Shown once, on first launch:
1. Choose a theme (Default preselected).
2. Enable iCloud sync of favorites (off).
3. Import favorites from an older installation if one is detected *(RustyFinder: import from a Finder sidebar is a nice-to-have; there is no reference-format import)*.
4. Button "Start using RustyFinder".
It then requests the notification permission and Full Disk Access (panes-tabs §9). The flow can be reopened from the Help menu *(RustyFinder choice)*.

## 9. View defaults
View defaults (mode, columns, sort, grouping, sizes, fonts) are set through View Options → "Use as Defaults" (views.md §6), not in Settings.

## 10. Acceptance tests

- **SD-01** Given a fresh install, then every value in §1 and §2 equals the listed default.
- **SD-02** Given Max. simultaneous transfers is changed from 3 to 1 while 3 transfers are queued, then from the next transfer start on only 1 runs at a time.
- **SD-03** Given "Open remote connections in" = Right and single-pane layout, when ⌘K connects to a server, then after switching to two panes the remote location is in the right pane.
- **SD-04** Given theme Default and macOS switches to Dark, then every window, inactive tabs and the Column view included, shows the dark palette.
- **SD-05** Given a custom theme is deleted, then a confirmation is shown first. Afterwards windows that used it fall back to Default.
- **SD-06** Given "Treat archives as folders" is off, when a .zip is double-clicked, then it opens in the default app and is not browsed.
- **SD-07** Given the Terminal application is set to Ghostty, when ⌘⇧R runs, then Ghostty opens at the current local folder.
