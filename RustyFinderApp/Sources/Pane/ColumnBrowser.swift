import AppKit
import QuickLookThumbnailing

/// Column view (⌘3): one column per folder level, Miller-style.
///
/// The column in focus is the tab's own list table, moved in here, so
/// selection, rename, drag and drop, the context menu and every command work
/// on it exactly as in List view. The columns to its left (the folders above)
/// and the one to its right (the selected folder's contents, or a preview of
/// the selected file) are read-only listings of their own.
@MainActor
final class ColumnBrowser: NSView {
    /// Clicked an item in a side column: show `folder` in focus and select `name`.
    var onPick: ((_ folder: String, _ name: String) -> Void)?
    /// Right-click menu for an item of a side column (it does not navigate).
    var makeMenu: ((_ path: String, _ row: Row) -> NSMenu)?

    private let scroll = NSScrollView()
    private let stack = NSStackView()
    /// Holds the focused list (the tab's table and its scroll view).
    let activeHost = NSView()
    private var activeWidth: NSLayoutConstraint!
    private var ancestors: [SideColumn] = []
    private var trailing: NSView?
    private var trailingGeneration = 0
    /// Top of the first column; empty until Column view first shows something.
    private(set) var root = ""
    private(set) var location = ""
    private var conn: UInt64 = 0
    /// The folder shown in the column right of the focused one, if any.
    private(set) var previewFolder: String?

    static let defaultWidth: CGFloat = 230
    private static let widthsKey = "ColumnWidths"

