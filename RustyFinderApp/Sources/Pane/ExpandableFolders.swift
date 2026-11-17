import AppKit

/// Expandable folders in List view: a disclosure triangle on each folder,
/// ⌘→ / ⌘← to expand or collapse the selected folders (⌥ for everything
/// inside them too). Items inside expanded folders are ordinary items:
/// select, copy, rename and delete work on them as on any other.
extension FileListViewController {
    /// Recursive expansion stops at this depth and item count, so ⌥⌘→ on a
    /// big tree can't flood the list.
    private static let maxRecursiveDepth = 8
    private static let maxRecursiveItems = 5000

    /// Folders can be expanded in a plain list (not grouped, not search
    /// results, not on another layout).
    var canExpand: Bool { layout == .list && content.groups.isEmpty && search == nil }

    @objc func toggleDisclosure(_ sender: NSButton) {
        let tableRow = tableView.row(for: sender)
        guard tableRow >= 0, let item = content.item(atTableRow: tableRow), let path = content.path(at: item) else { return }
        let recursive = NSEvent.modifierFlags.contains(.option)
        if content.isExpanded(path) {
            collapse([path])
        } else {
            expand([path], recursive: recursive)
        }
    }

    /// ⌘→ (⌥⌘→ recursive).
    func expandSelection(recursive: Bool) {
        guard canExpand else { return }
        let folders = selection.compactMap { index -> String? in
            guard let row = content.row(at: index), row.isDir, let path = content.path(at: index),
                  isRemote || !NSWorkspace.shared.isFilePackage(atPath: path) else { return nil }
            return path
        }
        expand(folders, recursive: recursive)
    }

    /// ⌘← (⌥⌘← also collapses everything inside). On an item inside an
    /// expanded folder, collapses that folder and selects it.
    func collapseSelection(recursive: Bool) {
        guard canExpand else { return }
        var targets: [String] = []
        var select: [String] = []
        for index in selection {
            guard let path = content.path(at: index) else { continue }
            if content.isExpanded(path) {
                targets.append(path)
            } else if content.depth(at: index) > 0 {
                let parent = (path as NSString).deletingLastPathComponent
                targets.append(parent)
                select.append(parent)
            }
        }
        _ = recursive  // collapsing always closes what is inside too
        collapse(targets, select: select)
    }

    func expand(_ paths: [String], recursive: Bool, depth: Int = 0, budget: Int = maxRecursiveItems) {
        guard canExpand, !paths.isEmpty, let view = session.view(tab: tabId) else { return }
        let core = AppEnvironment.shared.core
        let connection = conn
        for path in paths where !content.isExpanded(path) || depth > 0 {
            content.expand(path)
            DispatchQueue.global(qos: .userInitiated).async {
                let listing = try? core.listFolder(connection: connection, path: path, view: view)
                DispatchQueue.main.async {
                    MainActor.assumeIsolated { [weak self] in
                        guard let self, let listing, self.content.isExpanded(path) else { return }
                        self.changeExpansion { self.content.setListing(listing, for: path) }
                        guard recursive, depth + 1 < Self.maxRecursiveDepth else { return }
                        let left = budget - Int(listing.count())
                        guard left > 0 else { return }
                        let subfolders = listing.rows(start: 0, count: listing.count()).filter(\.isDir)
                            .map { (path as NSString).appendingPathComponent($0.name) }
                            .filter { connection != 0 || !NSWorkspace.shared.isFilePackage(atPath: $0) }
                        self.expand(subfolders, recursive: true, depth: depth + 1, budget: left)
                    }
                }
            }
        }
        reloadDisclosures()
    }

    func collapse(_ paths: [String], select: [String] = []) {
        guard !paths.isEmpty else { return }
        changeExpansion(select: select) { paths.forEach(content.collapse) }
    }

