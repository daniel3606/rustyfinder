import AppKit

/// Shared drop handling. The rule itself (copy or move, and whether the
/// drop makes sense at all) is decided by the Rust core.
@MainActor
enum DropRules {
    /// What this drop would do, or nil if it should be refused.
    static func mode(sources: [String], target: String, modifiers: NSEvent.ModifierFlags, remote: Bool = false) -> OpMode? {
        // Anything involving a server copies, unless Command is held.
        if remote { return modifiers.contains(.command) && !modifiers.contains(.option) ? .move : .copy }
        return AppEnvironment.shared.core.dropMode(
            sources: sources, target: target,
            option: modifiers.contains(.option), command: modifiers.contains(.command))
    }

    /// File paths being dragged, if the drag carries files.
    static func paths(from info: NSDraggingInfo) -> [String] {
        let urls = info.draggingPasteboard.readObjects(
            forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]
        return (urls ?? []).map(\.path)
    }

    /// Run the drop, asking first when the setting says so.
    static func perform(
        sources: [String], target: String, mode: OpMode, window: NSWindow?, windowId: UInt64, dst: UInt64 = 0
    ) {
        // Dragged files are always files on this Mac (connection 0).
        let run = {
            ActivityCenter.shared.submit(
                .transfer(mode: mode, sources: sources, destDir: target, renameTo: nil),
                window: windowId, src: 0, dst: dst)
        }
        guard AppEnvironment.shared.settings.confirmDragOperations, let window else {
            run()
            return
        }
        let alert = NSAlert()
        let count = sources.count == 1 ? "1 item" : "\(sources.count.formatted()) items"
        alert.messageText = "\(mode == .move ? "Move" : "Copy") \(count) to “\(target.folderDisplayName)”?"
        alert.informativeText = target.abbreviatingHome
        alert.addButton(withTitle: mode == .move ? "Move" : "Copy")
        alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: window) { response in
            if response == .alertFirstButtonReturn { run() }
        }
    }
}

extension FileListViewController {
    func installDragAndDrop() {
        let types = [.fileURL] + NSFilePromiseReceiver.readableDraggedTypes.map { NSPasteboard.PasteboardType($0) }
        tableView.registerForDraggedTypes(types)
        // Inside the app and out: other apps receive plain file references.
        tableView.setDraggingSourceOperationMask([.copy, .move, .link, .generic], forLocal: true)
        tableView.setDraggingSourceOperationMask([.copy, .move, .link, .generic], forLocal: false)
        gridView.registerForDraggedTypes(types)
        gridView.setDraggingSourceOperationMask([.copy, .move, .link, .generic], forLocal: true)
        gridView.setDraggingSourceOperationMask([.copy, .move, .link, .generic], forLocal: false)
    }

    // MARK: Icon grid (same rules as the list)

    func collectionView(_ collectionView: NSCollectionView, pasteboardWriterForItemAt indexPath: IndexPath) -> NSPasteboardWriting? {
        let index = content.item(atGridPath: indexPath)
        // Out of an archive: a promise, extracted where the drop lands.
        if isArchive, !isRenaming, let path = content.path(at: index), let row = content.row(at: index) {
            return ArchiveDragOut.provider(conn: conn, path: path, row: row)
        }
        guard !isRenaming, let path = content.path(at: index) else { return nil }
        // A remote item is a promise: downloaded where it is dropped.
        if isRemote, let row = content.row(at: index) { return remoteDragWriter(for: path, row: row) }
        return URL(fileURLWithPath: path) as NSURL
    }

    /// The folder a drop on the grid lands in: the folder item under the
    /// pointer, otherwise the folder being shown.
    private func gridDropTarget(_ indexPath: IndexPath, _ operation: NSCollectionView.DropOperation) -> String? {
        let index = content.item(atGridPath: indexPath)
        if operation == .on, let entry = content.row(at: index), entry.isDir,
           let path = content.path(at: index), isRemote || !NSWorkspace.shared.isFilePackage(atPath: path)
        {
            return path
        }
        return content.path
    }