    override init(frame: NSRect) {
        super.init(frame: frame)
        stack.orientation = .horizontal
        stack.spacing = 0
        stack.alignment = .top
        stack.distribution = .fill
        // Hug the columns: any spare width stays empty, right of them.
        stack.setHuggingPriority(.defaultHigh, for: .horizontal)
        stack.translatesAutoresizingMaskIntoConstraints = false
        let document = FlippedView()
        document.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(stack)
        scroll.documentView = document
        scroll.hasHorizontalScroller = true
        scroll.hasVerticalScroller = false
        scroll.autohidesScrollers = true
        scroll.drawsBackground = true
        scroll.translatesAutoresizingMaskIntoConstraints = false
        addSubview(scroll)
        activeHost.translatesAutoresizingMaskIntoConstraints = false
        activeWidth = activeHost.widthAnchor.constraint(equalToConstant: Self.defaultWidth)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: topAnchor),
            scroll.bottomAnchor.constraint(equalTo: bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: document.topAnchor),
            stack.leadingAnchor.constraint(equalTo: document.leadingAnchor),
            stack.bottomAnchor.constraint(equalTo: document.bottomAnchor),
            // Columns stay packed on the left; the document is as wide as
            // they are, or the view if that is wider (never stretching a
            // column to fill the gap).
            stack.trailingAnchor.constraint(lessThanOrEqualTo: document.trailingAnchor),
            document.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
            document.heightAnchor.constraint(equalTo: scroll.contentView.heightAnchor),
            document.widthAnchor.constraint(greaterThanOrEqualTo: scroll.contentView.widthAnchor),
            activeWidth,
        ])
        let snug = document.widthAnchor.constraint(equalTo: stack.widthAnchor)
        snug.priority = .defaultLow
        snug.isActive = true
        setAccessibilityRole(.browser)
        setAccessibilityLabel("Columns")
        // In the window from the start, so the table can take focus at once.
        _ = activeContainer
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    // MARK: Widths

    /// Saved widths by column position; the rest use the default.
    private static var widths: [CGFloat] {
        get { (Prefs.strings(widthsKey) ?? []).compactMap { Double($0) }.map { CGFloat($0) } }
        set { Prefs.setStrings(widthsKey, newValue.map { String(Double($0)) }) }
    }

    private static func width(at index: Int) -> CGFloat {
        widths.indices.contains(index) ? widths[index] : defaultWidth
    }

    /// Resize column `index` (all of them with Option) and remember it.
    fileprivate func resize(_ index: Int, to width: CGFloat, all: Bool, save: Bool) {
        let width = max(120, min(width, 900))
        let count = ancestors.count + 2
        var saved = Self.widths
        while saved.count < count { saved.append(Self.defaultWidth) }
        for i in (all ? Array(0..<count) : [index]) { saved[i] = width }
        applyWidths(saved)
        if save { Self.widths = saved }
    }

    private func applyWidths(_ widths: [CGFloat]) {
        func w(_ i: Int) -> CGFloat { widths.indices.contains(i) ? widths[i] : Self.defaultWidth }
        for (i, column) in ancestors.enumerated() { column.widthConstraint.constant = w(i) }
        activeWidth.constant = w(ancestors.count)
        (trailing as? SideColumn)?.widthConstraint.constant = w(ancestors.count + 1)
        (trailing as? FilePreviewColumn)?.widthConstraint.constant = max(w(ancestors.count + 1), 220)
    }

    /// Double-click on a divider: fit that column to its widest name.
    fileprivate func fit(_ index: Int) {
        let widest: CGFloat
        if index < ancestors.count {
            widest = ancestors[index].widestName()
        } else if index == ancestors.count {
            widest = owner?.widestVisibleName() ?? Self.defaultWidth
        } else if let side = trailing as? SideColumn {
            widest = side.widestName()
        } else {
            return
        }
        // Room for the icon, chevron, scroll bar and bold names.
        resize(index, to: widest + 16 + 6 + 24 + 20, all: false, save: true)
    }

    weak var owner: FileListViewController?

    // MARK: Showing a location

    /// The tab arrived at `path`: rebuild the columns above it.
    func show(location path: String, conn: UInt64, view: ViewOptions) {
        let rerooted = conn != self.conn || !isInside(path, root)
        if rerooted { root = Self.root(for: path, conn: conn) }
        self.conn = conn
        location = path
        let chain = Self.chain(from: root, to: path)  // folders above `path`
        // Keep the columns that still match, list the new ones.
        var kept: [SideColumn] = []
        for (i, folder) in chain.enumerated() {
            let child = i + 1 < chain.count ? chain[i + 1] : path
            let name = (child as NSString).lastPathComponent
            if !rerooted, i < ancestors.count, ancestors[i].path == folder {
                ancestors[i].highlight(name)
                kept.append(ancestors[i])
            } else {
                let column = SideColumn(path: folder, conn: conn, highlight: name)
                column.onPick = { [weak self] folder, name in self?.onPick?(folder, name) }
                column.makeMenu = { [weak self] path, row in self?.makeMenu?(path, row) }
                column.load(view: view)
                kept.append(column)
            }
        }
        for stale in ancestors where !kept.contains(where: { $0 === stale }) { stale.removeFromSuperview() }
        ancestors = kept
        rebuildStack()
        setTrailing(nil)
        applyWidths(Self.widths)
        scrollToEnd()
    }

    /// The selection in the focused column changed.
    func selectionChanged(folder: String?, file: (path: String, row: Row)?, view: ViewOptions, remote: Bool) {
        trailingGeneration += 1
        if let folder {
            if (trailing as? SideColumn)?.path == folder { return }
            let column = SideColumn(path: folder, conn: conn, highlight: nil)
            column.onPick = { [weak self] folder, name in self?.onPick?(folder, name) }
            column.makeMenu = { [weak self] path, row in self?.makeMenu?(path, row) }
            column.load(view: view)
            setTrailing(column)
            previewFolder = folder
        } else if let file {
            setTrailing(FilePreviewColumn(path: file.path, row: file.row, thumbnails: !remote))
            previewFolder = nil
        } else {
            setTrailing(nil)
            previewFolder = nil
        }
        applyWidths(Self.widths)
        scrollToEnd()
    }

    private func setTrailing(_ new: NSView?) {
        trailing?.removeFromSuperview()
        trailing = new
        if let new {
            stack.addArrangedSubview(new)
            new.heightAnchor.constraint(equalTo: stack.heightAnchor).isActive = true
        }
        if new == nil { previewFolder = nil }
    }

    /// The focused column and its divider. Built once and never taken out
    /// of the window: removing it would take the keyboard focus with it.
    private lazy var activeContainer: NSStackView = {
        let handle = ResizeHandle(index: 0, browser: self)
        activeHandle = handle
        let host = NSStackView(views: [activeHost, handle])
        host.spacing = 0
        host.orientation = .horizontal
        host.alignment = .top
        host.translatesAutoresizingMaskIntoConstraints = false
        // Exactly the focused column and its divider; never wider.
        host.setHuggingPriority(.defaultHigh, for: .horizontal)
        activeHost.heightAnchor.constraint(equalTo: host.heightAnchor).isActive = true
        handle.heightAnchor.constraint(equalTo: host.heightAnchor).isActive = true
        stack.addArrangedSubview(host)
        host.heightAnchor.constraint(equalTo: stack.heightAnchor).isActive = true
        return host
    }()
    private var activeHandle: ResizeHandle?

    /// Put the ancestor columns left of the focused one, leaving it alone.
    private func rebuildStack() {
        let container = activeContainer
        for view in stack.arrangedSubviews where view !== container && !ancestors.contains(where: { $0 === view }) {
            stack.removeArrangedSubview(view)
            view.removeFromSuperview()
        }
        for (i, column) in ancestors.enumerated() {
            column.index = i
            column.browser = self
            if stack.arrangedSubviews.firstIndex(of: column) != i {
                if column.superview != nil { stack.removeArrangedSubview(column) }
                stack.insertArrangedSubview(column, at: i)
            }
            if !column.heightPinned {
                column.heightAnchor.constraint(equalTo: stack.heightAnchor).isActive = true
                column.heightPinned = true
            }
        }
        activeHandle?.index = ancestors.count
        if let trailing { stack.addArrangedSubview(trailing) }
    }

    private func scrollToEnd() {
        layoutSubtreeIfNeeded()
        guard let document = scroll.documentView else { return }
        let x = max(0, document.frame.width - scroll.contentView.bounds.width)
        scroll.contentView.scroll(to: NSPoint(x: x, y: 0))
        scroll.reflectScrolledClipView(scroll.contentView)
    }

    // MARK: Paths

    private func isInside(_ path: String, _ folder: String) -> Bool {
        !folder.isEmpty && (folder == "/" || path == folder || path.hasPrefix(folder + "/"))
    }

    /// Where the columns start when Column view opens somewhere new: one
    /// level above, so the folder is seen in its context. Going deeper adds
    /// columns; going above this re-roots one level above the new place.
    static func root(for path: String, conn: UInt64) -> String {
        path == "/" ? "/" : (path as NSString).deletingLastPathComponent
    }

    /// Folders from `root` down to, but not including, `path`.
    static func chain(from root: String, to path: String) -> [String] {
        guard path != root else { return [] }
        var out = [root]
        let rest = path.dropFirst(root == "/" ? 1 : root.count + 1).split(separator: "/").dropLast()
        var current = root
        for part in rest {
            current = current == "/" ? "/\(part)" : "\(current)/\(part)"
            out.append(current)
        }
        return out
    }

    /// ⌘+ / ⌘− changed the text size.
    func reloadForTextSize() {
        ancestors.forEach { $0.reloadForTextSize() }
        (trailing as? SideColumn)?.reloadForTextSize()
    }

    /// "/a|/a/b|[/a/b/c]|>/a/b/c/d" for scripted checks.
    func debugDescription(focusPath: String) -> String {
        var parts = ancestors.map { "\($0.path)=\($0.highlighted ?? "-")" }
        parts.append("[\(focusPath)]")
        if let side = trailing as? SideColumn { parts.append(">\(side.path)") }
        if let file = trailing as? FilePreviewColumn { parts.append(">file:\((file.path as NSString).lastPathComponent)") }
        return parts.joined(separator: "|")
    }

    /// Frames of the columns (layout checks).
    func debugFrames() -> String {
        layoutSubtreeIfNeeded()
        let doc = scroll.documentView?.frame ?? .zero
        return "doc=\(Int(doc.width)) clip=\(Int(scroll.contentView.bounds.width)) stack=\(Int(stack.frame.width)) "
            + stack.arrangedSubviews.map { "\(type(of: $0)):\(Int($0.frame.minX))+\(Int($0.frame.width))" }.joined(separator: " ")
    }

    /// Text of the side columns' rows, for scripted checks.
    func debugRows() -> String {
        ancestors.map { "\(($0.path as NSString).lastPathComponent)(\($0.rowNames().joined(separator: ",")))" }
            .joined(separator: " ")
    }
}

