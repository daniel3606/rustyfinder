import AppKit

/// The columns List view can show. Name is always there and always first;
/// "Where" and "SCM" are switched by the app (search results, git status),
/// every other one by the user from the header's right-click menu.
enum ListColumn: String, CaseIterable {
    case name, scm, location
    case modified, created, added, lastOpened, size, kind, tags
    case owner, group, permissions, comments
    case duration, dimensions, title, authors, album, genre, track, year, rating, codecs, version

    /// Shown until the user changes the set.
    static let defaultVisible: [ListColumn] = [.modified, .size, .kind]

    /// The ones the header menu offers, in the spec's order.
    static let userToggled: [ListColumn] = [
        .modified, .created, .added, .lastOpened, .size, .kind, .tags, .owner, .group, .permissions,
        .comments, .duration, .dimensions, .title, .authors, .album, .genre, .track, .year, .rating,
        .codecs, .version,
    ]

    var title: String {
        switch self {
        case .name: return "Name"
        case .location: return "Where"
        case .scm: return "SCM"
        case .modified: return "Date Modified"
        case .created: return "Date Created"
        case .added: return "Date Added"
        case .lastOpened: return "Date Last Opened"
        case .size: return "Size"
        case .kind: return "Kind"
        case .tags: return "Tags"
        case .owner: return "Owner"
        case .group: return "Group"
        case .permissions: return "Permissions"
        case .comments: return "Comments"
        case .duration: return "Duration"
        case .dimensions: return "Dimensions"
        case .title: return "Title"
        case .authors: return "Authors"
        case .album: return "Album"
        case .genre: return "Musical Genre"
        case .track: return "Track Number"
        case .year: return "Year Recorded"
        case .rating: return "Rating"
        case .codecs: return "Codecs"
        case .version: return "Version"
        }
    }

    var width: CGFloat {
        switch self {
        case .name: return 260
        case .location: return 180
        case .scm: return 44
        case .modified, .created, .added, .lastOpened: return 150
        case .size, .duration, .track, .year, .rating: return 80
        case .permissions: return 100
        case .kind, .comments, .title, .authors, .album, .tags: return 120
        default: return 90
        }
    }

    /// The core sort key behind the column, if the core can sort by it.
    /// Spotlight columns are filled lazily and can't be sorted.
    var sort: SortColumn? {
        switch self {
        case .name: return .name
        case .modified: return .modified
        case .created: return .created
        case .added: return .added
        case .size: return .size
        case .kind: return .kind
        case .tags: return .tags
        case .owner: return .owner
        case .group: return .group
        case .permissions: return .permissions
        default: return nil
        }
    }

    /// Dates and sizes sort newest / largest first on the first click.
    var ascendingFirst: Bool {
        ![.modified, .created, .added, .size].contains(self)
    }

    var rightAligned: Bool { [.size, .duration, .track, .year].contains(self) }

    /// Spotlight attribute the column shows, for the metadata columns.
    var spotlightKey: String? {
        switch self {
        case .lastOpened: return kMDItemLastUsedDate as String
        case .comments: return kMDItemFinderComment as String
        case .duration: return kMDItemDurationSeconds as String
        case .dimensions: return kMDItemPixelWidth as String
        case .title: return kMDItemTitle as String
        case .authors: return kMDItemAuthors as String
        case .album: return kMDItemAlbum as String
        case .genre: return kMDItemMusicalGenre as String
        case .track: return kMDItemAudioTrackNumber as String
        case .year: return kMDItemRecordingYear as String
        case .rating: return kMDItemStarRating as String
        case .codecs: return kMDItemCodecs as String
        case .version: return kMDItemVersion as String
        default: return nil
        }
    }

    var id: NSUserInterfaceItemIdentifier { .init(rawValue) }

    // MARK: Saved choice

    private static let key = "ListColumns"

    /// The user's columns after Name, in their order.
    static var saved: [ListColumn] {
        get {
            guard let raw = Prefs.strings(key) else { return defaultVisible }
            return raw.compactMap(ListColumn.init(rawValue:)).filter { userToggled.contains($0) }
        }
        set { Prefs.setStrings(key, newValue.map(\.rawValue)) }
    }
}