    func collectionView(
        _ collectionView: NSCollectionView, validateDrop info: NSDraggingInfo,
        proposedIndexPath: AutoreleasingUnsafeMutablePointer<NSIndexPath>,
        dropOperation: UnsafeMutablePointer<NSCollectionView.DropOperation>
    ) -> NSDragOperation {
        guard !isRenaming, view.window?.attachedSheet == nil,
              let target = gridDropTarget(proposedIndexPath.pointee as IndexPath, dropOperation.pointee)
        else { return [] }
        springHover(over: target != content.path ? target : nil)
        let sources = DropRules.paths(from: info)
        if sources.isEmpty {
            let promised = info.draggingPasteboard.canReadObject(forClasses: [NSFilePromiseReceiver.self], options: nil)
            return promised ? .copy : []
        }
        guard let mode = DropRules.mode(sources: sources, target: target, modifiers: NSEvent.modifierFlags, remote: isRemote)
        else { return [] }
        return mode == .copy ? .copy : .move
    }

    func collectionView(
        _ collectionView: NSCollectionView, acceptDrop info: NSDraggingInfo, indexPath: IndexPath,
        dropOperation: NSCollectionView.DropOperation
    ) -> Bool {
        springCancel()
        guard let target = gridDropTarget(indexPath, dropOperation) else { return false }
        let sources = DropRules.paths(from: info)
        if sources.isEmpty { return receivePromises(from: info, into: target) }
        guard let mode = DropRules.mode(sources: sources, target: target, modifiers: NSEvent.modifierFlags, remote: isRemote)
        else { return false }
        DropRules.perform(sources: sources, target: target, mode: mode, window: view.window, windowId: session.id(), dst: conn)
        return true
    }

    func collectionView(_ collectionView: NSCollectionView, draggingSession session: NSDraggingSession, willBeginAt screenPoint: NSPoint, forItemsAt indexPaths: Set<IndexPath>) {
        guard DragTrace.enabled else { return }
        DragTrace.note("grid drag begin items=\(indexPaths.count) at=\(screenPoint) mode=\(DragTrace.mainMode)")
        for path in indexPaths { DragTrace.writer(self.collectionView(collectionView, pasteboardWriterForItemAt: path), row: path.item) }
    }

    func collectionView(_ collectionView: NSCollectionView, draggingSession session: NSDraggingSession, endedAt screenPoint: NSPoint, dragOperation operation: NSDragOperation) {
        DragTrace.note("grid drag END op=\(operation.rawValue) at=\(screenPoint) mode=\(DragTrace.mainMode) \(DragTrace.buttons)")
    }

    // MARK: Source

    func tableView(_ tableView: NSTableView, pasteboardWriterForRow tableRow: Int) -> NSPasteboardWriting? {
        // Out of an archive: a promise, extracted where the drop lands.
        if isArchive, !isRenaming, let index = content.item(atTableRow: tableRow), let path = content.path(at: index),
           let row = content.row(at: index)
        {
            return ArchiveDragOut.provider(conn: conn, path: path, row: row)
        }
        guard !isRenaming, let row = content.item(atTableRow: tableRow), let path = content.path(at: row)
        else { return nil }
        // A remote item is a promise: downloaded where it is dropped.
        if isRemote, let entry = content.row(at: row) { return remoteDragWriter(for: path, row: entry) }
        return URL(fileURLWithPath: path) as NSURL
    }

    func tableView(_ tableView: NSTableView, draggingSession session: NSDraggingSession, willBeginAt screenPoint: NSPoint, forRowIndexes rowIndexes: IndexSet) {
        guard DragTrace.enabled else { return }
        DragTrace.note("list drag begin rows=\(rowIndexes.count) at=\(screenPoint) mode=\(DragTrace.mainMode)")
        for row in rowIndexes { DragTrace.writer(self.tableView(tableView, pasteboardWriterForRow: row), row: row) }
    }

