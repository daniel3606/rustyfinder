import AppKit

/// The right-click menu of the file list, and the commands only it offers.
extension FileListViewController: NSMenuDelegate {
    func installContextMenu() {
        let menu = NSMenu()
        menu.delegate = self
        tableView.menu = menu
        // Each view needs its own menu object; both are filled the same way.
        let gridMenu = NSMenu()
        gridMenu.delegate = self
        gridView.menu = gridMenu
    }

    /// Rows the menu acts on: the selection if the click landed in it,
    /// otherwise just the clicked row (which AppKit highlights for us).
    private var menuRows: IndexSet {
        let clicked = clickedIndex
        if clicked < 0 { return [] }
        return selection.contains(clicked) ? selection : [clicked]
    }

    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        let rows = menuRows
        // Make the menu's target the selection, so the ordinary commands
        // (which read the selection) act on what was clicked.
        if !rows.isEmpty, rows != selection {
            setSelection(rows)
        }
        let paths = rows.compactMap { content.path(at: $0) }
        let single = rows.count == 1 ? rows.first.flatMap { content.row(at: $0) } : nil
        let singlePath = rows.count == 1 ? paths.first : nil

        func add(_ title: String, _ action: Selector, _ object: Any? = nil, tag: Int = 0) {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
            item.representedObject = object
            item.tag = tag
            menu.addItem(item)
        }

