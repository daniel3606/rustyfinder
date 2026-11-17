import AppKit

/// The list view of one tab. Owns the table; the window owns the tab.
final class FileListViewController: NSViewController {
    let session: WindowSession
    let tabId: UInt64
    let content = TabContent()

    /// The table took focus: its pane should become the active one.
    var onActivate: (() -> Void)?
    /// Selection, contents or location changed: refresh status line, title, toolbar.
    var onChange: (() -> Void)?
    /// Open `path` in the other pane (or this one in single-pane mode).
    var onOpenInOtherPane: ((String) -> Void)?
    /// Open `path` in a new tab of this pane.
    var onOpenInNewTab: ((String) -> Void)?
    /// Tab pressed in the list: move focus to the other pane.
    var onSwitchPane: (() -> Void)?
    /// Esc pressed in the list with nothing else to cancel: Quick Open.
    var onQuickOpen: (() -> Void)?
    /// Control-Tab / Control-Shift-Tab: step through this pane's tabs.
    var onCycleTab: ((Int) -> Void)?

    let tableView = FileTableView()
    private let scrollView = NSScrollView()
    let gridView = FileGridView()
    let columnBrowser = ColumnBrowser()
    /// The list's header, put back when leaving Column view.
    private var savedHeader: NSTableHeaderView?
    var tableScrollView: NSScrollView { scrollView }
    var gridScrollView: NSScrollView { gridScroll }

    func stashHeader() {
        savedHeader = tableView.headerView
        tableView.headerView = nil
    }

    func restoreHeader() {
        if let savedHeader { tableView.headerView = savedHeader }
        savedHeader = nil
    }
    private let gridScroll = NSScrollView()
    private let gridLayout = NSCollectionViewFlowLayout()
    /// Layout currently on screen (the tab's setting lives in the core).
    private(set) var layout: ViewLayout = .list
    let banner = ErrorBanner()
    private var bannerHeight: NSLayoutConstraint!

    private var isSyncingSort = false
    private var typeSelectBuffer = ""
    private var typeSelectTime: TimeInterval = 0
    /// Path of a load that failed and can be retried from the banner.
    private var failedPath: String?

    /// A confirmation sheet this list put up (kept for scripted checks).
    var pendingConfirmation: NSAlert?
    /// Row being renamed in place, and the name it had.
    private var renaming: (row: Int, original: String, path: String)?
    /// A listing that arrived during a rename; applied once editing ends so
    /// the text field is not pulled out from under the user.
    private var deferred: (SnapshotHandle, PaneInfo)?
    /// Items to select (and maybe rename) as soon as they show up.
    private var pendingReveal: (names: [String], rename: Bool, deadline: TimeInterval)?

    var isRenaming: Bool { renaming != nil }
    /// Spring-loading: the folder a drag is paused over, and its countdown.
    var springTarget: String?
    var springTimer: Timer?
    /// A rename waiting to see whether the click becomes a double-click.
    private var pendingSlowRename: DispatchWorkItem?