private final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}

/// The thin divider right of a column: drag to resize (Option: all columns),
/// double-click to fit.
private final class ResizeHandle: NSView {
    var index: Int
    weak var browser: ColumnBrowser?
    private var startX: CGFloat = 0
    private var startWidth: CGFloat = 0

    init(index: Int, browser: ColumnBrowser) {
        self.index = index
        self.browser = browser
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        widthAnchor.constraint(equalToConstant: 5).isActive = true
        setAccessibilityElement(false)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.separatorColor.setFill()
        NSRect(x: bounds.midX, y: 0, width: 1, height: bounds.height).fill()
    }

    override func resetCursorRects() { addCursorRect(bounds, cursor: .resizeLeftRight) }

    override func mouseDown(with event: NSEvent) {
        if event.clickCount == 2 {
            browser?.fit(index)
            return
        }
        startX = event.locationInWindow.x
        startWidth = superview.flatMap { host -> CGFloat? in
            (host as? SideColumn).map { $0.widthConstraint.constant }
                ?? (host as? NSStackView)?.arrangedSubviews.first?.frame.width
        } ?? ColumnBrowser.defaultWidth
    }

    override func mouseDragged(with event: NSEvent) {
        let width = startWidth + event.locationInWindow.x - startX
        browser?.resize(index, to: width, all: event.modifierFlags.contains(.option), save: false)
    }

