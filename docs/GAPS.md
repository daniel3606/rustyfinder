# Spec gaps

Everything in `docs/spec/` that is missing or only partly built, checked
against the code on 2026-11-16. Tick an item when it is built **and**
covered by a test or scripted check. Size: S = under an hour, M = a few
hours, L = a day or more. **ext:** = needs something outside the code.

## Scope rule (2026-11-16)

RustyFinder is local-first: no accounts, API keys, OAuth, telemetry or
hosted services, and nothing that needs a paid Apple Developer membership
to work. It still browses anything macOS itself mounts or syncs (external
and network volumes, folders kept by other installed cloud apps), and the
existing server protocols (SFTP, FTP, WebDAV, S3-compatible) stay, since
they work with servers the user runs or chooses and need no account with
us. Items removed under this rule are listed at the end with the reason.

## Done since the audit
- [x] views §3.1: every List column (Date Created/Added/Last Opened, Owner, Group, Permissions, Tags, Comments, Spotlight metadata columns), header right-click to toggle, reorder with Name kept first, double-click divider to fit, saved across launches (VW-09)
- [x] views §3.3: sort keys ⌃⌘3 Date Added, ⌃⌘7 Tags, ⌃⌘8 Owner, ⌃⌘9 Group, ⌃⌘0 Permissions
- [x] views §3.4: Group By (⌃⌥⌘0–6) with Kind, date, size and tag groups, headers in List view (VW-04, VW-05); toolbar button, Option-click for Sort By
- [x] views §2.1: Select All Files, Select All Folders; Home/End in List view
- [x] views §2.5 / VW-10: Calculate Folder Size (⌘⇧S), Calculate All Folder Sizes; shown in Size column, status line and Info
- [x] views §3.3: Always Sort Folders Alphabetically
- [x] views §6 / VW-07 / VW-08: per-folder view settings (layout, sort, grouping) remembered as you change them, View Options panel (⌘J) with This Folder / This Folder and Subfolders, Use as Defaults, Save View Settings as Defaults, Restore Default View Settings (⌥⌘R), remote folders keyed by server; Column view keeps its layout while browsing inside it
- [x] views §3.1/§3.2/§5: bold folder names, font family (Option-click for system font), icon previews in List view, always resize name column, resize columns to fit, icon size slider 16–512, grid spacing, Show item info (size, item count, pixel size), icon previews on/off, tag dots in Icon view
- [x] views §2.2: hidden extensions honoured (and kept on rename), lock badge, alias arrow, custom folder icons and colours (the system's icon for the item)
- [x] views §2.4: slow second click on a selected name renames it
- [x] views §3.4: groups in Icon view, one section per group with headers
- [x] views §3.2: expandable folders in List view: disclosure triangles, ⌘→/⌘← with ⌥ for everything inside (capped at 8 levels / 5000 items), indented rows that select, copy, rename and delete like any other, status line counts visible rows, re-read after operations; not offered in grouped lists or search results
- [x] views §5: spring-loaded folders in List and Icon view (follows the system's spring-loading switch and delay)
- [x] views §1/§4 / VW-06 / PT-07: Column view: parent columns, focused column (the list itself, so every command works), folder or file preview column, ←/→, re-rooting, widths per column with ⌥ for all, double-click to fit, saved widths, side-column menu without navigating, drag out, text size

## panes-tabs.md
- [x] §2 View-menu toggles for Title Bar and Path Bar — S
- [x] §3 Tab icons for search / tag tabs; favorite icons on tabs (FV-08) — S
- [x] §3.1 "Open in New Window" in the item context menu — S
- [x] §3.2 Drag tabs to reorder and to the other pane — M
- [x] §3.2 Spring-load a tab when files hover over it; drop files on a tab — M
- [x] §3.2 / PT-08 Folders dropped on "+" open one tab each — S
- [x] §4 / PT-15 Unplugged drive falls back to the starting directory — S
- [x] §4/§5 Recent Folders per data source, including servers — M
- [x] §5 Shares sidebar group (mounted shares, Bonjour servers, Eject) — M (share picker and Eject tested; a real mount and Bonjour need a server, see the end)
- [x] §5 Sidebar Editor, "Hide from Sidebar", group "hide other groups" — M
- [x] §5 Drag to reorder devices and favorites in the sidebar — M
- [x] §5 / PT-18 Drop on sidebar folders, favorites, apps, tags — M
- [x] §5 / PT-13 Multi-volume eject prompt; Option/Control eject modifiers — S
- [x] §6 Drop items on path-bar segments — M
- [x] §6 Path-bar volumes menu lists open remote locations — S
- [x] §7 Restore full-screen state — S
- [x] §7 Remote tabs reconnect lazily on launch, error with Retry — M
- [x] §8 / PT-16 Open from outside: Reveal service, Dock drop, `rustyfinder://` URL scheme, reuse of an open tab — M (default file viewer: see the end)
- [x] §9 Full Disk Access first-run prompt and privacy-failure prompt, "Continue without" remembered — S/M (granting itself is TCC's)

## views.md
- [x] §3.2 Remote-thumbnail option for icon previews (needs a download per file; local previews done) — done: View Options › Also for files on servers; only pictures, PDFs and movies up to 2 MB, fetched through the preview cache; off by default

## file-ops-transfer-queue.md
- [x] §1 Make Alias (⌃⌘A), Make Alias/Symlink to other pane — M
- [x] §1 Edit (⌘E) with the configured editor — M
- [x] §1 Print (⌘P) — S
- [x] §1 Share… — S
- [x] §1 Open With for remote files (download first) — S
- [x] §1 Copy Path for remote items: URL without password, server path — S
- [x] §1 Toolbar Copy to / Move to / Delete — S
- [x] §1 Retry with administrator rights, through the system's own authorization prompt (no helper) — M
- [x] §3 Drag remote items out (download on drop, path text to Terminal) — M
- [x] §5.2 / FO-07 Drag to reorder queued activities — S
- [x] §5.5 / FO-15 Auto-retry with max retries and back-off — M
- [x] §5.5 / FO-14 Bandwidth limits — M
- [x] §5.5 "Retry after N seconds" replies — S
- [x] §5.6 Notification when a long activity finishes in the background — S (ext: notification permission)
- [x] §8 Undo/Redo sound — S
- [x] §7 Remote editing with the per-extension editor; upload-failed alert with Retry — S/M (no editor setting exists yet: the default app, or the app chosen in Open With)
- [x] §8 Undo for tag and comment changes — M

## conflicts.md
- [x] §1 Folder Replace confirms when the target has extra items — S
- [x] §2 Resume policy setting — S
- [x] §3 NFC-normalise remote names before comparing — S
- [x] §6 / CF-11 / SF-08 / FT-06 Partial files and Resume (SFTP offset, FTP REST/APPE, S3 Range) — L

## preview-quicklook.md
- [x] §1.1 / PV-04 Preview and Info for remote items (cache, 10 MB threshold, ⌘R) — M
- [x] §1.1 Remember PDF display options — S
- [x] §1.2 Alias/symlink target details; change alias target — S
- [x] §1.2 Show ACL presence — S
- [x] §1.2 / SF-13 Get Info for remote items; remote permission editing — M
- [x] §1.2 / PV-09 Multi-item Tags/Comments editing, folder sizes, Created/Added ranges — M
- [x] §1.3 Get Info windows follow outside changes — S
- [x] §2 / PV-12 Quick Look for remote and archive items — M
- [x] §2 Slideshow advances by itself — S

## keyboard-shortcuts.md
- [x] §1 Menu commands with default keys: Help ⌘?, Print ⌘P, Eject ⌘⇧E, Edit ⌘E, Make Alias ⌃⌘A, Calculate Folder Size ⌘⇧S, Show View Options ⌘J, Restore Default View Settings ⌥⌘R, Show Favorites ⌥⌘F; Expand/Collapse ⌘→/⌘← are handled by the list itself (as menu keys they would steal ⌘←/⌘→ from text fields)
- [x] §2 / KS-06 Tools in Settings → Shortcuts with recorder and conflict check — S/M

## settings-defaults.md
- [x] §1 "Detect application when deleting" (App Deleter) — S
- [x] §1 / SD-03 "Open remote connections in", "Handle archives with RustyFinder", FDA reminder, favorite as starting directory — S each
- [x] §2 Transfer settings: auto-retry, max retries, speed limits, resume policy — S
- [x] §3 Editing section (editors, extensions, default) — M
- [x] §4 "Inherit $PATH from login shell" — S
- [x] §5 Tags section — M
- [x] §7 / SD-04 / SD-05 Themes and theme editor — L
- [x] §8 First-run welcome flow (theme, Full Disk Access); Help menu entry — M (no iCloud step: out of scope)

## edge-cases.md
- [x] §1 / SF-09 / EC-10 Non-UTF-8 names addressed by raw bytes — L (SFTP, via vendored russh-sftp; APFS cannot hold such names, so tested at the protocol layer)
- [x] §2 / EC-06 Hide AppleDouble `._` companions — M
- [x] §2 Busy-volume eject names the app; panes move off first — S/M
- [x] §3 Lazy symlink resolution with 2 s timeout — M
- [x] §3 Resolve Finder alias files — S
- [x] §4 "Authorize" for unreadable folders, through the system authorization prompt — M
- [x] §4 Offer to unlock locked items — S/M
- [x] §4 Link to System Settings for privacy-protected folders — S
- [x] §4 iCloud busy-item alert — S (logic and alert checked with a simulated busy item; a real iCloud account is ext)
- [x] §5 Reconnect stale remote sessions after sleep — M
- [x] §5 / EC-07 FTP data timeout and keep-alive; 15 s guard on network mounts — M

## remote-ftp.md
- [x] §1 Implicit FTPS (port 990) — S
- [x] §2 / FT-03 "Always trust" certificates (keychain) — M (pinned in a local trust file, not the keychain: see DECISIONS)
- [x] §2 / FT-02 Exercise the TLS path against a local fixture — S
- [x] §2 EPLF listing parser — S
- [x] §2 Data timeout, NOOP keep-alive, silent reconnect — M
- [x] §3 SITE CHMOD, SITE UTIME — S
- [x] §3 / FT-06 REST resume (with conflicts §6)
- [x] FT-08 Editable FTP favorites (with favorites) — done: the Favorite Manager edits a copy of the stored favorite, so a rename keeps passive mode, TLS and encoding (core test, through a relaunch)

## remote-webdav.md
- [x] §1 Warn before Basic auth over plain HTTP — S
- [x] §1 Permanent certificate trust (with FTP) — M
- [x] §3 PROPPATCH modification time — S
- [x] §3 Range resume (MAY) — S
- [x] Exercise HTTPS locally — S

## remote-s3.md
- [x] §1.1 Region pop-up — S
- [x] §1.1 Virtual-hosted vs path style, Advanced override — S
- [x] §1.2 / S3-08 Retry with `x-amz-bucket-region` — S
- [x] §1.3 Storage Class — S/M (upload option; the `item_details` FFI gives the Info line once Info shows remote items)
- [x] §1.3 Copy URL — S

## remote-smb-nfs.md
- [x] §1 / SM-02 Own share list hiding `$` shares — M
- [x] §2 Shares sidebar group with free space and Eject — S/M
- [x] §2 / SM-04 Server-gone error; 15 s timeouts on mounts — done: list/stat on SMB/NFS/AFP/WebDAV mounts give up after 15 s off the main thread and show the load error (core tests); a server really vanishing needs a real share
- [x] §2 / SM-05 Eject moves panes off first — S
- [x] §3 Share favorites — S/M

## workspaces-favorites.md
- [x] §1 Favorite kinds: files, connections, remote paths, shares, synclets, presets — L
- [x] §2 / FV-01 Add to Favorites sheet (name, group, icon); New Favorite… — M
- [x] §2 / FV-08 Favorite icons on tabs and in Quick Open — S
- [x] §2 Edit favorite icon, group, connection — M
- [x] §3 / FV-03 / FV-04 Favorite Manager (⌥⌘F) — L
- [x] §4 Workspaces: full screen, group, context menu, Quick Open, lazy reconnect — M

## tags.md
- [x] §1 / TG-05 Settings → Tags (colours, sidebar toggle, rename, delete, reorder) — M
- [x] §1 Read Finder's own tag list — S
- [x] §2 / TG-04 Drop on a sidebar tag — S
- [x] TG-06 Colour-tag shortcut test — S

## cloud.md
- [x] §2 / CL-05..07 iCloud Drive badges, Download Now, Remove Download (local FileManager APIs; RustyFinder never signs in) — M

## app-deleter.md
- [x] AD-01..05 App Deleter — L

## Out of scope (local-first rule)

- cloud.md §1.1–1.5 / CL-01..04: Google Drive, Dropbox, OneDrive APIs. Need OAuth client IDs registered with each provider. Their folders, when synced by the provider's own app, are browsed like any other folder.
- cloud.md §1.4 / CL-08: Copy Dropbox Link. Needs the Dropbox API.
- workspaces-favorites.md §5 / FV-07: iCloud sync of favorites. Needs CloudKit, an Apple Developer membership and entitlements. Favorites stay in a local JSON file.
- settings-defaults.md §1: "Synchronize favorites via iCloud" setting (goes with FV-07).
- remote-s3.md §1.1 / S3-09: MFA via STS. Can only be verified with an AWS account holding an MFA device; S3-compatible access with keys stays.
- remote-s3.md §2 / S3-10: Backblaze B2 native API. B2 is reachable through its S3-compatible endpoint, which works today.
- remote-s3.md §3: Rackspace Cloud Files (Swift). Needs a Rackspace account; the service is legacy.
- file-ops / edge-cases "privileged helper": a launchd helper needs Developer ID signing (SMJobBless / SMAppService). Replaced by the system authorization prompt where an item above says so.

## Blocked by the environment (not by code)

- panes-tabs.md §6: truncated path-bar segments expanding while something is dragged over them. Left to NSPathCell's own hover expansion; scripted checks cannot drive a real drag, so it is only checked by hand.
- remote-s3.md: running against MinIO. The S3 code is tested against a local scripted S3 server; a MinIO run needs that service installed.
- remote-smb-nfs.md §1: Bonjour discovery is built (NetServiceBrowser for _smb._tcp and _afpovertcp._tcp, listed under Shares) but only finds something with a server on the network.
- remote-smb-nfs.md SM-01..06: mounting needs an SMB/NFS server. Built against the system mount API; verified only if one is reachable.
- remote-sftp.md SF-05 / SF-07: keyboard-interactive needs an sshd with PAM; verifiable only with such a server.
- tags.md TG-07: removing the last tag on iCloud Drive needs an iCloud account; tags go through the same Foundation call as everywhere else.
- panes-tabs.md §8: "Reveal in Finder" from other apps reaching RustyFinder needs it set as the default file viewer outside the app (RustyFinder declares itself a folder viewer; the service, Dock drops and links are tested).
- remote-smb-nfs.md Shares group with a real mount and Bonjour servers: needs an SMB/AFP server on the network; the scripted checks never browse or mount.
- archives.md §1 StuffIt (.sit/.sitx): nothing on macOS reads it (libarchive/bsdtar and Archive Utility do not), and no maintained open-source library does; only third-party apps (The Unarchiver's `unar`, StuffIt Expander) can. Opening one says "Unrecognized archive format".
