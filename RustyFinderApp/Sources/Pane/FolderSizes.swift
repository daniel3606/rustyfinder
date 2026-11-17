import AppKit

extension Notification.Name {
    /// A folder size was calculated (object: the folder's path).
    static let rustyfinderFolderSizeChanged = Notification.Name("RustyFinderFolderSizeChanged")
}

/// Folder sizes worked out on request ("Calculate Folder Size", or the
/// Calculate button in Info). Shown in the Size column, the status line and
/// Info until the folder holding them is re-read.
@MainActor
final class FolderSizes {
    static let shared = FolderSizes()

    private final class Measurer: MeasureListener, @unchecked Sendable {
        func measured(path: String, items: UInt64, bytes: UInt64) {
            DispatchQueue.main.async {
                MainActor.assumeIsolated { FolderSizes.shared.finished(path, items, bytes) }
            }
        }
    }

    private var sizes: [String: (items: UInt64, bytes: UInt64)] = [:]
    private var pending: Set<String> = []
    private let measurer = Measurer()

    /// Items inside (counting the folder itself) and bytes.
    func size(of path: String) -> (items: UInt64, bytes: UInt64)? { sizes[path] }

    func isCalculating(_ path: String) -> Bool { pending.contains(path) }

    func calculate(_ paths: [String]) {
        for path in paths where !pending.contains(path) {
            pending.insert(path)
            AppEnvironment.shared.core.measure(path: path, listener: measurer)
        }
        if !paths.isEmpty { NotificationCenter.default.post(name: .rustyfinderFolderSizeChanged, object: nil) }
    }

    /// The folder `dir` was re-read: sizes of what it holds may be stale.
    func forget(inside dir: String) {
        let prefix = dir.hasSuffix("/") ? dir : dir + "/"
        sizes = sizes.filter { !$0.key.hasPrefix(prefix) || $0.key.dropFirst(prefix.count).contains("/") }
    }

    private func finished(_ path: String, _ items: UInt64, _ bytes: UInt64) {
        pending.remove(path)
        sizes[path] = (items, bytes)
        NotificationCenter.default.post(name: .rustyfinderFolderSizeChanged, object: path)
    }
}

extension FileListViewController {
    /// ⌘⇧S: work out the size of the selected folders.
    @objc func calculateFolderSize(_ sender: Any?) {
        FolderSizes.shared.calculate(selection.compactMap { index in
            content.row(at: index)?.isDir == true ? content.path(at: index) : nil
        })
    }

    /// Every folder in the view.
    @objc func calculateAllFolderSizes(_ sender: Any?) {
        FolderSizes.shared.calculate((0..<content.count).compactMap { index in
            content.row(at: index)?.isDir == true ? content.path(at: index) : nil
        })
    }

    var hasSelectedFolder: Bool { selection.contains { content.row(at: $0)?.isDir == true } }

    var hasAnyFolder: Bool { (0..<content.count).contains { content.row(at: $0)?.isDir == true } }

    @objc func selectAllFiles(_ sender: Any?) {
        setSelection(IndexSet((0..<content.count).filter { content.row(at: $0)?.isDir == false }))
    }

    @objc func selectAllFolders(_ sender: Any?) {
        setSelection(IndexSet((0..<content.count).filter { content.row(at: $0)?.isDir == true }))
    }

    @objc func toggleFoldersByName(_ sender: Any?) {
        guard var view = session.view(tab: tabId) else { return }
        view.foldersByName.toggle()
        session.setView(tab: tabId, view: view)
    }

    /// Bytes of the selection, counting folders whose size was calculated.
    func selectedBytes() -> UInt64 {
        selection.reduce(UInt64(0)) { total, index in
            guard let row = content.row(at: index) else { return total }
            if row.isDir {
                return total + (content.path(at: index).flatMap { FolderSizes.shared.size(of: $0)?.bytes } ?? 0)
            }
            return total + row.size
        }
    }

    /// Size cell text: calculated folder sizes, else the presenter's text.
    func sizeText(_ row: Row, at index: Int) -> String {
        guard row.isDir, !isRemote, let path = content.path(at: index) else { return RowPresenter.shared.size(row) }
        if let size = FolderSizes.shared.size(of: path) { return RowPresenter.shared.bytes(size.bytes) }
        return FolderSizes.shared.isCalculating(path) ? "Calculating…" : "--"
    }

    func folderSizesChanged() {
        let column = tableView.column(withIdentifier: ListColumn.size.id)
        guard column >= 0 else { return }
        tableView.reloadData(forRowIndexes: IndexSet(integersIn: 0..<content.tableCount), columnIndexes: [column])
        onChange?()
    }
}