        if rows.isEmpty {
            // Empty space: commands for the folder itself.
            add("New Folder", #selector(newFolder(_:)))
            add("New File", #selector(newFile(_:)))
            menu.addItem(.separator())
            add("Paste", #selector(paste(_:)))
            menu.addItem(.separator())
            if let dir = content.path { menu.addItem(isRemote ? remoteCopyPathItem(for: [dir]) : copyPathItem(for: [dir])) }
            if !isRemote { add("Reveal in Finder", #selector(revealInFinder(_:)), content.path.map { [$0] }) }
            return
        }

        add("Open", #selector(openSelection(_:)))
        if rows.contains(where: { content.row(at: $0)?.isDir == false }) { add("Edit", #selector(editSelection(_:))) }
        if let singlePath, single?.isDir == false {
            // On a server the file is downloaded first (see RemoteItemActions).
            menu.addItem(isRemote ? remoteOpenWithItem(for: singlePath) : openWithItem(for: singlePath))
        }
        if rows.contains(where: { folderPathForMenu(at: $0) != nil }) {
            add("Open in New Tab", #selector(openInNewTab(_:)))
            if rows.count == 1 { add("Open in Other Pane", #selector(openInOtherPane(_:))) }
            if rows.count == 1, !isRemote, let folder = rows.first.flatMap(folderPathForMenu(at:)) {
                add("Open in New Window", #selector(MainWindowController.openSelectionInNewWindow(_:)), folder)
            }
        }
        // Single-item commands are left out when several items are selected.
        if !isRemote, let singlePath, let single {
            if single.isDir, NSWorkspace.shared.isFilePackage(atPath: singlePath) {
                add("Show Package Contents", #selector(showPackageContents(_:)), singlePath)
            }
            if single.kind == .symlink {
                add("Show Original", #selector(showOriginal(_:)), singlePath)
            }
        }
        if let single, !single.isDir, isArchiveName(name: single.name) {
            add("Show Archive Contents", #selector(MainWindowController.showArchiveContents(_:)))
        }
        if paths.contains(where: { isArchiveName(name: ($0 as NSString).lastPathComponent) }), !isArchive {
            add("Extract", #selector(MainWindowController.extractHere(_:)))
            add("Extract to", #selector(MainWindowController.extractTo(_:)))
        }
        if !isArchive { add("Compress", #selector(MainWindowController.compress(_:))) }
        if paths.contains(where: { _ in single?.isDir != true }) || rows.count > 1 {
            add("Checksum…", #selector(MainWindowController.checksum(_:)))
        }
        if !isRemote, !isArchive { menu.addItem(ScmBadge.contextMenuItem()) }
        // The user's own tools.
        let tools = ToolCenter.shared.menuItems(contextMenuOnly: true)
        if !tools.isEmpty {
            let submenu = NSMenu()
            tools.forEach { submenu.addItem($0) }
            let item = NSMenuItem(title: "Tools", action: nil, keyEquivalent: "")
            item.submenu = submenu
            menu.addItem(item)
        }
        menu.addItem(.separator())

        add("Copy to", #selector(MainWindowController.copyToOtherPane(_:)))
        add("Move to", #selector(MainWindowController.moveToOtherPane(_:)))
        if singlePath != nil { add("Copy to as…", #selector(MainWindowController.copyToAs(_:))) }
        menu.addItem(.separator())

        add("Duplicate", #selector(duplicate(_:)))
        add(rows.count == 1 ? "Rename" : "Rename \(rows.count) Items…", #selector(rename(_:)))
        if !isRemote { add("Make Alias", #selector(makeAlias(_:))) }
        add("Make Symlink", #selector(makeSymlink(_:)))
        add("New Folder with Selection", #selector(newFolderWithSelection(_:)))
        if isRemote {
            add("Delete…", #selector(deleteImmediately(_:)))
        } else {
            add("Move to Trash", #selector(moveToTrash(_:)))
            if paths.contains(where: AppDeleter.isApp) {
                add("Move App to Trash with Related Files…", #selector(deleteAppWithRelatedFiles(_:)))
            }
            // Holding Option turns Move to Trash into the permanent variant.
            let delete = NSMenuItem(title: "Delete Immediately…", action: #selector(deleteImmediately(_:)), keyEquivalent: "")
            delete.isAlternate = true
            delete.keyEquivalentModifierMask = .option
            menu.addItem(delete)
        }
        menu.addItem(.separator())

        add("Copy", #selector(copy(_:)))
        // On a folder, Paste goes into that folder, not the one being shown.
        if let singlePath, folderPathForMenu(at: rows.first!) != nil {
            add("Paste into “\(single.map(RowPresenter.shared.displayName) ?? "")”", #selector(pasteIntoFolder(_:)), singlePath)
        } else {
            add("Paste", #selector(paste(_:)))
        }
        menu.addItem(isRemote ? remoteCopyPathItem(for: paths) : copyPathItem(for: paths))
        menu.addItem(.separator())

        if search != nil, let singlePath {
            add("Show in Enclosing Folder", #selector(MainWindowController.showInEnclosingFolder(_:)), singlePath)
        }
        if !isRemote {
            // Colour swatches toggle a tag on everything selected.
            menu.addItem(.separator())
            let swatches = TagSwatchesView(selected: Set(TagStore.common(of: paths)))
            swatches.onPick = { [weak self] name in self?.toggleTag(name) }
            let item = NSMenuItem()
            item.view = swatches
            menu.addItem(item)
            add("Tags…", #selector(editTags(_:)))
            menu.addItem(.separator())
        }
        if !isRemote { add("Reveal in Finder", #selector(revealInFinder(_:)), paths) }
        if !isRemote { add("Share…", #selector(shareSelection(_:))) }
        // iCloud Drive: fetch now, or keep only in iCloud (cloud.md §2).
        if !isRemote {
            if rows.contains(where: { content.row(at: $0).map(CloudItems.isCloudOnly) == true }) {
                add("Download Now", #selector(downloadNow(_:)))
            }
            if paths.contains(where: CloudItems.isInCloud),
               rows.contains(where: { content.row(at: $0).map(CloudItems.isCloudOnly) == false }) {
                add("Remove Download", #selector(removeDownload(_:)))
            }
        }
        if !isRemote, singlePath != nil, single?.isDir == true {
            add("Add to Favorites", #selector(MainWindowController.addToFavorites(_:)), singlePath)
        }
    }

    private func folderPathForMenu(at index: Int) -> String? {
        guard let row = content.row(at: index), row.isDir, let path = content.path(at: index) else { return nil }
        return !isRemote && NSWorkspace.shared.isFilePackage(atPath: path) ? nil : path
    }

    // MARK: Submenus

    func copyPathItem(for paths: [String]) -> NSMenuItem {
        let submenu = NSMenu()
        for (tag, title) in ["Path", "Path for Terminal", "File URL", "Name"].enumerated() {
            let item = NSMenuItem(title: title, action: #selector(copyPathAs(_:)), keyEquivalent: "")
            item.target = self
            item.tag = tag
            item.representedObject = paths
            submenu.addItem(item)
        }
        let item = NSMenuItem(title: "Copy Path", action: nil, keyEquivalent: "")
        item.submenu = submenu
        return item
    }

    /// Applications that can open the file, the default one first.
    func openWithItem(for path: String) -> NSMenuItem {
        let url = URL(fileURLWithPath: path)
        let workspace = NSWorkspace.shared
        let preferred = workspace.urlForApplication(toOpen: url)
        var apps = workspace.urlsForApplications(toOpen: url)
        if let preferred {
            apps.removeAll { $0 == preferred }
            apps.insert(preferred, at: 0)
        }
        let submenu = NSMenu()
        for (index, app) in apps.prefix(24).enumerated() {
            var title = FileManager.default.displayName(atPath: app.path)
            if index == 0, preferred != nil { title += " (default)" }
            let item = NSMenuItem(title: title, action: #selector(openWith(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = [app.path, path]
            let icon = workspace.icon(forFile: app.path)
            icon.size = NSSize(width: 16, height: 16)
            item.image = icon
            submenu.addItem(item)
            if index == 0, preferred != nil, apps.count > 1 { submenu.addItem(.separator()) }
        }
        if apps.isEmpty {
            submenu.addItem(NSMenuItem(title: "No Application Found", action: nil, keyEquivalent: ""))
        }
        let item = NSMenuItem(title: "Open With", action: nil, keyEquivalent: "")
        item.submenu = submenu
        return item
    }

    // MARK: Commands

    @objc func openWith(_ sender: NSMenuItem) {
        guard let pair = sender.representedObject as? [String], pair.count == 2 else { return }
        NSWorkspace.shared.open(
            [URL(fileURLWithPath: pair[1])], withApplicationAt: URL(fileURLWithPath: pair[0]),
            configuration: NSWorkspace.OpenConfiguration())
    }

    /// Browse a bundle as a folder.
    @objc func showPackageContents(_ sender: NSMenuItem) {
        guard let path = sender.representedObject as? String else { return }
        session.navigate(tab: tabId, path: path)
    }

    /// Go to a symlink's target and select it. A broken link says which
    /// path is missing.
    @objc func showOriginal(_ sender: NSMenuItem) {
        guard let path = sender.representedObject as? String else { return }
        let fm = FileManager.default
        guard let stored = try? fm.destinationOfSymbolicLink(atPath: path) else { return }
        let target = stored.hasPrefix("/")
            ? stored
            : ((path as NSString).deletingLastPathComponent as NSString).appendingPathComponent(stored)
        let resolved = (target as NSString).standardizingPath
        guard fm.fileExists(atPath: resolved) else {
            guard let window = view.window else { return }
            let alert = NSAlert()
            alert.alertStyle = .warning
            alert.messageText = "The original item for “\((path as NSString).lastPathComponent)” can’t be found."
            alert.informativeText = resolved
            alert.beginSheetModal(for: window)
            return
        }
        session.navigate(tab: tabId, path: (resolved as NSString).deletingLastPathComponent)
        reveal(names: [(resolved as NSString).lastPathComponent], renameAfter: false)
    }

    @objc func pasteIntoFolder(_ sender: NSMenuItem) {
        guard let folder = sender.representedObject as? String else { return }
        let (source, sources) = FileClipboard.contents()
        guard !sources.isEmpty else { return }
        let wasCut = FileClipboard.cutChangeCount == NSPasteboard.general.changeCount
        ActivityCenter.shared.submit(
            .transfer(mode: wasCut ? .move : .copy, sources: sources, destDir: folder, renameTo: nil),
            window: session.id(), src: source, dst: conn)
        if wasCut { FileClipboard.cutChangeCount = nil }
    }

    /// Tag picks the format: 0 path, 1 Terminal-escaped, 2 file URL, 3 name.
    /// Several items give one line each.
    @objc func copyPathAs(_ sender: NSMenuItem) {
        guard let paths = sender.representedObject as? [String] else { return }
        let text = paths.map { PathFormat(rawValue: sender.tag)?.format($0) ?? $0 }.joined(separator: "\n")
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    @objc func revealInFinder(_ sender: NSMenuItem) {
        guard let paths = sender.representedObject as? [String], !paths.isEmpty else { return }
        NSWorkspace.shared.activateFileViewerSelecting(paths.map { URL(fileURLWithPath: $0) })
    }
}

enum PathFormat: Int {
    case posix, terminal, fileURL, name

    func format(_ path: String) -> String {
        switch self {
        case .posix: return path
        case .terminal: return shellEscape(path: path)
        case .fileURL: return URL(fileURLWithPath: path).absoluteString
        case .name: return (path as NSString).lastPathComponent
        }
    }
}