    /// A slow second click on a selected name renames it, unless a second
    /// click (a double-click) or another action comes first.
    func slowClick(tableRow: Int) {
        guard let item = content.item(atTableRow: tableRow), !isArchive else { return }
        pendingSlowRename?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self, self.selection == [item], self.renaming == nil else { return }
            self.beginRename(row: item)
        }
        pendingSlowRename = work
        DispatchQueue.main.asyncAfter(deadline: .now() + NSEvent.doubleClickInterval + 0.05, execute: work)
    }
    /// The extension the item being renamed hides; added back on commit.
    private var renamingHiddenExtension: String?

    /// Connection the folder is on; 0 is the local disk.
    var conn: UInt64 { content.info?.conn ?? 0 }
    /// True while the tab shows a server. Things that need a file on this
    /// Mac (Quick Look, drags out, Finder, thumbnails) are off there.
    var isRemote: Bool { conn != 0 }

    /// Inside an archive: everything can be read, nothing changed.
    var isArchive: Bool { ArchiveCenter.shared.isArchive(conn) }

    /// Asked to browse an archive (its path) as a folder.
    var onOpenArchive: ((String) -> Bool)?

    private static let tagDotsId = NSUserInterfaceItemIdentifier("tagDots")
    private var tagsPopover: NSPopover?

    /// Git status of the items listed, by name (only those not clean).
    private var scm: [String: ScmStatus] = [:]
    private var scmGeneration = 0

    /// The live name filter typed into the search field.
    private(set) var currentFilter = ""

    typealias Column = ListColumn
    lazy var headerMenuDelegate: HeaderMenuDelegate = {
        let d = HeaderMenuDelegate()
        d.owner = self
        return d
    }()

    init(session: WindowSession, tabId: UInt64) {
        self.session = session
        self.tabId = tabId
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    // MARK: View setup

    override func loadView() {
        let root = NSView()

        // Name, the two the app switches, the user's columns in their
        // order, then the rest hidden so the header menu can show them.
        let visible = Column.saved
        let order = [Column.name, .scm, .location] + visible
            + Column.userToggled.filter { !visible.contains($0) }
        for column in order {
            let col = NSTableColumn(identifier: column.id)
            col.title = column.title
            col.width = column.width
            col.minWidth = column == .name ? 120 : 40
            if column.sort != nil {
                col.sortDescriptorPrototype = NSSortDescriptor(key: column.rawValue, ascending: column.ascendingFirst)
            }
            // "Where" only means something for search results, "SCM" only
            // inside a repository with the status switched on.
            col.isHidden = column != .name && !visible.contains(column)
            if column.rightAligned { col.headerCell.alignment = .right }
            col.headerToolTip = column.title
            tableView.addTableColumn(col)
        }
        installHeaderMenu()
        tableView.style = .inset
        tableView.rowHeight = 22
        tableView.allowsMultipleSelection = true
        tableView.allowsColumnReordering = true
        tableView.allowsTypeSelect = false  // done in Rust; AppKit's scans every row
        tableView.columnAutoresizingStyle = .firstColumnOnlyAutoresizingStyle
        tableView.dataSource = self
        tableView.delegate = self
        tableView.keyHandler = self
        tableView.target = self
        tableView.doubleAction = #selector(openSelection(_:))
        tableView.onDragExit = { [weak self] in self?.springCancel() }
        gridView.onDragExit = { [weak self] in self?.springCancel() }
        tableView.onSlowClick = { [weak self] tableRow in self?.slowClick(tableRow: tableRow) }
        tableView.onOtherClick = { [weak self] in self?.pendingSlowRename?.cancel() }
        tableView.setAccessibilityLabel("Files")
        installContextMenu()
        installDragAndDrop()
        applySettings()

        scrollView.documentView = tableView
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = true
        scrollView.autohidesScrollers = true
        scrollView.drawsBackground = true
        scrollView.translatesAutoresizingMaskIntoConstraints = false

        gridLayout.minimumInteritemSpacing = 8
        gridLayout.minimumLineSpacing = 8
        gridLayout.sectionInset = NSEdgeInsets(top: 10, left: 12, bottom: 10, right: 12)
        gridView.collectionViewLayout = gridLayout
        gridView.register(FileGridItem.self, forItemWithIdentifier: FileGridItem.id)
        gridView.register(
            GridGroupHeader.self, forSupplementaryViewOfKind: NSCollectionView.elementKindSectionHeader,
            withIdentifier: GridGroupHeader.id)
        gridView.indexOf = { [weak self] path in self?.content.item(atGridPath: path) ?? path.item }
        gridView.isSelectable = true
        gridView.allowsMultipleSelection = true
        gridView.allowsEmptySelection = true
        gridView.backgroundColors = [.clear]
        gridView.dataSource = self
        gridView.delegate = self
        gridView.keyHandler = self
        gridView.onDoubleClick = { [weak self] in self?.openSelection(nil) }
        gridView.onFocusChange = { [weak self] in self?.refreshGridHighlight() }
        gridView.setAccessibilityLabel("Files")
        gridScroll.documentView = gridView
        gridScroll.hasVerticalScroller = true
        gridScroll.autohidesScrollers = true
        gridScroll.translatesAutoresizingMaskIntoConstraints = false
        gridScroll.isHidden = true
        applySizes()

        banner.translatesAutoresizingMaskIntoConstraints = false
        banner.isHidden = true
        banner.onRetry = { [weak self] in
            guard let self, let path = self.failedPath else { return }
            self.session.navigate(tab: self.tabId, path: path)
        }
        banner.onDismiss = { [weak self] in self?.hideBanner() }

        columnBrowser.translatesAutoresizingMaskIntoConstraints = false
        columnBrowser.isHidden = true
        columnBrowser.owner = self
        columnBrowser.onPick = { [weak self] folder, name in self?.pickInColumn(folder: folder, name: name) }
        columnBrowser.makeMenu = { [weak self] path, row in self?.sideColumnMenu(path: path, row: row) ?? NSMenu() }
        root.addSubview(banner)
        root.addSubview(scrollView)
        root.addSubview(gridScroll)
        root.addSubview(columnBrowser)
        bannerHeight = banner.heightAnchor.constraint(equalToConstant: 0)
        NSLayoutConstraint.activate([
            banner.topAnchor.constraint(equalTo: root.topAnchor),
            banner.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            banner.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            bannerHeight,
        ])
        pinBelowBanner(scrollView, in: root)
        pinBelowBanner(gridScroll, in: root)
        pinBelowBanner(columnBrowser, in: root)
        view = root

        NotificationCenter.default.addObserver(
            self, selector: #selector(settingsChanged), name: .rustyfinderSettingsChanged, object: nil)
        // Operations inside an expanded folder don't reach this folder's
        // watcher: read the expanded ones again when one finishes.
        NotificationCenter.default.addObserver(
            forName: .rustyfinderActivitiesChanged, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, self.content.hasExpansions else { return }
                self.relistExpandedSoon()
            }
        }
        NotificationCenter.default.addObserver(
            forName: .rustyfinderFolderSizeChanged, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.folderSizesChanged() }
        }
        NotificationCenter.default.addObserver(
            forName: .rustyfinderListColumnsChanged, object: nil, queue: .main
        ) { [weak self] note in
            MainActor.assumeIsolated {
                guard let self, note.object as AnyObject? !== self else { return }
                self.adoptSavedColumns()
            }
        }
    }

    @objc private func settingsChanged() {
        applySettings()
        applySizes()
        if layout == .columns { columnBrowser.reloadForTextSize() }
        gridView.reloadData()
        tableView.noteHeightOfRows(withIndexesChanged: IndexSet(integersIn: 0..<content.tableCount))
        // Redraw what is on screen only. A full reloadData would drop the
        // selection, and a new snapshot may be on its way anyway.
        let visible = tableView.rows(in: tableView.visibleRect)
        guard visible.length > 0 else { return }
        tableView.reloadData(
            forRowIndexes: IndexSet(integersIn: visible.location..<(visible.location + visible.length)),
            columnIndexes: IndexSet(integersIn: 0..<tableView.numberOfColumns))
    }

    private func applySettings() {
        tableView.usesAlternatingRowBackgroundColors = AppEnvironment.shared.settings.alternatingRows
        tableView.columnAutoresizingStyle = ViewDisplay.resizeNameColumn ? .firstColumnOnlyAutoresizingStyle : .noColumnAutoresizing
        gridLayout.minimumInteritemSpacing = ViewDisplay.gridSpacing
        gridLayout.minimumLineSpacing = ViewDisplay.gridSpacing
    }

    func focus() {
        view.window?.makeFirstResponder(primaryView)
    }

    // MARK: One selection, two layouts

    /// The view that shows the files and takes the keyboard: the list or
    /// the icon grid.
    var primaryView: NSView { layout == .icons ? gridView : tableView }

    /// Indexes of the selected items, whatever the layout.
    var selection: IndexSet {
        layout == .icons
            ? IndexSet(gridView.selectionIndexPaths.map(content.item(atGridPath:)))
            : content.items(atTableRows: tableView.selectedRowIndexes)
    }

    func setSelection(_ indexes: IndexSet) {
        if layout == .icons {
            gridView.selectionIndexPaths = Set(indexes.map(content.gridPath(ofItem:)))
            // The grid does not report selections made in code.
            selectionDidChange()
        } else {
            tableView.selectRowIndexes(content.tableRows(ofItems: indexes), byExtendingSelection: false)
        }
    }

    /// Scroll only as far as needed to show the item; never re-centre it.
    func scrollToVisible(_ index: Int) {
        guard index >= 0, index < content.count else { return }
        if layout == .icons {
            gridView.scrollToItems(at: [content.gridPath(ofItem: index)], scrollPosition: .nearestHorizontalEdge)
            gridView.scrollToItems(at: [content.gridPath(ofItem: index)], scrollPosition: .nearestVerticalEdge)
        } else {
            tableView.scrollRowToVisible(content.tableRow(ofItem: index))
        }
    }

    /// Item under the pointer at the last click (-1 for empty space).
    var clickedIndex: Int {
        layout == .icons ? gridView.clickedIndex : (content.item(atTableRow: tableView.clickedRow) ?? -1)
    }

    func reloadViews() {
        tableView.reloadData()
        gridView.reloadData()
    }

    /// Show the layout the tab is set to, keeping selection and focus.
    private func applyLayout() {
        let target = session.view(tab: tabId)?.layout ?? .list
        guard target != layout else { return }
        let kept = selection
        let hadFocus = view.window?.firstResponder === primaryView
        let old = layout
        // Expanded folders belong to List view.
        if target != .list, content.hasExpansions { content.collapseAll() }
        layout = target
        // The list table is also the focused column of Column view.
        if target == .columns { enterColumns() }
        if old == .columns { leaveColumns() }
        // Show the new view and give it the keyboard before hiding the old
        // one. Hiding the view that has focus first would let AppKit pass
        // focus on to the next view in line: the other pane.
        let shown: NSView = target == .icons ? gridScroll : (target == .columns ? columnBrowser : scrollView)
        shown.isHidden = false
        scrollView.isHidden = target == .icons
        setSelection(kept)
        if hadFocus { focus() }
        for other in [gridScroll, columnBrowser] as [NSView] where other !== shown { other.isHidden = true }
        if let first = kept.first { scrollToVisible(first) }
        if target == .columns { refreshColumns() }
    }

    @objc func viewAsIcons(_ sender: Any?) { leaveColumnsAtDeepest { $0.layout = .icons } }
    @objc func viewAsList(_ sender: Any?) { leaveColumnsAtDeepest { $0.layout = .list } }
    @objc func viewAsColumns(_ sender: Any?) { updateView { $0.layout = .columns } }

    /// VW-06: leaving Column view shows the deepest folder whose contents
    /// were on screen, which is the selected folder's when its column is open.
    private func leaveColumnsAtDeepest(_ change: (inout ViewOptions) -> Void) {
        let deeper = layout == .columns ? columnBrowser.previewFolder : nil
        // View first: changing the view cancels a listing still under way,
        // so navigating first could lose the navigation.
        updateView(change)
        if let deeper { session.navigate(tab: tabId, path: deeper) }
    }

    // MARK: Text and icon size

    private static let textSizes: [CGFloat] = [11, 12, 13, 14, 16, 18, 20]
    private static let iconSizes: [CGFloat] = [32, 48, 64, 96, 128, 192, 256]

    /// Each layout keeps its own size (stored with the other settings).
    private var textSize: CGFloat { CGFloat(AppEnvironment.shared.settings.listTextSize) }
    private var iconSize: CGFloat { CGFloat(AppEnvironment.shared.settings.iconSize) }

    private func applySizes() {
        tableView.rowHeight = textSize + 9
        let side = iconSize
        let info: CGFloat = ViewDisplay.iconItemInfo ? 14 : 0
        gridLayout.itemSize = NSSize(width: max(side + 28, 96), height: side + 46 + info)
    }

    /// Cmd-plus / Cmd-minus: text size in the list, icon size in the grid.
    @objc func makeBigger(_ sender: Any?) { stepSize(by: 1) }
    @objc func makeSmaller(_ sender: Any?) { stepSize(by: -1) }

    private func stepSize(by delta: Int) {
        let icons = layout == .icons
        let sizes = icons ? Self.iconSizes : Self.textSizes
        let current = icons ? iconSize : textSize
        let index = sizes.firstIndex(where: { $0 >= current }) ?? sizes.count - 1
        let next = sizes[min(max(index + delta, 0), sizes.count - 1)]
        guard next != current else { return }
        // Saving posts a settings change, so every tab of every window follows.
        AppEnvironment.shared.update { settings in
            if icons { settings.iconSize = UInt32(next) } else { settings.listTextSize = UInt32(next) }
        }
    }

    private func refreshGridHighlight() {
        let active = view.window?.firstResponder === gridView
        gridView.visibleItems().forEach { ($0 as? FileGridItem)?.paneIsActive = active }
    }

    /// Selection changed in either layout.
    private func selectionDidChange() {
        // Remembered so that coming back to this folder restores the cursor.
        let name = selection.first.flatMap { content.row(at: $0)?.name }
        session.rememberSelection(tab: tabId, name: name)
        if layout == .columns { updateColumnPreview() }
        quickLookSelectionChanged()
        onChange?()
    }

    // MARK: Model updates (called by the window)

    func apply(_ snapshot: SnapshotHandle, _ info: PaneInfo) {
        // A background tab can receive its listing before it is ever shown.
        loadViewIfNeeded()
        if renaming != nil, info.reason != .navigated {
            deferred = (snapshot, info)
            return
        }
        if renaming != nil { cancelRename() }
        // The selection, by path, so it survives rows moving or expanded
        // folders being re-read.
        let keptPaths = Set(selection.compactMap { content.path(at: $0) })
        content.apply(snapshot, info)
        if content.hasExpansions, info.reason != .navigated { relistExpanded() }
        if info.reason != .viewChanged {
            SpotlightColumns.shared.clear()
            FolderSizes.shared.forget(inside: info.path)
        }
        tableView.tableColumn(withIdentifier: Column.location.id)?.isHidden = info.search == nil
        // A filter belongs to the folder it was typed in.
        if info.reason == .navigated, info.search == nil, !currentFilter.isEmpty {
            currentFilter = ""
            updateView { $0.filter = "" }
        }
        hideBanner()
        applyLayout()
        switch info.reason {
        case .navigated:
            reloadViews()
            setSelection([])
            if content.count > 0 {
                scrollToVisible(0)
                let target = info.selectName.flatMap { snapshot.indexOf(name: $0) }.map { content.shownIndex(ofMain: Int($0)) } ?? 0
                select(row: target)
            }
        case .reloaded, .viewChanged:
            // Same folder: carry the selection over by name, since rows moved.
            reloadViews()
            let rows = content.indices(ofPaths: keptPaths)
            setSelection(rows)
            if info.reason == .viewChanged, let first = rows.first {
                scrollToVisible(first)
            }
            // Results arriving, or a filter or sort that left nothing
            // selected: keep a cursor, on the first item.
            if rows.isEmpty, content.count > 0, info.search != nil || info.reason == .viewChanged {
                select(row: 0)
            }
        }
        if layout == .columns { refreshColumns() }
        if ViewDisplay.resizeColumnsToFit, layout == .list, info.reason != .viewChanged { fitAllColumns() }
        syncSortIndicator()
        applyPendingReveal()
        NotificationCenter.default.post(name: .rustyfinderTabViewChanged, object: self)
        refreshScm()
        onChange?()
    }

    // MARK: Tags

    /// Toggle a tag on the selection: off if every item has it, else on.
    func toggleTag(_ name: String) {
        guard !isRemote else { return }
        recordTags(TagStore.toggle(name, on: selectedPaths))
        tagsChanged()
    }

    /// Menu item for one colour tag; its title is the tag's name.
    @objc func toggleColorTag(_ sender: NSMenuItem) {
        toggleTag(sender.representedObject as? String ?? sender.title)
    }

    /// "Tags…": edit the selection's tags in a pop-over at the selection.
    @objc func editTags(_ sender: Any?) {
        guard !isRemote, let first = selection.first else { return }
        let controller = TagsPopoverController(paths: selectedPaths)
        controller.onChange = { [weak self] in self?.tagsChanged() }
        controller.onFinish = { [weak self] changes in self?.recordTags(changes) }
        let popover = NSPopover()
        popover.behavior = .transient
        popover.contentViewController = controller
        tagsPopover = popover
        scrollToVisible(first)
        let anchor = layout == .icons ? gridView.bounds : tableView.rect(ofRow: content.tableRow(ofItem: first))
        popover.show(relativeTo: anchor, of: primaryView, preferredEdge: .maxY)
    }

    private func tagsChanged() {
        tableView.reloadData(
            forRowIndexes: content.tableRows(ofItems: selection), columnIndexes: IndexSet(integer: max(0, tableView.column(withIdentifier: Column.name.id))))
        onChange?()
    }

    /// Tag edits are undoable like file operations (file-ops §8).
    private func recordTags(_ changes: [TagChange]) {
        guard !changes.isEmpty else { return }
        AppEnvironment.shared.core.recordTagChange(window: session.id(), changes: changes)
    }

    /// Apply an edited list of common tags, as the pop-over does.
    func debugEditTags(_ edited: [String]) {
        recordTags(TagStore.apply(edited, to: selectedPaths))
        tagsChanged()
    }

    // MARK: Source control status

    private static func scmLetter(_ status: ScmStatus) -> String {
        switch status {
        case .modified: return "M"
        case .added: return "A"
        case .untracked: return "?"
        case .ignored: return "!"
        case .conflicted: return "U"
        }
    }

    private static func scmName(_ status: ScmStatus) -> String {
        switch status {
        case .modified: return "Modified"
        case .added: return "Added"
        case .untracked: return "Untracked"
        case .ignored: return "Ignored"
        case .conflicted: return "Conflicted"
        }
    }

    /// Ask git about this folder (off the main thread) and show the answer
    /// in the SCM column. Outside a repository, on a server, or with the
    /// option off, the column is simply not there.
    func refreshScm() {
        loadViewIfNeeded()
        scmGeneration += 1
        let generation = scmGeneration
        let column = tableView.tableColumn(withIdentifier: Column.scm.id)
        guard Prefs.bool("view.scmStatus"), !isRemote, search == nil, let path = content.path else {
            scm = [:]
            column?.isHidden = true
            refreshGridScmBadges()
            return
        }
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let entries = scmStatus(path: path)
            DispatchQueue.main.async {
                guard let self, self.scmGeneration == generation else { return }
                self.scm = Dictionary((entries ?? []).map { ($0.name, $0.status) }, uniquingKeysWith: { a, _ in a })
                column?.isHidden = entries == nil
                self.refreshGridScmBadges()
                self.tableView.reloadData(
                    forRowIndexes: IndexSet(integersIn: 0..<self.content.tableCount),
                    columnIndexes: IndexSet(integer: self.tableView.column(withIdentifier: Column.scm.id)))
            }
        }
    }

    /// Badges on the icons on screen follow the git status.
    private func refreshGridScmBadges() {
        for indexPath in gridView.indexPathsForVisibleItems() {
            guard let cell = gridView.item(at: indexPath) as? FileGridItem,
                  let row = content.row(at: indexPath.item)
            else { continue }
            cell.setScmBadge(scm[row.name])
        }
    }

    /// Names of the icons on screen that show a thumbnail.
    var debugGridThumbnails: String {
        gridView.indexPathsForVisibleItems().sorted().compactMap { indexPath -> String? in
            guard let cell = gridView.item(at: indexPath) as? FileGridItem, cell.hasThumbnail,
                  let row = content.row(at: content.item(atGridPath: indexPath))
            else { return nil }
            return row.name
        }.joined(separator: ",")
    }

    /// Git status letter of each icon on screen that has a badge.
    var debugGridScmBadges: String {
        gridView.indexPathsForVisibleItems().sorted().compactMap { indexPath -> String? in
            guard let cell = gridView.item(at: indexPath) as? FileGridItem, let letter = cell.scmBadgeLetter,
                  let row = content.row(at: indexPath.item)
            else { return nil }
            return "\(row.name)=\(letter)"
        }.joined(separator: ",")
    }

    // MARK: Revealing new items

    /// Select these items, scrolling them into view. They may not be listed
    /// yet (the folder refreshes a moment after the operation), so the
    /// request is kept briefly and applied when they appear.
    func reveal(names: [String], renameAfter: Bool) {
        pendingReveal = (names, renameAfter, ProcessInfo.processInfo.systemUptime + 3)
        applyPendingReveal()
    }

    private func applyPendingReveal() {
        guard let pending = pendingReveal, let snapshot = content.snapshot else { return }
        if ProcessInfo.processInfo.systemUptime > pending.deadline {
            pendingReveal = nil
            return
        }
        let rows = snapshot.indicesOf(names: pending.names).map { content.shownIndex(ofMain: Int($0)) }
        guard rows.count == pending.names.count, let first = rows.first else { return }
        pendingReveal = nil
        setSelection(IndexSet(rows))
        scrollToVisible(first)
        if pending.rename, rows.count == 1 {
            beginRename(row: first)
        }
    }

    // MARK: Inline rename

    private var nameColumnIndex: Int { tableView.column(withIdentifier: Column.name.id) }

    func beginRename(row: Int) {
        guard renaming == nil, let entry = content.row(at: row), let path = content.path(at: row),
              let field = nameField(at: row, make: true)
        else { return }
        scrollToVisible(row)
        let local = isRemote ? nil : path
        let shown = RowPresenter.shared.displayName(entry, path: local)
        renamingHiddenExtension = RowPresenter.shared.hiddenExtension(entry, path: local)
        renaming = (row, shown, path)
        field.isEditable = true
        field.delegate = self
        field.drawsBackground = true
        field.backgroundColor = .textBackgroundColor
        field.textColor = .labelColor
        view.window?.makeFirstResponder(field)
        // Pre-select the base name so typing keeps the extension.
        let stem = entry.isDir || renamingHiddenExtension != nil ? shown : ((shown as NSString).deletingPathExtension)
        let length = stem.isEmpty ? (shown as NSString).length : (stem as NSString).length
        field.currentEditor()?.selectedRange = NSRange(location: 0, length: length)
        onChange?()
    }

    /// The text field showing an item's name, in the list or the grid.
    private func nameField(at index: Int, make: Bool) -> NSTextField? {
        if layout == .icons {
            let path = content.gridPath(ofItem: index)
            if make { gridView.scrollToItems(at: [path], scrollPosition: .nearestVerticalEdge) }
            return gridView.item(at: path)?.textField
        }
        guard nameColumnIndex >= 0 else { return nil }
        let cell = tableView.view(atColumn: nameColumnIndex, row: content.tableRow(ofItem: index), makeIfNecessary: make) as? NSTableCellView
        return cell?.textField
    }

    private func renameField() -> NSTextField? {
        guard let renaming else { return nil }
        return nameField(at: renaming.row, make: false)
    }

    /// Leave edit mode. `commit` submits the new name if it changed.
    private func endRename(commit: Bool) {
        guard let renaming else { return }
        let field = renameField()
        let typed = field?.stringValue ?? renaming.original
        self.renaming = nil
        field?.isEditable = false
        field?.delegate = nil
        field?.drawsBackground = false
        field?.stringValue = renaming.original
        if view.window?.firstResponder !== primaryView {
            view.window?.makeFirstResponder(primaryView)
        }
        if commit, typed != renaming.original, !typed.isEmpty {
            // A hidden extension stays on: the user never saw it to keep it.
            var newName = typed
            if let ext = renamingHiddenExtension, !typed.lowercased().hasSuffix("." + ext.lowercased()) {
                newName += "." + ext
            }
            let (path, window, conn) = (renaming.path, session.id(), conn)
            LocalPreflight.run([path], conn: conn, verb: "renamed", window: view.window) {
                ActivityCenter.shared.submit(.rename(path: path, newName: newName), window: window, src: conn, dst: conn)
            }
        }
        renamingHiddenExtension = nil
        if let (snapshot, info) = deferred {
            deferred = nil
            apply(snapshot, info)
        }
        onChange?()
    }

    func cancelRename() {
        if let editor = renameField()?.currentEditor() { editor.string = renaming?.original ?? "" }
        view.window?.makeFirstResponder(primaryView)
        endRename(commit: false)
    }

    /// A load failed. Shown inline, never as a modal; the next successful
    /// load clears it.
    func showFailure(path: String, error: RustyFinderError) {
        loadViewIfNeeded()
        failedPath = path
        banner.show(title: "“\((path as NSString).lastPathComponent)” can’t be opened.", detail: error.message)
        configureAccessAction(path: path, error: error)
        banner.isHidden = false
        bannerHeight.constant = 44
        onChange?()
    }

    private func hideBanner() {
        failedPath = nil
        banner.isHidden = true
        bannerHeight.constant = 0
    }

    var hasFailure: Bool { failedPath != nil }

    // MARK: Status line

    /// "N items" or "x of N selected, 12 MB", plus how many the view hides.
    /// Search results and a live filter say so instead.
    var statusText: String {
        guard let info = content.info else { return hasFailure ? "" : "Loading…" }
        let shown = content.count
        if let search = info.search {
            var text = "\(shown.formatted()) found"
            if !selection.isEmpty { text = "\(selection.count.formatted()) of \(text) selected" }
            if search.running { text += ", searching…" }
            if let note = search.note { text += " · \(note)" }
            return text
        }
        if !currentFilter.isEmpty {
            return "Filtered: \(shown.formatted())"
        }
        let items = shown == 1 ? "1 item" : "\(shown.formatted()) items"
        var text = items
        let selected = selection
        if !selected.isEmpty, let snapshot = content.snapshot {
            _ = snapshot
            let bytes = selectedBytes()
            text = "\(selected.count.formatted()) of \(items) selected"
            if bytes > 0 { text += ", \(RowPresenter.shared.bytes(bytes))" }
        }
        // Hidden or filtered out of the folder itself; expanded rows don't count.
        let filteredOut = Int(info.totalCount) - content.mainCount
        if filteredOut > 0 { text += ", \(filteredOut.formatted()) hidden" }
        return text
    }

    // MARK: Actions (reached through the responder chain)

    /// Folder to enter for a row, or nil if the row is a file or a package
    /// that should open in its application.
    private func folderPath(at index: Int) -> String? {
        guard let row = content.row(at: index), row.isDir, let path = content.path(at: index) else { return nil }
        // Bundles only exist as such on this Mac; on a server a folder
        // with a dot in its name is just a folder.
        if !isRemote, !AppEnvironment.shared.settings.treatPackagesAsFolders,
           NSWorkspace.shared.isFilePackage(atPath: path)
        {
            return nil
        }
        return path
    }

    @objc func openSelection(_ sender: Any?) {
        pendingSlowRename?.cancel()
        // A double-click on empty space reports clickedRow == -1 and must do nothing.
        if sender is NSTableView, clickedIndex < 0 { return }
        let rows = selection
        guard !rows.isEmpty else { return }

        if rows.count == 1, let index = rows.first, let folder = folderPath(at: index) {
            session.navigate(tab: tabId, path: folder)
            return
        }
        // An archive opens as a folder, if the setting says so.
        if rows.count == 1, let index = rows.first, let row = content.row(at: index), !row.isDir,
           AppEnvironment.shared.settings.treatArchivesAsFolders, isArchiveName(name: row.name),
           let path = content.path(at: index), onOpenArchive?(path) == true
        {
            return
        }
        for index in rows {
            // With several items selected, plain folders are skipped: there
            // is only one pane to show them in.
            guard folderPath(at: index) == nil, let path = content.path(at: index) else { continue }
            if isRemote {
                // Downloaded to a temporary copy, opened, and uploaded
                // again whenever that copy is saved.
                RemoteFiles.shared.open(connection: conn, remotePath: path, window: session.id())
            } else {
                openLocalFile(path)
            }
        }
    }

    @objc func openInNewTab(_ sender: Any?) {
        for index in selection {
            if let folder = folderPath(at: index) { onOpenInNewTab?(folder) }
        }
    }

    @objc func openInOtherPane(_ sender: Any?) {
        guard let index = selection.first, let folder = folderPath(at: index) else { return }
        onOpenInOtherPane?(folder)
    }

    @objc func deselectAll(_ sender: Any?) {
        setSelection([])
    }

    @objc func invertSelection(_ sender: Any?) {
        let all = IndexSet(integersIn: 0..<content.count)
        setSelection(all.subtracting(selection))
    }

    @objc func sortByMenu(_ sender: NSMenuItem) {
        guard let column = SortColumn.menuOrder[safe: sender.tag] else { return }
        updateView { $0.sort = column }
    }

    @objc func groupByMenu(_ sender: NSMenuItem) {
        guard let group = GroupBy.menuOrder[safe: sender.tag] else { return }
        updateView { $0.group = group }
    }

    @objc func toggleDescending(_ sender: Any?) {
        updateView { $0.ascending.toggle() }
    }

    @objc func toggleFoldersFirst(_ sender: Any?) {
        updateView { $0.foldersFirst.toggle() }
    }

    private func updateView(_ change: (inout ViewOptions) -> Void) {
        guard var view = session.view(tab: tabId) else { return }
        change(&view)
        session.setView(tab: tabId, view: view)
    }

    /// Show only names containing `text` ("" shows everything again).
    func setFilter(_ text: String) {
        guard text != currentFilter else { return }
        currentFilter = text
        updateView { $0.filter = text }
    }

    /// The search whose results this tab shows, if any.
    var search: SearchInfo? { content.info?.search }

    /// The selected rows with their paths, for the preview pane and Get Info.
    var inspectedSelection: [InspectedItem] {
        selection.compactMap { index in
            guard let row = content.row(at: index), let path = content.path(at: index) else { return nil }
            return InspectedItem(path: path, row: row, conn: conn)
        }
    }

    /// Paths of the selected rows.
    var selectedPaths: [String] {
        selection.compactMap { content.path(at: $0) }
    }

    /// The single selected folder, if exactly one folder is selected.
    var selectedFolder: String? {
        let rows = selection
        guard rows.count == 1, let index = rows.first else { return nil }
        return folderPath(at: index)
    }

    // MARK: Helpers

    private func syncSortIndicator() {
        guard let view = session.view(tab: tabId),
              let column = Column.allCases.first(where: { $0.sort == view.sort })
        else {
            isSyncingSort = true
            tableView.sortDescriptors = []
            isSyncingSort = false
            return
        }
        isSyncingSort = true
        tableView.sortDescriptors = [NSSortDescriptor(key: column.rawValue, ascending: view.ascending)]
        isSyncingSort = false
    }

    private func select(row: Int, scroll: Bool = true) {
        guard row >= 0, row < content.count else { return }
        setSelection([row])
        if scroll { scrollToVisible(row) }
    }

    /// True when no name is being typed (the type-select pause has passed).
    private var typeSelectExpired: Bool {
        typeSelectBuffer.isEmpty || ProcessInfo.processInfo.systemUptime - typeSelectTime > 1.0
    }

    private func typeSelect(_ characters: String) {
        guard let snapshot = content.snapshot else { return }
        let now = ProcessInfo.processInfo.systemUptime
        if now - typeSelectTime > 1.0 { typeSelectBuffer = "" }
        typeSelectTime = now
        typeSelectBuffer += characters
        if let row = snapshot.findPrefix(prefix: typeSelectBuffer, start: 0) {
            select(row: content.shownIndex(ofMain: Int(row)))
        }
    }
}

