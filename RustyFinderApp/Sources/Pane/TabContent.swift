import AppKit

/// What one tab currently shows: the Rust snapshot plus a small page cache
/// of rows, so the table pulls rows by index without holding the folder.
@MainActor
final class TabContent {
    private(set) var snapshot: SnapshotHandle?
    private(set) var info: PaneInfo?
    /// Number of items shown (group headers not counted; the contents of
    /// expanded folders counted).
    var count: Int { flat?.count ?? mainCount }
    /// Items of the folder itself.
    private(set) var mainCount = 0

    // MARK: Expanded folders (List view)

    /// One shown item: an entry of the folder, or of an expanded folder
    /// inside it (`depth` levels down).
    private enum Ref {
        case main(Int)
        case child(folder: String, index: Int, depth: Int)
    }

    /// Listings of expanded folders, by path (nil while being read).
    private(set) var expanded: [String: SnapshotHandle?] = [:]
    /// The shown items when anything is expanded; nil means "just the folder".
    private var flat: [Ref]?
    /// Position in `flat` of each entry of the folder itself.
    private var mainPositions: [Int] = []

    var hasExpansions: Bool { !expanded.isEmpty }

    func isExpanded(_ path: String) -> Bool { expanded.keys.contains(path) }

    /// Folders nested `depth` levels below the folder shown (0 for its own).
    func depth(at index: Int) -> Int {
        guard let flat, flat.indices.contains(index), case .child(_, _, let depth) = flat[index] else { return 0 }
        return depth
    }

    /// Shown position of an entry of the folder itself.
    func shownIndex(ofMain index: Int) -> Int {
        flat == nil ? index : (mainPositions.indices.contains(index) ? mainPositions[index] : index)
    }

    /// Mark `path` expanded (its listing arrives with `setListing`).
    func expand(_ path: String) {
        if expanded[path] == nil { expanded[path] = .some(nil) }
    }

    /// Collapse `path` and everything expanded below it.
    func collapse(_ path: String) {
        expanded = expanded.filter { $0.key != path && !$0.key.hasPrefix(path + "/") }
        rebuild()
    }

    func collapseAll() {
        expanded.removeAll()
        rebuild()
    }

    func setListing(_ listing: SnapshotHandle, for path: String) {
        guard expanded.keys.contains(path) else { return }
        expanded[path] = listing
        rebuild()
    }

    /// Lay out the folder with the expanded folders' contents under them.
    /// Grouped lists don't expand (a child would belong to no group).
    func rebuild() {
        guard let snapshot, !expanded.isEmpty, groups.isEmpty else {
            flat = nil
            mainPositions = []
            return
        }
        // Where the expanded folders of this folder sit, by entry index.
        var insertAfter: [Int: String] = [:]
        let base = info?.path ?? ""
        for path in expanded.keys where (path as NSString).deletingLastPathComponent == base {
            if let i = snapshot.indexOf(name: (path as NSString).lastPathComponent) { insertAfter[Int(i)] = path }
        }
        var out: [Ref] = []
        out.reserveCapacity(mainCount + 64)
        var positions = [Int](repeating: 0, count: mainCount)
        func addChildren(of folder: String, depth: Int) {
            guard case .some(.some(let listing)) = expanded[folder] else { return }
            let rows = listing.rows(start: 0, count: listing.count())
            for (j, row) in rows.enumerated() {
                out.append(.child(folder: folder, index: j, depth: depth))
                let childPath = (folder as NSString).appendingPathComponent(row.name)
                if row.isDir, expanded.keys.contains(childPath) { addChildren(of: childPath, depth: depth + 1) }
            }
        }
        for i in 0..<mainCount {
            positions[i] = out.count
            out.append(.main(i))
            if let folder = insertAfter[i] { addChildren(of: folder, depth: 1) }
        }
        flat = out
        mainPositions = positions
    }

    /// Shown positions of these paths (used to keep a selection). Entries
    /// of the folder itself are found by Rust; only expanded contents are
    /// walked here, so a huge folder costs nothing extra.
    func indices(ofPaths paths: Set<String>) -> IndexSet {
        guard !paths.isEmpty, let snapshot else { return [] }
        let base = info?.path ?? ""
        let own = paths.filter { ($0 as NSString).deletingLastPathComponent == base }
        var out = IndexSet(snapshot.indicesOf(names: own.map { ($0 as NSString).lastPathComponent }).map { shownIndex(ofMain: Int($0)) })
        if let flat {
            for (i, ref) in flat.enumerated() {
                if case .child = ref, let p = path(at: i), paths.contains(p) { out.insert(i) }
            }
        }
        return out
    }
    /// Group headers of a grouped list, in order; empty otherwise.
    private(set) var groups: [RowGroup] = []
    /// Table row of each group header.
    private var headerRows: [Int] = []