/// Spotlight values for the metadata columns, read when a row first shows
/// and kept until the folder is re-read.
@MainActor
final class SpotlightColumns {
    static let shared = SpotlightColumns()
    private var cache: [String: [String: Any]] = [:]

    func clear() { cache.removeAll(keepingCapacity: true) }

    func text(_ column: ListColumn, path: String) -> String {
        guard let key = column.spotlightKey else { return "" }
        let values = cache[path] ?? load(path)
        switch column {
        case .lastOpened:
            return (values[key] as? Date).map { RowPresenter.shared.date(Int64($0.timeIntervalSince1970 * 1000)) } ?? ""
        case .duration:
            guard let secs = values[key] as? Double else { return "" }
            let s = Int(secs.rounded())
            return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60)
                : String(format: "%d:%02d", s / 60, s % 60)
        case .dimensions:
            guard let w = values[key] as? Int, let h = values[kMDItemPixelHeight as String] as? Int else { return "" }
            return "\(w) × \(h)"
        case .rating:
            guard let stars = values[key] as? Int, stars > 0 else { return "" }
            return String(repeating: "★", count: min(stars, 5))
        default:
            switch values[key] {
            case let list as [String]: return list.joined(separator: ", ")
            case let text as String: return text
            case let number as NSNumber: return number.stringValue
            default: return ""
            }
        }
    }

    private func load(_ path: String) -> [String: Any] {
        var values: [String: Any] = [:]
        if let item = MDItemCreate(nil, path as CFString) {
            var keys = ListColumn.allCases.compactMap(\.spotlightKey)
            keys.append(kMDItemPixelHeight as String)
            for key in keys {
                if let value = MDItemCopyAttribute(item, key as CFString) { values[key] = value }
            }
        }
        cache[path] = values
        return values
    }
}

extension RowPresenter {
    /// "rwxr-xr-x"-style permissions, with the octal value after it.
    func permissions(_ row: Row) -> String {
        guard let mode = row.mode else { return "" }
        let kind = row.isDir ? "d" : (row.kind == .symlink ? "l" : "-")
        var bits = ""
        for shift in stride(from: 6, through: 0, by: -3) {
            let m = (mode >> UInt32(shift)) & 7
            bits += (m & 4 != 0 ? "r" : "-") + (m & 2 != 0 ? "w" : "-") + (m & 1 != 0 ? "x" : "-")
        }
        return kind + bits
    }
}

// MARK: - Header menu, order and fit

extension FileListViewController {
    func installHeaderMenu() {
        let menu = NSMenu(title: "Columns")
        menu.delegate = headerMenuDelegate
        tableView.headerView?.menu = menu
    }