// MARK: - Table data

extension FileListViewController: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int { content.tableCount }

    func tableView(_ tableView: NSTableView, isGroupRow row: Int) -> Bool {
        content.group(atTableRow: row) != nil
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
        content.group(atTableRow: row) == nil
    }

    func tableView(_ tableView: NSTableView, selectionIndexesForProposedSelection proposed: IndexSet) -> IndexSet {
        // Select All, shift-click ranges: never the headers.
        content.groups.isEmpty ? proposed : proposed.filteredIndexSet { content.group(atTableRow: $0) == nil }
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row tableRow: Int) -> NSView? {
        if let group = content.group(atTableRow: tableRow) {
            return groupHeader(group)
        }
        guard let tableColumn, let column = Column(rawValue: tableColumn.identifier.rawValue),
              let index = content.item(atTableRow: tableRow), let row = content.row(at: index)
        else { return nil }

        let cell = makeCell(for: column)
        let presenter = RowPresenter.shared
        cell.textField?.font = column == .name
            ? ViewDisplay.font(size: textSize, bold: row.isDir && ViewDisplay.boldFolders)
            : ViewDisplay.font(size: textSize, monospacedDigits: true)
        switch column {
        case .name:
            // Hidden items are drawn lighter when shown.
            cell.textField?.stringValue = presenter.displayName(row, path: isRemote ? nil : content.path(at: index))
            cell.textField?.textColor = row.isHidden ? .tertiaryLabelColor : .labelColor
            cell.imageView?.image = listIcon(row, at: index, in: cell)
            cell.imageView?.alphaValue = row.isHidden ? 0.5 : 1
            // Colour tags show as dots after the name (this Mac only).
            configureDisclosure(cell, row: row, index: index)
            let dots = cell.subviews.first { $0.identifier == Self.tagDotsId } as? NSTextField
            let tags = isRemote ? [] : (content.path(at: index).map(TagStore.tags(at:)) ?? [])
            let marks = NSMutableAttributedString(attributedString: TagStore.dots(for: tags))
            if layout == .columns, row.isDir, !(row.extension.isEmpty == false && !isRemote
                && NSWorkspace.shared.isFilePackage(atPath: content.path(at: index) ?? "")) {
                // Column view marks folders with a chevron, as Finder does.
                marks.append(NSAttributedString(string: " ›", attributes: [
                    .foregroundColor: NSColor.tertiaryLabelColor, .font: NSFont.systemFont(ofSize: textSize, weight: .semibold),
                ]))
            }
            dots?.attributedStringValue = CloudItems.badge(
                after: marks, cloudOnly: !isRemote && CloudItems.isCloudOnly(row))
        case .scm:
            let status = scm[row.name]
            cell.textField?.stringValue = status.map(Self.scmLetter) ?? ""
            cell.textField?.textColor = status == .conflicted ? .systemRed
                : (status == .ignored ? .tertiaryLabelColor : .systemOrange)
            cell.toolTip = status.map(Self.scmName)
        case .location:
            let place = presenter.location(row)
            cell.textField?.stringValue = place.isEmpty ? (content.path ?? "").folderDisplayName : place
            cell.textField?.textColor = .secondaryLabelColor
        case .modified:
            cell.textField?.stringValue = presenter.date(row.modifiedMs)
        case .created:
            cell.textField?.stringValue = presenter.date(row.createdMs)
        case .added:
            // Read per row on screen: too slow to read for a whole folder.
            let added = isRemote ? nil : content.path(at: index).flatMap {
                try? URL(fileURLWithPath: $0).resourceValues(forKeys: [.addedToDirectoryDateKey]).addedToDirectoryDate
            }
            cell.textField?.stringValue = added.map { presenter.date(Int64($0.timeIntervalSince1970 * 1000)) } ?? ""
        case .size:
            cell.textField?.stringValue = sizeText(row, at: index)
        case .kind:
            cell.textField?.stringValue = presenter.kind(row)
        case .tags:
            let tags = isRemote ? [] : (content.path(at: index).map(TagStore.tags(at:)) ?? [])
            cell.textField?.stringValue = tags.joined(separator: ", ")
        case .owner:
            cell.textField?.stringValue = row.owner ?? ""
        case .group:
            cell.textField?.stringValue = row.group ?? ""
        case .permissions:
            cell.textField?.stringValue = presenter.permissions(row)
        default:
            // Spotlight columns: read lazily, only for items on this Mac.
            let path = isRemote ? nil : content.path(at: index)
            cell.textField?.stringValue = path.map { SpotlightColumns.shared.text(column, path: $0) } ?? ""
        }
        return cell
    }

    private func makeCell(for column: Column) -> NSTableCellView {
        if let reused = tableView.makeView(withIdentifier: column.id, owner: self) as? NSTableCellView {
            return reused
        }
        let cell = NSTableCellView()
        cell.identifier = column.id
        let text = NSTextField(labelWithString: "")
        text.lineBreakMode = column == .name ? .byTruncatingMiddle : .byTruncatingTail
        text.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(text)
        cell.textField = text

        if column == .name {
            let image = NSImageView()
            image.imageScaling = .scaleProportionallyUpOrDown
            image.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(image)
            cell.imageView = image
            let imageLeading = image.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 2)
            NSLayoutConstraint.activate([
                imageLeading,
                image.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
                image.widthAnchor.constraint(equalToConstant: 16),
                image.heightAnchor.constraint(equalToConstant: 16),
                text.leadingAnchor.constraint(equalTo: image.trailingAnchor, constant: 6),
            ])
            addDisclosure(to: cell, imageLeading: imageLeading)
            let dots = NSTextField(labelWithString: "")
            dots.identifier = Self.tagDotsId
            dots.translatesAutoresizingMaskIntoConstraints = false
            dots.setContentHuggingPriority(.required, for: .horizontal)
            dots.setContentCompressionResistancePriority(.required, for: .horizontal)
            cell.addSubview(dots)
            NSLayoutConstraint.activate([
                dots.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -4),
                dots.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
                text.trailingAnchor.constraint(lessThanOrEqualTo: dots.leadingAnchor, constant: -4),
                text.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
            return cell
        } else {
            text.textColor = .secondaryLabelColor
            text.font = .monospacedDigitSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)
            text.alignment = column.rightAligned ? .right : .left
            text.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 2).isActive = true
        }
        NSLayoutConstraint.activate([
            text.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -2),
            text.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        return cell
    }

    func tableView(_ tableView: NSTableView, sortDescriptorsDidChange oldDescriptors: [NSSortDescriptor]) {
        guard !isSyncingSort, let descriptor = tableView.sortDescriptors.first,
              let key = descriptor.key, let column = Column(rawValue: key)
        else { return }
        guard let sort = column.sort else { return }
        updateView { view in
            view.sort = sort
            view.ascending = descriptor.ascending
        }
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        selectionDidChange()
    }
}