    /// The folder was re-read or re-sorted: read the expanded folders again
    /// so their contents follow (they are not watched themselves).
    func relistExpanded() {
        guard let view = session.view(tab: tabId) else { return }
        let core = AppEnvironment.shared.core
        let connection = conn
        for path in content.expanded.keys {
            DispatchQueue.global(qos: .utility).async {
                let listing = try? core.listFolder(connection: connection, path: path, view: view)
                DispatchQueue.main.async {
                    MainActor.assumeIsolated { [weak self] in
                        guard let self else { return }
                        // Gone (renamed, deleted): stop showing it expanded.
                        guard let listing else {
                            self.changeExpansion { self.content.collapse(path) }
                            return
                        }
                        self.changeExpansion { self.content.setListing(listing, for: path) }
                    }
                }
            }
        }
    }

    /// Activities report every step; re-read once things settle.
    func relistExpandedSoon() {
        NSObject.cancelPreviousPerformRequests(withTarget: self, selector: #selector(relistExpandedNow), object: nil)
        perform(#selector(relistExpandedNow), with: nil, afterDelay: 0.3)
    }

    @objc private func relistExpandedNow() { relistExpanded() }

    /// Apply a change to what is expanded, keeping the selection by path.
    private func changeExpansion(select: [String] = [], _ change: () -> Void) {
        let kept = select.isEmpty ? Set(selection.compactMap { content.path(at: $0) }) : Set(select)
        change()
        tableView.reloadData()
        setSelection(content.indices(ofPaths: kept))
        onChange?()
    }

    private func reloadDisclosures() {
        let name = tableView.column(withIdentifier: ListColumn.name.id)
        guard name >= 0 else { return }
        let visible = tableView.rows(in: tableView.visibleRect)
        tableView.reloadData(
            forRowIndexes: IndexSet(integersIn: visible.location..<(visible.location + visible.length)), columnIndexes: [name])
    }

    /// Indent and triangle for a row's name cell.
    func configureDisclosure(_ cell: NSTableCellView, row: Row, index: Int) {
        guard let button = cell.subviews.first(where: { $0.identifier == Self.disclosureId }) as? NSButton,
              let indent = cell.constraints.first(where: { $0.identifier == Self.indentId })
        else { return }
        let expandable = canExpand
        let depth = CGFloat(content.depth(at: index))
        let path = content.path(at: index)
        let isFolder = row.isDir && (isRemote || path.map { !NSWorkspace.shared.isFilePackage(atPath: $0) } ?? false)
        button.isHidden = !(expandable && isFolder)
        button.state = path.map(content.isExpanded) == true ? .on : .off
        button.setAccessibilityLabel(button.state == .on ? "Collapse \(row.name)" : "Expand \(row.name)")
        indent.constant = expandable ? 18 + depth * 16 : 2
    }

    static let disclosureId = NSUserInterfaceItemIdentifier("disclosure")
    static let indentId = "indent"

    /// The triangle, added to a new name cell.
    func addDisclosure(to cell: NSTableCellView, imageLeading: NSLayoutConstraint) {
        let button = NSButton()
        button.bezelStyle = .disclosure
        button.setButtonType(.pushOnPushOff)
        button.title = ""
        button.identifier = Self.disclosureId
        button.target = self
        button.action = #selector(toggleDisclosure(_:))
        button.translatesAutoresizingMaskIntoConstraints = false
        button.isHidden = true
        cell.addSubview(button)
        imageLeading.identifier = Self.indentId
        NSLayoutConstraint.activate([
            button.trailingAnchor.constraint(equalTo: cell.imageView!.leadingAnchor, constant: -2),
            button.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
    }

    /// "folder a|>inner|Folder B" for scripted checks: ">" per level.
    var debugTree: String {
        (0..<min(content.count, 60)).compactMap { index in
            content.row(at: index).map { String(repeating: ">", count: content.depth(at: index)) + $0.name }
        }.joined(separator: "|")
    }
}