    /// Checkmarks follow the columns on screen each time the menu opens.
    func fillHeaderMenu(_ menu: NSMenu) {
        menu.removeAllItems()
        for column in ListColumn.userToggled {
            let item = NSMenuItem(title: column.title, action: #selector(toggleListColumn(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = column.rawValue
            item.state = tableView.tableColumn(withIdentifier: column.id)?.isHidden == false ? .on : .off
            menu.addItem(item)
        }
    }

    @objc func toggleListColumn(_ sender: NSMenuItem) {
        guard let raw = sender.representedObject as? String, let column = ListColumn(rawValue: raw) else { return }
        setColumn(column, visible: tableView.tableColumn(withIdentifier: column.id)?.isHidden != false)
    }

    /// Show or hide a column and remember the choice for every list.
    func setColumn(_ column: ListColumn, visible: Bool) {
        guard column != .name, let col = tableView.tableColumn(withIdentifier: column.id) else { return }
        col.isHidden = !visible
        if visible {
            // A newly shown column goes after the last visible one.
            let from = tableView.column(withIdentifier: column.id)
            let lastVisible = tableView.tableColumns.lastIndex { !$0.isHidden && $0 !== col } ?? 0
            if from > lastVisible + 1 { tableView.moveColumn(from, toColumn: lastVisible + 1) }
        }
        saveColumns()
        NotificationCenter.default.post(name: .rustyfinderListColumnsChanged, object: self)
    }

    func saveColumns() {
        ListColumn.saved = tableView.tableColumns.compactMap { col in
            guard !col.isHidden, let column = ListColumn(rawValue: col.identifier.rawValue),
                  ListColumn.userToggled.contains(column) else { return nil }
            return column
        }
    }

    /// Another list changed the columns: take the same set and order.
    func adoptSavedColumns() {
        let visible = ListColumn.saved
        for column in ListColumn.userToggled {
            tableView.tableColumn(withIdentifier: column.id)?.isHidden = !visible.contains(column)
        }
        var target = tableView.column(withIdentifier: ListColumn.location.id) + 1
        for column in visible {
            let from = tableView.column(withIdentifier: column.id)
            if from >= 0, from != target { tableView.moveColumn(from, toColumn: target) }
            target += 1
        }
    }

    func tableView(_ tableView: NSTableView, shouldReorderColumn columnIndex: Int, toColumn newColumnIndex: Int) -> Bool {
        // Name stays first.
        let name = tableView.column(withIdentifier: ListColumn.name.id)
        return columnIndex != name && newColumnIndex > name
    }

    func tableViewColumnDidMove(_ notification: Notification) {
        saveColumns()
    }

    /// Double-click on a header divider: fit the column to its widest
    /// visible text.
    func tableView(_ tableView: NSTableView, sizeToFitWidthOfColumn index: Int) -> CGFloat {
        let col = tableView.tableColumns[index]
        var widest = col.headerCell.cellSize.width + 12
        let rows = tableView.rows(in: tableView.visibleRect)
        for row in rows.location..<(rows.location + rows.length) where content.group(atTableRow: row) == nil {
            guard let cell = tableView.view(atColumn: index, row: row, makeIfNecessary: false) as? NSTableCellView,
                  let text = cell.textField else { continue }
            var width = text.attributedStringValue.size().width + 8
            if col.identifier == ListColumn.name.id { width += 16 + 6 + 2 + 24 }  // icon, gap, tag dots
            widest = max(widest, width)
        }
        return ceil(max(col.minWidth, widest))
    }
}

extension Notification.Name {
    /// A list showed or hid a column; the others follow.
    static let rustyfinderListColumnsChanged = Notification.Name("RustyFinderListColumnsChanged")
}

/// Fills the header menu as it opens (a plain object, since the view
/// controller already has a menu delegate for the rows).
final class HeaderMenuDelegate: NSObject, NSMenuDelegate {
    weak var owner: FileListViewController?
    func menuNeedsUpdate(_ menu: NSMenu) { owner?.fillHeaderMenu(menu) }
}

// MARK: - Group headers

extension FileListViewController {
    private static let groupHeaderId = NSUserInterfaceItemIdentifier("groupHeader")

    func groupTitle(_ group: RowGroup) -> String {
        group.ext.map { RowPresenter.shared.kindName(extension: $0) } ?? group.title
    }

    /// The full-width header row of a group.
    func groupHeader(_ group: RowGroup) -> NSView {
        let cell: NSTableCellView
        if let reused = tableView.makeView(withIdentifier: Self.groupHeaderId, owner: self) as? NSTableCellView {
            cell = reused
        } else {
            cell = NSTableCellView()
            cell.identifier = Self.groupHeaderId
            let text = NSTextField(labelWithString: "")
            text.font = .boldSystemFont(ofSize: NSFont.smallSystemFontSize)
            text.textColor = .secondaryLabelColor
            text.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(text)
            cell.textField = text
            NSLayoutConstraint.activate([
                text.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 6),
                text.trailingAnchor.constraint(lessThanOrEqualTo: cell.trailingAnchor, constant: -6),
                text.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
        }
        cell.textField?.stringValue = groupTitle(group)
        return cell
    }
}

extension GroupBy {
    /// Order of the Group By menu (⌃⌥⌘ + digit 0-6); the tag indexes this.
    static let menuOrder: [GroupBy] = [.none, .kind, .added, .modified, .created, .size, .tags]

    var menuTitle: String {
        switch self {
        case .none: return "None"
        case .kind: return "Kind"
        case .added: return "Date Added"
        case .modified: return "Date Modified"
        case .created: return "Date Created"
        case .size: return "Size"
        case .tags: return "Tags"
        }
    }
}