// MARK: - Icon grid data

extension FileListViewController: NSCollectionViewDataSource, NSCollectionViewDelegate, NSCollectionViewDelegateFlowLayout {
    func numberOfSections(in collectionView: NSCollectionView) -> Int { content.gridSections }

    func collectionView(_ collectionView: NSCollectionView, numberOfItemsInSection section: Int) -> Int {
        content.gridCount(inSection: section)
    }

    func collectionView(
        _ collectionView: NSCollectionView, viewForSupplementaryElementOfKind kind: NSCollectionView.SupplementaryElementKind,
        at indexPath: IndexPath
    ) -> NSView {
        let header = collectionView.makeSupplementaryView(ofKind: kind, withIdentifier: GridGroupHeader.id, for: indexPath)
        if let header = header as? GridGroupHeader, content.groups.indices.contains(indexPath.section) {
            header.title = groupTitle(content.groups[indexPath.section])
        }
        return header
    }

    func collectionView(_ collectionView: NSCollectionView, layout: NSCollectionViewLayout, referenceSizeForHeaderInSection section: Int) -> NSSize {
        content.groups.isEmpty ? .zero : NSSize(width: collectionView.bounds.width, height: 26)
    }

    func collectionView(_ collectionView: NSCollectionView, itemForRepresentedObjectAt indexPath: IndexPath) -> NSCollectionViewItem {
        let item = collectionView.makeItem(withIdentifier: FileGridItem.id, for: indexPath)
        let index = content.item(atGridPath: indexPath)
        guard let cell = item as? FileGridItem, let row = content.row(at: index),
              let path = content.path(at: index)
        else { return item }
        let presenter = RowPresenter.shared
        let side = iconSize
        // Thumbnails stand in for generic icons where the system can make
        // one: on this Mac, and inside archives (from an extracted copy). A
        // cloud-only item gets none: making one would download it.
        let cloudOnly = !isRemote && CloudItems.isCloudOnly(row)
        let previews = ViewDisplay.iconPreviews && !cloudOnly
            && (!isRemote || isArchive || ViewDisplay.remoteThumbnailAllowed(row))
        let plain = (previews ? ThumbnailCache.shared.cached(path, row: row, side: side) : nil)
            ?? presenter.icon(row, path: isRemote ? nil : path)
        let image = cloudOnly ? CloudItems.badged(plain) : plain
        cell.configure(
            name: presenter.displayName(row, path: isRemote ? nil : path), path: path, image: image, hidden: row.isHidden,
            iconSize: side, fontSize: max(textSize - 1, 11))
        cell.setInfo(ViewDisplay.iconItemInfo ? itemInfo(row, path: path) : nil)
        if !isRemote { cell.setTagDots(TagStore.dots(for: TagStore.tags(at: path))) }
        cell.paneIsActive = view.window?.firstResponder === gridView
        cell.setScmBadge(scm[row.name])
        if previews, !row.isDir, ThumbnailCache.shared.cached(path, row: row, side: side) == nil {
            let scale = view.window?.backingScaleFactor ?? 2
            if isRemote {
                // Archives and (when allowed) servers: from a local copy.
                ThumbnailCache.shared.requestInArchive(
                    InspectedItem(path: path, row: row, conn: conn), side: side, scale: scale
                ) { [weak cell] thumbnail in
                    cell?.setThumbnail(thumbnail, for: path)
                }
            } else {
                ThumbnailCache.shared.request(path, row: row, side: side, scale: scale) { [weak cell] thumbnail in
                    cell?.setThumbnail(thumbnail, for: path)
                }
            }
        }
        return cell
    }