    override func mouseUp(with event: NSEvent) {
        guard event.clickCount < 2 else { return }
        let width = startWidth + event.locationInWindow.x - startX
        browser?.resize(index, to: width, all: event.modifierFlags.contains(.option), save: true)
    }
}

/// A read-only column: a folder above the focused one, or the selected
/// folder's contents to its right.
@MainActor
final class SideColumn: NSView, NSTableViewDataSource, NSTableViewDelegate, NSMenuDelegate {
    let path: String
    let conn: UInt64
    var onPick: ((String, String) -> Void)?
    var makeMenu: ((String, Row) -> NSMenu?)?
    var index = 0 { didSet { divider?.index = index } }
    var heightPinned = false
    weak var browser: ColumnBrowser?
    private(set) var highlighted: String?
    private var snapshot: SnapshotHandle?
    private var rows: [Row] = []
    private let table = NSTableView()
    private let scroll = NSScrollView()
    private(set) var widthConstraint: NSLayoutConstraint!
    private var divider: ResizeHandle?
    private var isSyncing = false

    init(path: String, conn: UInt64, highlight: String?) {
        self.path = path
        self.conn = conn
        self.highlighted = highlight
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        widthConstraint = widthAnchor.constraint(equalToConstant: ColumnBrowser.defaultWidth)
        widthConstraint.isActive = true
        let column = NSTableColumn(identifier: .init("name"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.style = .plain
        table.rowHeight = CGFloat(AppEnvironment.shared.settings.listTextSize) + 9
        table.dataSource = self
        table.delegate = self
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.backgroundColor = .clear
        table.target = self
        table.action = #selector(clicked(_:))
        table.setAccessibilityLabel((path as NSString).lastPathComponent)
        let menu = NSMenu()
        menu.delegate = self
        table.menu = menu
        // Local items can be dragged out to other apps and panes.
        table.setDraggingSourceOperationMask(conn == 0 ? [.copy, .move, .link] : [], forLocal: false)
        table.setDraggingSourceOperationMask([.copy, .move], forLocal: true)
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        addSubview(scroll)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: topAnchor),
            scroll.bottomAnchor.constraint(equalTo: bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
        ])
    }