    func tableView(_ tableView: NSTableView, draggingSession session: NSDraggingSession, endedAt screenPoint: NSPoint, operation: NSDragOperation) {
        DragTrace.note("list drag END op=\(operation.rawValue) at=\(screenPoint) mode=\(DragTrace.mainMode) \(DragTrace.buttons)")
    }

    // MARK: Destination

    /// The folder a drop at `row` lands in: a folder row when dropping on
    /// one, otherwise the folder being shown.
    private func dropTarget(row: Int, operation: NSTableView.DropOperation) -> String? {
        if operation == .on, row >= 0, let entry = content.row(at: row), entry.isDir,
           let path = content.path(at: row), isRemote || !NSWorkspace.shared.isFilePackage(atPath: path)
        {
            return path
        }
        return content.path
    }

    func tableView(
        _ tableView: NSTableView, validateDrop info: NSDraggingInfo, proposedRow tableRow: Int,
        proposedDropOperation operation: NSTableView.DropOperation
    ) -> NSDragOperation {
        let row = content.item(atTableRow: tableRow) ?? -1
        guard !isRenaming, view.window?.attachedSheet == nil else { return [] }
        let onFolder = operation == .on && row >= 0 && content.row(at: row)?.isDir == true
        // Anything other than "onto a folder" means "into this location":
        // highlight the whole list instead of a gap between rows.
        if !onFolder { tableView.setDropRow(-1, dropOperation: .on) } else { tableView.setDropRow(tableRow, dropOperation: .on) }
        springHover(over: onFolder ? content.path(at: row) : nil)
        guard let target = dropTarget(row: onFolder ? row : -1, operation: .on) else { return [] }

        let sources = DropRules.paths(from: info)
        if sources.isEmpty {
            // File promises (Photos, Mail, browsers): always written as copies.
            let promised = info.draggingPasteboard.canReadObject(forClasses: [NSFilePromiseReceiver.self], options: nil)
            return promised ? .copy : []
        }
        guard let mode = DropRules.mode(sources: sources, target: target, modifiers: NSEvent.modifierFlags, remote: isRemote)
        else { return [] }
        // .copy shows the "+" badge; .move shows the plain arrow.
        return mode == .copy ? .copy : .move
    }

    func tableView(
        _ tableView: NSTableView, acceptDrop info: NSDraggingInfo, row tableRow: Int,
        dropOperation: NSTableView.DropOperation
    ) -> Bool {
        springCancel()
        let row = content.item(atTableRow: tableRow) ?? -1
        guard let target = dropTarget(row: row, operation: dropOperation) else { return false }
        let sources = DropRules.paths(from: info)
        if !sources.isEmpty {
            guard let mode = DropRules.mode(sources: sources, target: target, modifiers: NSEvent.modifierFlags, remote: isRemote)
            else { return false }
            DropRules.perform(sources: sources, target: target, mode: mode, window: view.window, windowId: session.id(), dst: conn)
            return true
        }
        return receivePromises(from: info, into: target)
    }

    /// Accept promised files: the source app writes them into the folder.
    func receivePromises(from info: NSDraggingInfo, into target: String) -> Bool {
        let receivers = info.draggingPasteboard.readObjects(forClasses: [NSFilePromiseReceiver.self], options: nil) as? [NSFilePromiseReceiver]
        guard let receivers, !receivers.isEmpty else { return false }
        let queue = OperationQueue()
        let destination = URL(fileURLWithPath: target)
        for receiver in receivers {
            receiver.receivePromisedFiles(atDestination: destination, options: [:], operationQueue: queue) { [weak self] url, error in
                guard error == nil else { return }
                DispatchQueue.main.async {
                    self?.reveal(names: [url.lastPathComponent], renameAfter: false)
                }
            }
        }
        return true
    }
}