    func collectionView(_ collectionView: NSCollectionView, didSelectItemsAt indexPaths: Set<IndexPath>) {
        selectionDidChange()
    }

    func collectionView(_ collectionView: NSCollectionView, didDeselectItemsAt indexPaths: Set<IndexPath>) {
        selectionDidChange()
    }
}

// MARK: - Keyboard

extension FileListViewController: FileTableKeyHandler {
    private enum Key {
        static let tab: UInt16 = 48
        static let down: UInt16 = 125
        static let returnKey: UInt16 = 36
        static let enter: UInt16 = 76
        static let space: UInt16 = 49
        static let left: UInt16 = 123
        static let right: UInt16 = 124
        static let home: UInt16 = 115
        static let end: UInt16 = 119
        static let escape: UInt16 = 53
    }

    func fileTableDidBecomeFirstResponder(_ view: NSView) {
        onActivate?()
    }

    func fileTable(_ view: NSView, undo redo: Bool) {
        let core = AppEnvironment.shared.core
        _ = redo ? core.redo(window: session.id()) : core.undo(window: session.id())
    }

    func fileTable(_ view: NSView, undoTitle redo: Bool) -> String? {
        // Not behind a sheet: a prompt must be answered first.
        guard view.window?.attachedSheet == nil else { return nil }
        let core = AppEnvironment.shared.core
        return redo ? core.redoTitle(window: session.id()) : core.undoTitle(window: session.id())
    }