    override func viewDidMoveToSuperview() {
        super.viewDidMoveToSuperview()
        guard let browser, divider?.browser !== browser || divider?.index != index else { return }
        divider?.removeFromSuperview()
        let handle = ResizeHandle(index: index, browser: browser)
        addSubview(handle)
        NSLayoutConstraint.activate([
            handle.topAnchor.constraint(equalTo: topAnchor),
            handle.bottomAnchor.constraint(equalTo: bottomAnchor),
            handle.trailingAnchor.constraint(equalTo: trailingAnchor),
            scroll.trailingAnchor.constraint(equalTo: handle.leadingAnchor),
        ])
        divider = handle
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func load(view: ViewOptions) {
        let core = AppEnvironment.shared.core
        let (conn, path) = (self.conn, self.path)
        var view = view
        view.group = .none
        DispatchQueue.global(qos: .userInitiated).async {
            let result = try? core.listFolder(connection: conn, path: path, view: view)
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated {
                    guard let self, let result else { return }
                    self.snapshot = result
                    self.rows = result.rows(start: 0, count: result.count())
                    self.table.reloadData()
                    self.highlight(self.highlighted)
                }
            }
        }
    }

    func highlight(_ name: String?) {
        highlighted = name
        guard let name, let row = rows.firstIndex(where: { $0.name == name }) else {
            table.deselectAll(nil)
            return
        }
        isSyncing = true
        table.selectRowIndexes([row], byExtendingSelection: false)
        table.scrollRowToVisible(row)
        isSyncing = false
    }

    func rowNames() -> [String] { rows.map(\.name) }

    func widestName() -> CGFloat {
        let font = NSFont.systemFont(ofSize: CGFloat(AppEnvironment.shared.settings.listTextSize))
        return rows.map { ($0.name as NSString).size(withAttributes: [.font: font]).width }.max() ?? 0
    }

    @objc private func clicked(_ sender: Any?) {
        let row = table.clickedRow
        guard row >= 0, rows.indices.contains(row) else { return }
        onPick?(path, rows[row].name)
    }

    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        let row = table.clickedRow
        guard rows.indices.contains(row), let built = makeMenu?(fullPath(of: rows[row]), rows[row]) else { return }
        for item in built.items {
            built.removeItem(item)
            menu.addItem(item)
        }
    }

    private func fullPath(of row: Row) -> String {
        path == "/" ? "/\(row.name)" : "\(path)/\(row.name)"
    }

    func tableView(_ tableView: NSTableView, pasteboardWriterForRow row: Int) -> NSPasteboardWriting? {
        guard conn == 0, rows.indices.contains(row) else { return nil }
        return URL(fileURLWithPath: fullPath(of: rows[row])) as NSURL
    }

    func reloadForTextSize() {
        table.rowHeight = CGFloat(AppEnvironment.shared.settings.listTextSize) + 9
        table.reloadData()
        highlight(highlighted)
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cell = (tableView.makeView(withIdentifier: ColumnCell.id, owner: self) as? ColumnCell) ?? ColumnCell()
        let item = rows[row]
        let full = conn == 0 ? (path as NSString).appendingPathComponent(item.name) : nil
        cell.configure(row: item, icon: RowPresenter.shared.icon(item, path: full))
        return cell
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { true }
}

/// Name, icon and a chevron for folders.
final class ColumnCell: NSTableCellView {
    static let id = NSUserInterfaceItemIdentifier("columnCell")
    private let chevron = NSImageView()

    init() {
        super.init(frame: .zero)
        identifier = Self.id
        let image = NSImageView()
        image.imageScaling = .scaleProportionallyUpOrDown
        let text = NSTextField(labelWithString: "")
        text.lineBreakMode = .byTruncatingMiddle
        chevron.image = NSImage(systemSymbolName: "chevron.right", accessibilityDescription: nil)
        chevron.contentTintColor = .tertiaryLabelColor
        chevron.symbolConfiguration = .init(pointSize: 9, weight: .semibold)
        for view in [image, text, chevron] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        imageView = image
        textField = text
        NSLayoutConstraint.activate([
            image.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 4),
            image.centerYAnchor.constraint(equalTo: centerYAnchor),
            image.widthAnchor.constraint(equalToConstant: 16),
            image.heightAnchor.constraint(equalToConstant: 16),
            text.leadingAnchor.constraint(equalTo: image.trailingAnchor, constant: 6),
            text.centerYAnchor.constraint(equalTo: centerYAnchor),
            chevron.leadingAnchor.constraint(greaterThanOrEqualTo: text.trailingAnchor, constant: 4),
            chevron.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -6),
            chevron.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func configure(row: Row, icon: NSImage) {
        textField?.stringValue = RowPresenter.shared.displayName(row)
        textField?.font = .systemFont(ofSize: CGFloat(AppEnvironment.shared.settings.listTextSize))
        textField?.textColor = row.isHidden ? .tertiaryLabelColor : .labelColor
        imageView?.image = icon
        chevron.isHidden = !row.isDir
    }
}

/// Right of a selected file: a large thumbnail and the basics.
@MainActor
final class FilePreviewColumn: NSView {
    let path: String
    private(set) var widthConstraint: NSLayoutConstraint!
    private let image = NSImageView()

