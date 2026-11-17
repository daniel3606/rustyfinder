import AppKit

/// File commands for the list: clipboard, new items, duplicate, rename,
/// trash and delete. Each one hands a `FileOp` to the activity queue.
extension FileListViewController {
    private var windowId: UInt64 { session.id() }

    /// Run an operation on the file system this tab shows.
    private func submit(_ op: FileOp) {
        ActivityCenter.shared.submit(op, window: windowId, src: conn, dst: conn)
    }

    // MARK: Clipboard

    /// Copy puts file references on the pasteboard, plus the paths as text
    /// so pasting into a text editor gives the paths.
    @objc func copy(_ sender: Any?) {
        writeSelectionToPasteboard()
        FileClipboard.cutChangeCount = nil
    }

    /// Cut is Copy that remembers to move on Paste.
    @objc func cut(_ sender: Any?) {
        writeSelectionToPasteboard()
        FileClipboard.cutChangeCount = NSPasteboard.general.changeCount
    }

    private func writeSelectionToPasteboard() {
        let paths = selectedPaths
        guard !paths.isEmpty else { return }
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        if isRemote {
            // Remote items are not files on this Mac, so other apps get
            // their paths as text. RustyFinder remembers where they live.
            pasteboard.setString(paths.joined(separator: "\n"), forType: .string)
            FileClipboard.remote = (conn, paths, pasteboard.changeCount)
        } else {
            pasteboard.writeObjects(paths.map { URL(fileURLWithPath: $0) as NSURL })
            pasteboard.addTypes([.string], owner: nil)
            pasteboard.setString(paths.joined(separator: "\n"), forType: .string)
            FileClipboard.remote = nil
        }
    }

    @objc func paste(_ sender: Any?) {
        // Items that were cut are moved; copied ones are copied.
        let wasCut = FileClipboard.cutChangeCount == NSPasteboard.general.changeCount
        transferFromPasteboard(mode: wasCut ? .move : .copy)
        if wasCut { FileClipboard.cutChangeCount = nil }
    }

    @objc func moveItemsHere(_ sender: Any?) {
        transferFromPasteboard(mode: .move)
    }

    private func transferFromPasteboard(mode: OpMode) {
        let (source, sources) = FileClipboard.contents()
        guard !sources.isEmpty, let dir = content.path else { return }
        let (window, conn) = (session.id(), conn)
        // Moving needs the sources to be changeable; copying does not.
        LocalPreflight.run(mode == .move ? sources : [], conn: source, verb: "moved", window: view.window) {
            ActivityCenter.shared.submit(
                .transfer(mode: mode, sources: sources, destDir: dir, renameTo: nil),
                window: window, src: source, dst: conn)
        }
    }

    // MARK: New items

    @objc func newFolder(_ sender: Any?) {
        guard let dir = content.path else { return }
        submit(.newFolder(dir: dir))
    }

    @objc func newFile(_ sender: Any?) {
        guard let dir = content.path else { return }
        submit(.newFile(dir: dir))
    }

    @objc func newFolderWithSelection(_ sender: Any?) {
        let paths = selectedPaths
        guard !paths.isEmpty, let dir = content.path else { return }
        submit(.newFolderWithSelection(dir: dir, paths: paths))
    }

    @objc func duplicate(_ sender: Any?) {
        let paths = selectedPaths
        guard !paths.isEmpty else { return }
        submit(.duplicate(paths: paths))
    }

    /// ⌃⌘A: a Finder alias next to each selected item.
    @objc func makeAlias(_ sender: Any?) {
        let paths = selectedPaths
        guard !paths.isEmpty, !isRemote, let dir = content.path else { return }
        submit(.alias(paths: paths, destDir: dir))
    }

    @objc func makeSymlink(_ sender: Any?) {
        let paths = selectedPaths
        guard !paths.isEmpty, let dir = content.path else { return }
        submit(.symlink(paths: paths, destDir: dir))
    }

    // MARK: Removing