    func fileTable(_ view: NSView, handleKeyDown event: NSEvent) -> Bool {
        let mods = event.modifierFlags.intersection([.command, .control, .option, .shift])

        switch event.keyCode {
        case Key.tab where mods.isEmpty:
            onSwitchPane?()
            return true
        case Key.tab where mods == .control:
            onCycleTab?(1)
            return true
        case Key.tab where mods == [.control, .shift]:
            onCycleTab?(-1)
            return true
        case Key.down where mods == .command:
            openSelection(nil)
            return true
        case Key.returnKey where mods.isEmpty, Key.enter where mods.isEmpty:
            rename(nil)
            return true
        // In the grid, Right at the end of a row goes on to the next row
        // and Left at the start goes back to the previous one.
        case Key.left where mods.isEmpty && layout == .icons, Key.right where mods.isEmpty && layout == .icons:
            let step = event.keyCode == Key.right ? 1 : -1
            let from = selection.last ?? (step > 0 ? -1 : content.count)
            let target = min(max((step > 0 ? from : (selection.first ?? from)) + step, 0), content.count - 1)
            if target >= 0 {
                setSelection([target])
                scrollToVisible(target)
            }
            return true
        // List view: ⌘→ / ⌘← expand and collapse folders (⌥: all inside).
        case Key.right where (mods == .command || mods == [.command, .option]) && layout == .list:
            expandSelection(recursive: mods.contains(.option))
            return true
        case Key.left where (mods == .command || mods == [.command, .option]) && layout == .list:
            collapseSelection(recursive: mods.contains(.option))
            return true
        // Column view: → enters the selected folder, ← goes back up.
        case Key.right where mods.isEmpty && layout == .columns:
            if let folder = selectedFolder { session.navigate(tab: tabId, path: folder) }
            return true
        case Key.left where mods.isEmpty && layout == .columns:
            if content.path != "/" { session.goUp(tab: tabId) }
            return true
        case Key.home where mods.isEmpty, Key.end where mods.isEmpty:
            let target = event.keyCode == Key.home ? 0 : content.count - 1
            if target >= 0 {
                setSelection([target])
                scrollToVisible(target)
            }
            return true
        // Esc keeps its usual meaning wherever something can be cancelled
        // (a sheet, a rename, Quick Look all take it before the list
        // does). Only with the list itself in focus does it open Quick Open.
        case Key.escape where mods.isEmpty:
            if isQuickLookVisible {
                toggleQuickLook(nil)
            } else {
                onQuickOpen?()
            }
            return true
        // Space opens Quick Look, unless it is part of a name being typed.
        case Key.space where mods.isEmpty && typeSelectExpired:
            toggleQuickLook(nil)
            return true
        default:
            break
        }

        // Plain typing jumps to the first matching name. Option+letter is
        // left alone: those combinations may be bound to commands.
        guard mods.subtracting(.shift).isEmpty, let chars = event.characters,
              let scalar = chars.unicodeScalars.first
        else { return false }
        let isFunctionKey = (0xF700...0xF8FF).contains(scalar.value)
        let isControl = scalar.value < 0x20 || scalar.value == 0x7F
        if isFunctionKey || isControl { return false }
        typeSelect(chars)
        return true
    }
}