    init(path: String, row: Row, thumbnails: Bool) {
        self.path = path
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        let presenter = RowPresenter.shared
        image.image = presenter.icon(row, path: thumbnails ? path : nil)
        image.imageScaling = .scaleProportionallyUpOrDown
        let name = NSTextField(wrappingLabelWithString: presenter.displayName(row))
        name.font = .boldSystemFont(ofSize: NSFont.systemFontSize)
        name.alignment = .center
        var lines = [presenter.kind(row)]
        if !row.isDir { lines.append(presenter.bytes(row.size)) }
        if let modified = row.modifiedMs { lines.append("Modified \(presenter.date(modified))") }
        let details = NSTextField(wrappingLabelWithString: lines.joined(separator: "\n"))
        details.alignment = .center
        details.textColor = .secondaryLabelColor
        details.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        let stack = NSStackView(views: [image, name, details])
        stack.orientation = .vertical
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        widthConstraint = widthAnchor.constraint(equalToConstant: 260)
        widthConstraint.isActive = true
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor, constant: 24),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 16),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -16),
            image.widthAnchor.constraint(equalToConstant: 160),
            image.heightAnchor.constraint(equalToConstant: 160),
        ])
        setAccessibilityLabel("Preview of \(row.name)")
        guard thumbnails else { return }
        let request = QLThumbnailGenerator.Request(
            fileAt: URL(fileURLWithPath: path), size: CGSize(width: 160, height: 160),
            scale: NSScreen.main?.backingScaleFactor ?? 2, representationTypes: .thumbnail)
        QLThumbnailGenerator.shared.generateBestRepresentation(for: request) { [weak self] rep, _ in
            guard let cg = rep?.cgImage else { return }
            DispatchQueue.main.async {
                MainActor.assumeIsolated { self?.image.image = NSImage(cgImage: cg, size: .zero) }
            }
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }
}

// MARK: - The list controller's side of Column view