    /// Rows the list table shows: items plus group headers. Everything
    /// else in the app counts items; only the table sees header rows.
    var tableCount: Int { count + groups.count }

    private static let pageSize = 256
    private static let maxPages = 48
    private var pages: [Int: [Row]] = [:]
    private var pageOrder: [Int] = []

    var path: String? { info?.path }

    /// Install a new snapshot; returns the one it replaced.
    @discardableResult
    func apply(_ snapshot: SnapshotHandle, _ info: PaneInfo) -> SnapshotHandle? {
        let previous = self.snapshot
        self.snapshot = snapshot
        self.info = info
        mainCount = Int(snapshot.count())
        groups = snapshot.groups()
        headerRows = groups.enumerated().map { Int($0.element.start) + $0.offset }
        pages.removeAll(keepingCapacity: true)
        pageOrder.removeAll(keepingCapacity: true)
        // Another folder: nothing in it is expanded.
        if previous?.path() != snapshot.path() { expanded.removeAll() }
        rebuild()
        return previous
    }

    func row(at index: Int) -> Row? {
        if let flat {
            guard flat.indices.contains(index) else { return nil }
            switch flat[index] {
            case .main(let i): return mainRow(i)
            case .child(let folder, let j, _):
                guard case .some(.some(let listing)) = expanded[folder] else { return nil }
                return listing.row(index: UInt32(j))
            }
        }
        return mainRow(index)
    }

    private func mainRow(_ index: Int) -> Row? {
        guard let snapshot, index >= 0, index < mainCount else { return nil }
        let page = index / Self.pageSize
        if pages[page] == nil {
            pages[page] = snapshot.rows(start: UInt32(page * Self.pageSize), count: UInt32(Self.pageSize))
            pageOrder.append(page)
            if pageOrder.count > Self.maxPages {
                pages[pageOrder.removeFirst()] = nil
            }
        }
        let rows = pages[page] ?? []
        let offset = index % Self.pageSize
        return offset < rows.count ? rows[offset] : nil
    }

    /// The group header shown at a table row, if that row is one.
    func group(atTableRow row: Int) -> RowGroup? {
        guard !headerRows.isEmpty, let k = headerRows.firstIndex(of: row) else { return nil }
        return groups[k]
    }

    /// Item shown at a table row; nil for a header or past the end.
    func item(atTableRow row: Int) -> Int? {
        guard row >= 0, row < tableCount else { return nil }
        if headerRows.isEmpty { return row }
        let before = headerRows.partitionIndex { $0 > row }  // headers at or above
        if before > 0, headerRows[before - 1] == row { return nil }
        return row - before
    }

    /// Table row of an item.
    func tableRow(ofItem item: Int) -> Int {
        guard !groups.isEmpty else { return item }
        return item + groups.partitionIndex { Int($0.start) > item }
    }

    // MARK: Icon view: one section per group (a single one when ungrouped)

    var gridSections: Int { groups.isEmpty ? 1 : groups.count }

    func gridCount(inSection section: Int) -> Int {
        guard !groups.isEmpty else { return count }
        let start = Int(groups[section].start)
        let end = section + 1 < groups.count ? Int(groups[section + 1].start) : count
        return end - start
    }

    func gridPath(ofItem item: Int) -> IndexPath {
        guard !groups.isEmpty else { return IndexPath(item: item, section: 0) }
        let section = max(0, groups.partitionIndex { Int($0.start) > item } - 1)
        return IndexPath(item: item - Int(groups[section].start), section: section)
    }

    func item(atGridPath path: IndexPath) -> Int {
        guard !groups.isEmpty, groups.indices.contains(path.section) else { return path.item }
        return Int(groups[path.section].start) + path.item
    }

    func items(atTableRows rows: IndexSet) -> IndexSet {
        IndexSet(rows.compactMap(item(atTableRow:)))
    }

    func tableRows(ofItems items: IndexSet) -> IndexSet {
        IndexSet(items.map(tableRow(ofItem:)))
    }

    func path(at index: Int) -> String? {
        if let flat {
            guard flat.indices.contains(index) else { return nil }
            switch flat[index] {
            case .main(let i): return snapshot?.pathOf(index: UInt32(i))
            case .child(let folder, let j, _):
                guard case .some(.some(let listing)) = expanded[folder] else { return nil }
                return listing.pathOf(index: UInt32(j))
            }
        }
        guard index >= 0, index < mainCount else { return nil }
        return snapshot?.pathOf(index: UInt32(index))
    }
}

extension Array {
    /// Index of the first element for which `isAfter` is true, given that
    /// it is false for a prefix and true for the rest (binary search).
    func partitionIndex(where isAfter: (Element) -> Bool) -> Int {
        var low = 0, high = count
        while low < high {
            let mid = (low + high) / 2
            if isAfter(self[mid]) { high = mid } else { low = mid + 1 }
        }
        return low
    }
}