// MARK: - Menu state

extension FileListViewController: NSUserInterfaceValidations {
    func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
        let menuItem = item as? NSMenuItem
        let view = session.view(tab: tabId)
        let selected = selection.count
        // While a name is being edited, or a sheet is up, shortcuts must not
        // act on the selection behind it (Cmd-Delete never deletes there).
        let blocked = isRenaming || self.view.window?.attachedSheet != nil
        // Nothing inside an archive can be changed.
        let changes: [Selector] = [
            #selector(cut(_:)), #selector(duplicate(_:)), #selector(deleteImmediately(_:)),
            #selector(newFolderWithSelection(_:)), #selector(makeSymlink(_:)), #selector(rename(_:)),
            #selector(paste(_:)), #selector(moveItemsHere(_:)), #selector(newFolder(_:)),
            #selector(newFile(_:)), #selector(moveToTrash(_:)),
        ]
        if isArchive, let action = item.action, changes.contains(action) { return false }
        if item.action == #selector(toggleColorTag(_:)) || item.action == #selector(editTags(_:)) {
            // Tags exist on this Mac's disks only.
            if let name = menuItem?.representedObject as? String, !isRemote, selected > 0 {
                menuItem?.state = TagStore.common(of: selectedPaths).contains(name) ? .on : .off
            }
            return !blocked && !isRemote && selected > 0
        }
        switch item.action {
        case #selector(copy(_:)), #selector(cut(_:)), #selector(duplicate(_:)),
             #selector(deleteImmediately(_:)), #selector(newFolderWithSelection(_:)), #selector(makeSymlink(_:)):
            return !blocked && selected > 0
        case #selector(rename(_:)):
            return !blocked && selected == 1
        case #selector(paste(_:)), #selector(moveItemsHere(_:)):
            return !blocked && content.path != nil && !FileClipboard.paths().isEmpty
        case #selector(newFolder(_:)), #selector(newFile(_:)):
            return !blocked && content.path != nil
        case #selector(openSelection(_:)):
            return selection.count > 0
        case #selector(toggleQuickLook(_:)), #selector(quickLookSlideshow(_:)):
            return isQuickLookVisible || selection.count > 0
        case #selector(editSelection(_:)):
            return !blocked && !isArchive && !selectedFiles.isEmpty
        case #selector(printSelection(_:)):
            return !blocked && !isRemote && !isArchive && !selectedFiles.isEmpty
        case #selector(shareSelection(_:)):
            return !blocked && !isRemote && !isArchive && selected > 0
        case #selector(makeAlias(_:)):
            return !blocked && selected > 0 && !isRemote && !isArchive
        case #selector(deleteAppWithRelatedFiles(_:)):
            return !blocked && !isRemote && !isArchive && selectedPaths.contains(where: AppDeleter.isApp)
        case #selector(moveToTrash(_:)):
            // A server has no Trash: the command deletes, after asking.
            menuItem?.title = isRemote ? "Delete…" : "Move to Trash"
            return !blocked && selected > 0
        case #selector(openInNewTab(_:)):
            return selection.contains { folderPath(at: $0) != nil }
        case #selector(openInOtherPane(_:)):
            return selectedFolder != nil
        case #selector(deselectAll(_:)):
            return selection.count > 0
        case #selector(toggleFoldersFirst(_:)):
            menuItem?.state = view?.foldersFirst == true ? .on : .off
            return true
        // A checkmark on the current layout only.
        case #selector(viewAsIcons(_:)):
            menuItem?.state = view?.layout == .icons ? .on : .off
            return true
        case #selector(viewAsList(_:)):
            menuItem?.state = view?.layout == .list ? .on : .off
            return true
        case #selector(viewAsColumns(_:)):
            menuItem?.state = view?.layout == .columns ? .on : .off
            return true
        case #selector(calculateFolderSize(_:)):
            return !isRemote && hasSelectedFolder
        case #selector(calculateAllFolderSizes(_:)):
            return !isRemote && hasAnyFolder
        case #selector(toggleFoldersByName(_:)):
            menuItem?.state = view?.foldersByName == true ? .on : .off
            return view?.foldersFirst == true
        case #selector(groupByMenu(_:)):
            if let menuItem, let group = GroupBy.menuOrder[safe: menuItem.tag] {
                menuItem.state = group == view?.group ? .on : .off
            }
            return true
        case #selector(toggleDescending(_:)):
            menuItem?.state = view?.ascending == false ? .on : .off
            return true
        case #selector(sortByMenu(_:)):
            if let menuItem, let column = SortColumn.menuOrder[safe: menuItem.tag] {
                menuItem.state = column == view?.sort ? .on : .off
            }
            return true
        default:
            return true
        }
    }
}

// MARK: - Scripted checks

extension FileListViewController {
    private func debugNameFont(_ row: Int) -> String {
        guard let cell = tableView(tableView, viewFor: tableView.tableColumn(withIdentifier: Column.name.id), row: content.tableRow(ofItem: row)) as? NSTableCellView,
              let font = cell.textField?.font else { return "-" }
        return NSFontManager.shared.traits(of: font).contains(.boldFontMask) || font.fontName.contains("Semibold") ? "bold" : "regular"
    }