extension FileListViewController {
    func pinBelowBanner(_ child: NSView, in root: NSView) {
        child.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            child.topAnchor.constraint(equalTo: root.subviews.first { $0 is ErrorBanner }!.bottomAnchor),
            child.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            child.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            child.bottomAnchor.constraint(equalTo: root.bottomAnchor),
        ])
    }

    /// Move the list table into the browser as its focused column, showing
    /// names only.
    func enterColumns() {
        let scroll = tableScrollView
        scroll.removeFromSuperview()
        let host = columnBrowser.activeHost
        host.addSubview(scroll)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: host.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: host.bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: host.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: host.trailingAnchor),
        ])
        scroll.hasHorizontalScroller = false
        if tableView.headerView != nil { stashHeader() }
        for column in tableView.tableColumns where column.identifier != ListColumn.name.id { column.isHidden = true }
        tableView.sizeLastColumnToFit()
        tableView.reloadData()
    }

    /// Put the table back as the plain list, with its header and columns.
    func leaveColumns() {
        let scroll = tableScrollView
        scroll.removeFromSuperview()
        view.addSubview(scroll, positioned: .below, relativeTo: gridScrollView)
        pinBelowBanner(scroll, in: view)
        scroll.hasHorizontalScroller = true
        restoreHeader()
        adoptSavedColumns()
        tableView.tableColumn(withIdentifier: ListColumn.location.id)?.isHidden = search == nil
        tableView.reloadData()
    }

    func refreshColumns() {
        guard let path = content.path, let view = session.view(tab: tabId) else { return }
        columnBrowser.show(location: path, conn: conn, view: view)
        updateColumnPreview()
    }

    /// The column right of the focused one: the selected folder's contents,
    /// or a preview of the selected file.
    func updateColumnPreview() {
        guard let view = session.view(tab: tabId) else { return }
        let picked = selection
        var folder: String?
        var file: (String, Row)?
        if picked.count == 1, let index = picked.first, let row = content.row(at: index),
           let path = content.path(at: index)
        {
            let isPackage = !isRemote && row.isDir && NSWorkspace.shared.isFilePackage(atPath: path)
            if row.isDir, !isPackage {
                folder = path
            } else if !(view.layout == .columns && previewPaneShown) {
                file = (path, row)
            }
        }
        columnBrowser.selectionChanged(
            folder: folder, file: file.map { (path: $0.0, row: $0.1) }, view: view, remote: isRemote)
    }

    /// The preview pane, when open, already shows the selected file.
    private var previewPaneShown: Bool {
        (view.window?.windowController as? MainWindowController)?.isPreviewShown ?? false
    }

    /// Clicked in a side column: that folder goes in focus with the item
    /// selected (a folder then opens to its right).
    func pickInColumn(folder: String, name: String) {
        if folder == content.path {
            if let row = content.snapshot?.indexOf(name: name) { setSelection([content.shownIndex(ofMain: Int(row))]) }
            return
        }
        session.navigate(tab: tabId, path: folder)
        reveal(names: [name], renameAfter: false)
    }

    func widestVisibleName() -> CGFloat {
        let font = NSFont.systemFont(ofSize: CGFloat(AppEnvironment.shared.settings.listTextSize))
        return (0..<min(content.count, 2000)).compactMap { content.row(at: $0)?.name }
            .map { ($0 as NSString).size(withAttributes: [.font: font]).width }.max() ?? 0
    }
}

extension FileListViewController {
    /// The right-click menu of an item in a side column of Column view.
    func sideColumnMenu(path: String, row: Row) -> NSMenu {
        let menu = NSMenu()
        let folder = (path as NSString).deletingLastPathComponent
        let open = NSMenuItem(title: "Open", action: #selector(openFromSideColumn(_:)), keyEquivalent: "")
        open.representedObject = [folder, row.name, row.isDir ? "dir" : "file"]
        open.target = self
        menu.addItem(open)
        if row.isDir {
            let tab = NSMenuItem(title: "Open in New Tab", action: #selector(openSideInNewTab(_:)), keyEquivalent: "")
            tab.representedObject = path
            tab.target = self
            menu.addItem(tab)
            let other = NSMenuItem(title: "Open in Other Pane", action: #selector(openSideInOtherPane(_:)), keyEquivalent: "")
            other.representedObject = path
            other.target = self
            menu.addItem(other)
        } else if !isRemote {
            menu.addItem(openWithItem(for: path))
        }
        menu.addItem(.separator())
        menu.addItem(copyPathItem(for: [path]))
        if !isRemote {
            let reveal = NSMenuItem(title: "Reveal in Finder", action: #selector(revealInFinder(_:)), keyEquivalent: "")
            reveal.representedObject = [path]
            reveal.target = self
            menu.addItem(reveal)
        }
        return menu
    }

    @objc func openFromSideColumn(_ sender: NSMenuItem) {
        guard let info = sender.representedObject as? [String], info.count == 3 else { return }
        if info[2] == "dir" {
            session.navigate(tab: tabId, path: (info[0] as NSString).appendingPathComponent(info[1]))
        } else if !isRemote {
            NSWorkspace.shared.open(URL(fileURLWithPath: (info[0] as NSString).appendingPathComponent(info[1])))
        } else {
            pickInColumn(folder: info[0], name: info[1])
        }
    }

    @objc func openSideInNewTab(_ sender: NSMenuItem) {
        if let path = sender.representedObject as? String { onOpenInNewTab?(path) }
    }

    @objc func openSideInOtherPane(_ sender: NSMenuItem) {
        if let path = sender.representedObject as? String { onOpenInOtherPane?(path) }
    }
}