    /// Straight to the Trash, no question asked: it can be undone. A
    /// server has no Trash, so there the command deletes, after asking.
    @objc func moveToTrash(_ sender: Any?) {
        if isRemote {
            deleteImmediately(sender)
            return
        }
        let paths = selectedPaths
        guard !paths.isEmpty else { return }
        // With "Detect application when deleting" on, apps go through
        // App Deleter so their leftovers can go too; the rest goes now.
        let apps = AppEnvironment.shared.settings.detectAppWhenDeleting ? paths.filter(AppDeleter.isApp) : []
        let rest = paths.filter { !apps.contains($0) }
        LocalPreflight.run(paths, conn: conn, verb: "moved to the Trash", window: view.window) { [weak self] in
            if !rest.isEmpty { self?.submit(.trash(paths: rest)) }
            if !apps.isEmpty, let id = self?.windowId { AppDeleter.start(apps, window: id) }
        }
    }

    /// Commands > "Move App to Trash with Related Files…": App Deleter for
    /// the selected apps, whatever the setting says.
    @objc func deleteAppWithRelatedFiles(_ sender: Any?) {
        let apps = selectedPaths.filter(AppDeleter.isApp)
        guard !isRemote, !apps.isEmpty else { return }
        AppDeleter.start(apps, window: windowId)
    }

    /// Permanent, so it asks every time, with the count.
    @objc func deleteImmediately(_ sender: Any?) {
        let paths = selectedPaths
        guard !paths.isEmpty, let window = view.window else { return }
        let alert = NSAlert()
        alert.alertStyle = .critical
        let what = paths.count == 1
            ? "“\((paths[0] as NSString).lastPathComponent)”"
            : "\(paths.count.formatted()) items"
        alert.messageText = isRemote ? "Delete \(what) from the server?" : "Delete \(what) immediately?"
        alert.informativeText = isRemote
            ? "This can’t be undone. There is no Trash on a server."
            : "This can’t be undone. The items are erased, not moved to the Trash."
        let delete = alert.addButton(withTitle: "Delete")
        delete.hasDestructiveAction = true
        // Cancel answers Return and Esc, so a stray keypress never deletes.
        delete.keyEquivalent = ""
        let cancel = alert.addButton(withTitle: "Cancel")
        cancel.keyEquivalent = "\r"
        pendingConfirmation = alert
        alert.beginSheetModal(for: window) { [weak self] response in
            guard let self else { return }
            self.pendingConfirmation = nil
            guard response == .alertFirstButtonReturn else { return }
            LocalPreflight.run(paths, conn: self.conn, verb: "deleted", window: self.view.window) { [weak self] in
                self?.submit(.delete(paths: paths))
            }
        }
    }

    // MARK: Rename

    /// Return: rename the single selected item in place. With several
    /// items selected, the Multi-Rename window opens instead.
    @objc func rename(_ sender: Any?) {
        let items = inspectedSelection
        if items.count > 1 {
            MultiRenameWindowController.show(items: items, conn: conn, windowId: session.id())
            return
        }
        guard selection.count == 1 else {
            NSSound.beep()
            return
        }
        beginRename(row: (selection.first ?? -1))
    }
}

/// Shared state for Cut: the pasteboard change count at the time of the cut.
@MainActor
enum FileClipboard {
    static var cutChangeCount: Int?
    /// Remote items that were copied: their connection, paths, and the
    /// pasteboard change count at that moment (so a later copy elsewhere
    /// makes them stale).
    static var remote: (connection: UInt64, paths: [String], changeCount: Int)?

    /// File paths currently on the general pasteboard.
    static func paths() -> [String] {
        contents().paths
    }

    /// What Paste would transfer, and which connection it lives on.
    static func contents() -> (connection: UInt64, paths: [String]) {
        if let remote, remote.changeCount == NSPasteboard.general.changeCount {
            return (remote.connection, remote.paths)
        }
        let urls = NSPasteboard.general.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]
        return (0, (urls ?? []).map(\.path))
    }
}