    /// Text of every visible cell in a row, columns joined by "|".
    private func debugCells(_ row: Int) -> String {
        tableView.tableColumns.indices.filter { !tableView.tableColumns[$0].isHidden }.map { index in
            let view = tableView(tableView, viewFor: tableView.tableColumns[index], row: content.tableRow(ofItem: row)) as? NSTableCellView
            return view?.textField?.stringValue ?? ""
        }.joined(separator: "|")
    }

    func debugDump(prefix: String) -> [String] {
        let view = session.view(tab: tabId)
        let selected = selection.compactMap { content.row(at: $0)?.name }
        let names = (0..<min(content.count, 50)).compactMap { content.row(at: $0)?.name }
        let sort = view.map { "\($0.sort) asc=\($0.ascending) foldersFirst=\($0.foldersFirst) hidden=\($0.showHidden)" } ?? "-"
        return [
            "\(prefix)path: \(content.path ?? "-")",
            "\(prefix)count: \(content.count)",
            "\(prefix)sort: \(sort)",
            "\(prefix)back: \(content.info?.canGoBack ?? false) forward: \(content.info?.canGoForward ?? false)",
            "\(prefix)selected: \(selected.joined(separator: "|"))",
            "\(prefix)rows: \(names.joined(separator: "|"))",
            "\(prefix)groups: \(content.groups.map { "\(groupTitle($0))@\($0.start)" }.joined(separator: "|"))",
            "\(prefix)tree: \(content.hasExpansions ? debugTree : "-")",
            "\(prefix)namefont: \(selection.first.map(debugNameFont) ?? "-")",
            "\(prefix)gridinfo: \(layout == .icons ? (selection.first.flatMap { (gridView.item(at: content.gridPath(ofItem: $0)) as? FileGridItem)?.infoText } ?? "-") : "-")",
            "\(prefix)columnview: \(layout == .columns ? columnBrowser.debugDescription(focusPath: content.path ?? "-") : "-")",
            "\(prefix)columnframes: \(layout == .columns ? columnBrowser.debugFrames() : "-")",
            "\(prefix)columnrows: \(layout == .columns ? columnBrowser.debugRows() : "-")",
            "\(prefix)columns: \(tableView.tableColumns.filter { !$0.isHidden }.map(\.title).joined(separator: "|"))",
            "\(prefix)cells: \(selection.first.map(debugCells) ?? "-")",
            "\(prefix)status: \(statusText)",
            "\(prefix)error: \(hasFailure ? banner.titleText : "-")",
            "\(prefix)error.action: \(hasFailure ? banner.extraTitle : "-")",
            "\(prefix)renaming: \(renaming.map { "\($0.original) editor=\(renameField()?.currentEditor()?.string ?? "-")" } ?? "-")",
            "\(prefix)confirm: \(pendingConfirmation?.messageText ?? "-")",
            "\(prefix)quicklook: \(isQuickLookVisible ? "visible" : "hidden")",
            "\(prefix)layout: \(layout) size=\(Int(layout == .icons ? iconSize : textSize))",
            "\(prefix)conn: \(isArchive ? "archive" : (isRemote ? "remote" : "local"))",
            "\(prefix)tags: \(isRemote ? "" : selectedPaths.map { "\(($0 as NSString).lastPathComponent)=[\(TagStore.tags(at: $0).joined(separator: "+"))]" }.joined(separator: " "))",
            "\(prefix)tagdots: \(isRemote ? 0 : (selectedPaths.first.map { TagStore.dots(for: TagStore.tags(at: $0)).length } ?? 0))",
            "\(prefix)scm: \(scm.sorted { $0.key < $1.key }.map { "\($0.key)=\(Self.scmLetter($0.value))" }.joined(separator: ","))",
            "\(prefix)search: \(search.map { "\($0.term) running=\($0.running) note=\($0.note ?? "-")" } ?? "-")",
        ]
    }
}

// MARK: - Inline rename editing

extension FileListViewController: NSTextFieldDelegate {
    func controlTextDidEndEditing(_ notification: Notification) {
        guard renaming != nil else { return }
        // Return commits. Losing focus any other way (a click elsewhere,
        // Tab) commits too, as in Finder; Esc is handled below.
        endRename(commit: true)
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        guard renaming != nil else { return false }
        if selector == #selector(NSResponder.cancelOperation(_:)) {
            cancelRename()
            return true
        }
        return false
    }
}

/// Inline, non-modal error strip above the list.
final class ErrorBanner: NSView {
    var onRetry: (() -> Void)?
    var onDismiss: (() -> Void)?

    private let title = NSTextField(labelWithString: "")
    private let detail = NSTextField(labelWithString: "")

    var titleText: String { title.stringValue }
    /// Title of the extra action button, "-" when there is none.
    var extraTitle: String { extra.isHidden ? "-" : extra.title }
    private let extra = NSButton(title: "", target: nil, action: nil)
    private var onExtra: (() -> Void)?

    /// An extra action beside Retry ("Authorize…"), or none.
    func setExtra(_ title: String?, action: (() -> Void)?) {
        extra.title = title ?? ""
        extra.isHidden = title == nil
        onExtra = action
    }

    @objc private func extraPressed(_ sender: Any?) { onExtra?() }

    func debugPressExtra() { if !extra.isHidden { onExtra?() } }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = NSColor.systemYellow.withAlphaComponent(0.18).cgColor

        let icon = NSImageView(image: NSImage(systemSymbolName: "exclamationmark.triangle.fill", accessibilityDescription: "Error")!)
        icon.contentTintColor = .systemYellow
        title.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .semibold)
        title.lineBreakMode = .byTruncatingMiddle
        detail.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        detail.textColor = .secondaryLabelColor
        detail.lineBreakMode = .byTruncatingTail
        let text = NSStackView(views: [title, detail])
        text.orientation = .vertical
        text.alignment = .leading
        text.spacing = 1

        let retry = NSButton(title: "Retry", target: self, action: #selector(retry(_:)))
        let dismiss = NSButton(title: "Dismiss", target: self, action: #selector(dismiss(_:)))
        for button in [retry, dismiss] {
            button.bezelStyle = .accessoryBarAction
            button.controlSize = .small
            button.setContentCompressionResistancePriority(.required, for: .horizontal)
        }
        extra.target = self
        extra.action = #selector(extraPressed(_:))
        extra.bezelStyle = .accessoryBarAction
        extra.controlSize = .small
        extra.isHidden = true
        extra.setContentCompressionResistancePriority(.required, for: .horizontal)
        let row = NSStackView(views: [icon, text, extra, retry, dismiss])
        row.orientation = .horizontal
        row.spacing = 8
        row.edgeInsets = NSEdgeInsets(top: 0, left: 10, bottom: 0, right: 10)
        row.translatesAutoresizingMaskIntoConstraints = false
        text.setContentHuggingPriority(.defaultLow, for: .horizontal)
        text.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        addSubview(row)
        NSLayoutConstraint.activate([
            row.leadingAnchor.constraint(equalTo: leadingAnchor),
            row.trailingAnchor.constraint(equalTo: trailingAnchor),
            row.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
        setAccessibilityRole(.group)
        setAccessibilityLabel("Error")
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func show(title: String, detail: String) {
        self.title.stringValue = title
        self.detail.stringValue = detail
    }

    @objc private func retry(_ sender: Any?) { onRetry?() }
    @objc private func dismiss(_ sender: Any?) { onDismiss?() }
}

extension SortColumn {
    /// Order of the Sort By menu; the item's tag indexes into this.
    /// Matches the spec's ⌃⌘ digits: 1 Name … 9 Group, 0 Permissions.
    static let menuOrder: [SortColumn] = [.name, .kind, .added, .modified, .created, .size, .tags, .owner, .group, .permissions]

    var menuTitle: String {
        switch self {
        case .name: return "Name"
        case .kind: return "Kind"
        case .added: return "Date Added"
        case .modified: return "Date Modified"
        case .created: return "Date Created"
        case .size: return "Size"
        case .tags: return "Tags"
        case .owner: return "Owner"
        case .group: return "Group"
        case .permissions: return "Permissions"
        }
    }
}

extension Array {
    subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}
