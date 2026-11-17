# RustyFinder functional specs: index

These specs describe **what** RustyFinder must do, as behavior a user or tester can verify. They say nothing about how to build it. Every internal design choice (data model, storage, threading, crate layout) belongs to the implementation team.

## How to read a spec

- Each file covers one feature area: behavior, UI states, options and defaults, keyboard flows, rules and edge cases, then numbered **Given / When / Then** acceptance tests (IDs like `PT-07`).
- **Default:** values marked *(reference default)* match the reference product's default. Values marked *(RustyFinder choice)* are reasonable picks where the reference default could not be confirmed; the implementer may change them, but should keep them configurable.
- **MUST / SHOULD / MAY** follow the usual meaning. P0 items are MUST unless marked otherwise.
- Labels in quotes ("Keep Both") are short standard labels. Wording of longer UI text is up to RustyFinder.

## Evidence base and limits

Behavior was derived from the reference product's public manual, its public version history (4.0 to 4.7.7), the product feature page, the reference app's menu structure and default key bindings, its set of UI labels and option names, and protocol-level facts visible from the app bundle (for example, the options it passes to the system OpenSSH client).

**Limit:** interactive GUI observation of the running app was not possible in this pass (the analysis machine lacked Accessibility and Screen Recording permission). Fine visual layout and some exact defaults are therefore marked *(RustyFinder choice)*. Nothing in these specs depends on the reference implementation's internals.

## Coverage checklist and priority

| Status | Priority | Spec | Area |
|---|---|---|---|
| ✅ | P0 | [panes-tabs.md](panes-tabs.md) | Window, dual panes, tabs, sidebar, path bar, navigation |
| ✅ | P0 | [views.md](views.md) | List, Column, Icon views; sorting, grouping, view options |
| ✅ | P0 | [file-ops-transfer-queue.md](file-ops-transfer-queue.md) | Copy, move, delete, rename, new, duplicate; activities queue, logs, undo |
| ✅ | P0 | [conflicts.md](conflicts.md) | Name conflicts during copy/move/rename |
| ✅ | P0 | [preview-quicklook.md](preview-quicklook.md) | Preview pane, info, Quick Look, text edit |
| ✅ | P0 | [keyboard-shortcuts.md](keyboard-shortcuts.md) | Default key bindings, customization |
| ✅ | P0 | [settings-defaults.md](settings-defaults.md) | All settings and their defaults |
| ✅ | P0 | [edge-cases.md](edge-cases.md) | Cross-cutting edge cases (names, case, permissions, volumes) |
| ✅ | P1 | [quick-open.md](quick-open.md) | Command palette / Quick Open, Go to Folder, Quick Select |
| ✅ | P1 | [multi-rename.md](multi-rename.md) | Batch rename |
| ✅ | P1 | [sync-compare.md](sync-compare.md) | Folder sync, sync browsing, file compare |
| ✅ | P1 | [search.md](search.md) | Search and filter |
| ✅ | P1 | [tools.md](tools.md) | Custom command-line tools, Open in Terminal, checksum, git |
| ✅ | P1 | [archives.md](archives.md) | Browse, compress, extract |
| ✅ | P1 | [remote-sftp.md](remote-sftp.md) | SFTP |
| ✅ | P1 | [remote-ftp.md](remote-ftp.md) | FTP / FTPS |
| ✅ | P1 | [remote-webdav.md](remote-webdav.md) | WebDAV / WebDAV HTTPS |
| ✅ | P1 | [remote-s3.md](remote-s3.md) | Amazon S3 and S3-compatible, Backblaze B2 |
| ✅ | P1 | [remote-smb-nfs.md](remote-smb-nfs.md) | SMB, AFP, NFS network shares |
| ✅ | P2 | [workspaces-favorites.md](workspaces-favorites.md) | Favorites, groups, workspaces, iCloud sync of favorites |
| ✅ | P2 | [tags.md](tags.md) | Finder tags |
| ✅ | P2 | [cloud.md](cloud.md) | Google Drive, Dropbox, OneDrive, iCloud Drive |
| ✅ | P2 | [app-deleter.md](app-deleter.md) | Application uninstaller |

Shared concepts used across specs (defined in panes-tabs.md): **active pane**, **other pane**, **location**, **selection**, **activity**.

Out of scope on purpose: licensing, trial, activation, update checks, newsletter, crash reporting.

## Final summary (analysis pass complete)

All 23 feature areas are specified, with **232 numbered acceptance tests** (IDs `PT-`, `VW-`, `FO-`, `CF-`, `PV-`, `KS-`, `SD-`, `EC-`, `QO-`, `MR-`, `SY-`, `SE-`, `TL-`, `AR-`, `SF-`, `FT-`, `WD-`, `S3-`, `SM-`, `FV-`, `TG-`, `CL-`, `AD-`).

**Suggested build order:** panes-tabs → views → file-ops-transfer-queue + conflicts → edge-cases → preview-quicklook → keyboard-shortcuts / settings-defaults (grow these as features land) → quick-open → multi-rename → search → archives → sync-compare → remote-sftp → remote-ftp → remote-s3 → remote-webdav → remote-smb-nfs → tools → P2.

**Test fixtures the protocol specs assume** (all local, all scriptable): an OpenSSH `sshd` on a high port with a generated key pair; a scripted FTP server (UTF-8 on/off, TLS with a self-signed certificate, LIST-only mode); a WebDAV server with Basic auth plus an anonymous share; MinIO for S3; a mock server for Backblaze B2; a Samba or macOS file-sharing share for SMB.

**Where the specs are firm vs flexible:**
- Firm: everything stated without a *(RustyFinder choice)* tag. That includes the default key bindings, conflict choices and their rules, the sync comparison semantics and the options' meanings, the SFTP client contract (60 s connect timeout, 30 s × 2 keep-alive, one password prompt, honoring `~/.ssh/config`), the S3 and WebDAV request behavior, and the copy-preservation rules.
- Flexible: values tagged *(RustyFinder choice)*. These are sensible defaults that couldn't be confirmed against the reference. Keep them configurable where a setting exists.

**Known gaps** (only observable with GUI access to the reference, which this pass did not have): exact visual layout and spacing, the exact icon-size steps, the exact set of preset date formats in Multi-Rename, and default values of the transfer settings. If a later analysis pass gets GUI access, those values will be updated in place, and the change will be noted in this section.
