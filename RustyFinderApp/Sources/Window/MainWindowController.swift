import AppKit

/// Window that routes mouse buttons 4 and 5 to Back and Forward for the
/// pane under the pointer.
final class RustyFinderWindow: NSWindow {
    var onNavigationButton: ((_ forward: Bool, _ location: NSPoint) -> Void)?

    override func sendEvent(_ event: NSEvent) {
        if event.type == .otherMouseDown, event.buttonNumber == 3 || event.buttonNumber == 4 {
            onNavigationButton?(event.buttonNumber == 4, event.locationInWindow)
            return
        }
        super.sendEvent(event)
    }
}

/// One browser window. The Rust `WindowSession` owns the tabs and their
/// rules; this class draws its layout and forwards commands.
final class MainWindowController: NSWindowController, NSWindowDelegate, NSToolbarDelegate {
    /// Receives callbacks on Rust worker threads and hops to the main thread.
    /// Holds the controller weakly: Rust owns this object and the controller
    /// owns the Rust session, so a strong reference would never be released.
    private final class Bridge: WindowListener, @unchecked Sendable {
        weak var controller: MainWindowController?  // touched on the main thread only

        func windowChanged(layout: WindowLayout) {
            // Tabs report from their own threads, so these can arrive out
            // of order; an older layout landing last would undo a newer
            // one. Read the current layout when it is applied instead.
            DispatchQueue.main.async {
                guard let controller = self.controller else { return }
                controller.apply(controller.session.layout())
            }
        }

        func snapshotReady(tab: UInt64, snapshot: SnapshotHandle, info: PaneInfo) {
            DispatchQueue.main.async { self.controller?.fileList(for: tab)?.apply(snapshot, info) }
        }

        func loadFailed(tab: UInt64, path: String, error: RustyFinderError) {
            DispatchQueue.main.async {
                self.controller?.fileList(for: tab)?.showFailure(path: path, error: error)
                self.controller?.noteLoadFailure(path: path, error: error)
            }
        }
    }

    /// Shell state saved with the session (the Rust side stores it opaquely).
    private struct UIState: Codable {
        var frame: String
        var sidebarCollapsed: Bool
        var toolbarVisible: Bool
        var paneSplit: Double?
        var previewVisible: Bool?
        var previewMode: Int?
        var fullScreen: Bool?
        var titleHidden: Bool?
        var pathBarHidden: Bool?
    }

    let session: WindowSession
    var onClose: (() -> Void)?
    /// The workspace this window was opened from, if any.
    var workspaceId: UInt64?
    /// Tab icons, tag listings, reconnecting tabs and other chrome state.
    let chrome = WindowChrome()

    private(set) var layout: WindowLayout
    private var lists: [UInt64: FileListViewController] = [:]
    /// Tabs showing search results, and what was searched for.
    var searchTitles: [UInt64: String] = [:]
    private let searchField = SearchFieldController()
    private let rootSplit = NSSplitViewController()
    let panesSplit = NSSplitViewController()
    let sidebar = SidebarViewController()
    let left = PaneContainerViewController(side: .left)
    let right = PaneContainerViewController(side: .right)
    private(set) var sidebarItem: NSSplitViewItem!
    private let preview = PreviewPaneViewController()
    private(set) var previewItem: NSSplitViewItem!
    /// Pane and tab that last received keyboard focus from us.
    private var focusKey: (PaneSide, UInt64)?
    private var saveScheduled = false
    /// The "operation failed" sheet on screen, if any (for scripted checks).
    private(set) var problemAlert: NSAlert?

    private enum Item {
        static let back = NSToolbarItem.Identifier("rustyfinder.back")
        static let forward = NSToolbarItem.Identifier("rustyfinder.forward")
        static let up = NSToolbarItem.Identifier("rustyfinder.up")
        static let reload = NSToolbarItem.Identifier("rustyfinder.reload")
        static let hidden = NSToolbarItem.Identifier("rustyfinder.hidden")
        static let singlePane = NSToolbarItem.Identifier("rustyfinder.singlePane")
        static let activities = NSToolbarItem.Identifier("rustyfinder.activities")
        static let groupBy = NSToolbarItem.Identifier("rustyfinder.groupBy")
        static let copyTo = NSToolbarItem.Identifier("rustyfinder.copyTo")
        static let moveTo = NSToolbarItem.Identifier("rustyfinder.moveTo")
        static let delete = NSToolbarItem.Identifier("rustyfinder.delete")
    }

    /// Fills the Group By toolbar menu as it opens: the Sort By keys
    /// instead while Option is held.
    private final class GroupByMenuFiller: NSObject, NSMenuDelegate {
        func menuNeedsUpdate(_ menu: NSMenu) {
            menu.removeAllItems()
            if NSEvent.modifierFlags.contains(.option) {
                for (i, column) in SortColumn.menuOrder.enumerated() {
                    let item = NSMenuItem(title: column.menuTitle, action: #selector(FileListViewController.sortByMenu(_:)), keyEquivalent: "")
                    item.tag = i
                    menu.addItem(item)
                }
            } else {
                for (i, group) in GroupBy.menuOrder.enumerated() {
                    let item = NSMenuItem(title: group.menuTitle, action: #selector(FileListViewController.groupByMenu(_:)), keyEquivalent: "")
                    item.tag = i
                    menu.addItem(item)
                }
            }
        }
    }

    private let groupByFiller = GroupByMenuFiller()

    /// The preview pane is open.
    var isPreviewShown: Bool { !previewItem.isCollapsed }

    private let activitiesButton = NSButton()
    private let activitiesSpinner = NSProgressIndicator()
    private lazy var activitiesPopover: NSPopover = {
        let popover = NSPopover()
        let controller = ActivitiesViewController()
        controller.windowId = session.id()
        popover.contentViewController = controller
        popover.behavior = .transient
        return popover
    }()

    init(restoring saved: SavedWindowRecord?, leftPath: String? = nil) {
        let bridge = Bridge()
        session = AppEnvironment.shared.core.openWindow(listener: bridge, restore: saved?.json)
        layout = session.layout()

        let window = RustyFinderWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1280, height: 760),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered, defer: false)
        window.minSize = NSSize(width: 640, height: 360)
        window.title = "RustyFinder"
        window.toolbarStyle = .unified
        window.isReleasedWhenClosed = false
        window.isRestorable = false  // the Rust session restores windows
        window.tabbingMode = .disallowed
        super.init(window: window)
        bridge.controller = self

        left.delegate = self
        right.delegate = self
        panesSplit.addSplitViewItem(NSSplitViewItem(viewController: left))
        panesSplit.addSplitViewItem(NSSplitViewItem(viewController: right))
        panesSplit.splitView.dividerStyle = .thin

        sidebarItem = NSSplitViewItem(sidebarWithViewController: sidebar)
        sidebarItem.minimumThickness = 150
        sidebarItem.maximumThickness = 360
        rootSplit.addSplitViewItem(sidebarItem)
        rootSplit.addSplitViewItem(NSSplitViewItem(viewController: panesSplit))

        // The preview pane starts hidden (Cmd-Shift-P shows it).
        previewItem = NSSplitViewItem(viewController: preview)
        previewItem.canCollapse = true
        previewItem.minimumThickness = 240
        previewItem.holdingPriority = .init(260)
        previewItem.isCollapsed = true
        rootSplit.addSplitViewItem(previewItem)
        preview.windowId = session.id()
        preview.onNavigate = { [weak self] path in self?.open(path, inOtherPane: false) }

        sidebar.onOpen = { [weak self] path, other in self?.open(path, inOtherPane: other) }
        sidebar.onOpenTag = { [weak self] name in self?.showTagged(name) }
        sidebar.onOpenConnection = { [weak self] connection, other in
            self?.open(connection: connection, inOtherPane: other)
        }
        sidebar.onOpenInNewTab = { [weak self] path in
            guard let self else { return }
            _ = self.session.newTab(side: self.layout.active, path: path)
        }
        window.onNavigationButton = { [weak self] forward, point in self?.navigateByMouse(forward: forward, at: point) }

        window.contentViewController = rootSplit
        window.setContentSize(NSSize(width: 1280, height: 760))
        window.center()
        window.delegate = self

        setUpSearch()
        let toolbar = NSToolbar(identifier: "RustyFinderMainToolbar")
        toolbar.delegate = self
        toolbar.displayMode = .iconOnly
        window.toolbar = toolbar

        installChrome()
        restoreUI(saved?.ui)
        if let leftPath {
            session.navigate(tab: layout.left.tabs[Int(layout.left.selected)].id, path: leftPath)
        }
        apply(session.layout())

        NotificationCenter.default.addObserver(
            self, selector: #selector(settingsChanged), name: .rustyfinderSettingsChanged, object: nil)
        NotificationCenter.default.addObserver(
            self, selector: #selector(activitiesChanged), name: .rustyfinderActivitiesChanged, object: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    // MARK: File operations

    /// F5 / F6: copy or move the selection into the other pane's location.
    @objc func copyToOtherPane(_ sender: Any?) { transferToOtherPane(.copy) }
    @objc func moveToOtherPane(_ sender: Any?) { transferToOtherPane(.move) }

    private func transferToOtherPane(_ mode: OpMode) {
        guard let list = activeList, let target = otherPanePath else { return }
        let sources = list.selectedPaths
        guard !sources.isEmpty else { return }
        // From the active pane's file system to the other pane's: an
        // upload, a download, or a plain local copy.
        let (windowId, src, dst) = (session.id(), list.conn, otherPaneConn)
        LocalPreflight.run(mode == .move ? sources : [], conn: src, verb: "moved", window: window) {
            ActivityCenter.shared.submit(
                .transfer(mode: mode, sources: sources, destDir: target, renameTo: nil),
                window: windowId, src: src, dst: dst)
        }
    }

    // MARK: Volumes

    /// ⌘⇧E: eject the selected volume, or the one the active pane shows.
    @objc func ejectVolume(_ sender: Any?) {
        guard let volume = volumeToEject else { return }
        VolumeEject.eject(volume, window: window)
    }

    private var volumeToEject: URL? {
        guard let list = activeList, !list.isRemote else { return nil }
        if let selected = list.selectedPaths.first, list.selectedPaths.count == 1,
           let volume = VolumeEject.ejectableVolume(of: selected),
           volume.standardizedFileURL.path == URL(fileURLWithPath: selected).standardizedFileURL.path
        {
            return volume
        }
        return list.content.path.flatMap(VolumeEject.ejectableVolume(of:))
    }

    /// Tabs showing a local folder on the volume mounted at `mount` go to
    /// the starting folder, so RustyFinder never keeps the volume busy.
    func leaveVolume(_ mount: String) {
        let start = AppEnvironment.shared.core.startingDirectory()
        for tab in layout.left.tabs + layout.right.tabs where tab.conn == 0 {
            if tab.path == mount || tab.path.hasPrefix(mount.hasSuffix("/") ? mount : mount + "/") {
                session.navigateTo(tab: tab.id, conn: 0, path: start)
            }
        }
    }

    // MARK: Toolbar Delete

    /// The toolbar's Delete: Move to Trash, or with Option held the
    /// permanent delete (which always asks first).
    @objc func toolbarDelete(_ sender: Any?) {
        let option = NSApp.currentEvent?.modifierFlags.contains(.option) == true || LocalOpsDebug.optionHeld
        if option {
            activeList?.deleteImmediately(sender)
        } else {
            activeList?.moveToTrash(sender)
        }
    }

    /// "Make Alias in Other Pane" / "Make Symlink in Other Pane": links to
    /// the selection, created in the other pane's folder.
    @objc func makeAliasInOtherPane(_ sender: Any?) { linkInOtherPane(alias: true) }
    @objc func makeSymlinkInOtherPane(_ sender: Any?) { linkInOtherPane(alias: false) }

    private func linkInOtherPane(alias: Bool) {
        guard let list = activeList, let target = otherPanePath, canLinkInOtherPane(alias: alias) else { return }
        let sources = list.selectedPaths
        let op: FileOp = alias ? .alias(paths: sources, destDir: target) : .symlink(paths: sources, destDir: target)
        ActivityCenter.shared.submit(op, window: session.id(), src: list.conn, dst: list.conn)
    }

    /// Links point within one file system: an alias only on this Mac, a
    /// symlink only where both panes show the same one.
    private func canLinkInOtherPane(alias: Bool) -> Bool {
        guard let list = activeList, otherPanePath != nil, !list.selectedPaths.isEmpty,
              !list.isArchive, otherList?.isArchive != true, otherPaneConn == list.conn else { return false }
        return !alias || !list.isRemote
    }

    /// Connection of the pane that receives "Copy to" and "Move to".
    private var otherPaneConn: UInt64 {
        fileList(for: currentTab(other(layout.active)))?.conn ?? 0
    }

    // MARK: Search

    private func setUpSearch() {
        let title: (UInt64) -> String? = { [weak self] tab in
            self?.searchTitles[tab].map { "Search: “\($0)”" }
        }
        left.tabTitleOverride = title
        right.tabTitleOverride = title
        searchField.showsHidden = { AppEnvironment.shared.settings.showHidden }
        searchField.onFilter = { [weak self] text in self?.activeList?.setFilter(text) }
        searchField.onSearch = { [weak self] query, spotlight in
            guard let self else { return }
            // The results are not narrowed by an earlier filter.
            self.activeList?.setFilter("")
            let path = self.activeList?.content.path ?? NSHomeDirectory()
            if self.activeList?.isRemote == false { self.chrome.searches[self.activeTab] = (query, spotlight, path) }
            self.chrome.tagTabs[self.activeTab] = nil
            _ = self.session.search(tab: self.activeTab, queryJson: query, useSpotlight: spotlight)
        }
        searchField.onEnd = { [weak self] in
            guard let self, let list = self.activeList else { return }
            list.setFilter("")
            if list.search != nil { self.session.endSearch(tab: list.tabId) }
        }
        searchField.onDone = { [weak self] in self?.activeList?.focus() }
    }

    /// List everything in the home folder that carries a tag (Spotlight).
    func showTagged(_ name: String) {
        let query: [String: Any] = ["term": name, "criterion": "Tags", "subfolders": true]
        guard let data = try? JSONSerialization.data(withJSONObject: query),
              let json = String(data: data, encoding: .utf8)
        else { return }
        activeList?.setFilter("")
        chrome.tagTabs[activeTab] = name
        chrome.searches[activeTab] = nil
        _ = session.searchAt(tab: activeTab, connection: 0, path: NSHomeDirectory(), queryJson: json, useSpotlight: true)
        sidebar.highlight(tag: name)
    }

    /// Cmd-F: put the cursor in the search field.
    @objc func find(_ sender: Any?) {
        searchField.focus()
    }

    private var hasSearchField: Bool {
        window?.toolbar?.isVisible == true
            && window?.toolbar?.items.contains { $0.itemIdentifier == SearchFieldController.itemId } == true
    }

    /// Go to the folder a search result is in, with the result selected.
    @objc func showInEnclosingFolder(_ sender: NSMenuItem) {
        guard let path = sender.representedObject as? String, let list = activeList else { return }
        session.navigateTo(tab: list.tabId, conn: list.conn, path: (path as NSString).deletingLastPathComponent)
        list.reveal(names: [(path as NSString).lastPathComponent], renameAfter: false)
    }

    // MARK: Workspaces and network shares

    /// Save this window (frame, panes, tabs, view settings) under a name.
    @objc func saveWorkspace(_ sender: Any?) {
        guard let window, window.attachedSheet == nil else { return }
        let draft = FavoriteDraftRecord(name: "", path: "", icon: "rectangle.split.2x1", kind: .workspace, server: nil, payload: "")
        AddFavoriteSheet(draft: draft, group: "Workspaces").begin(on: window) { [weak self] result in
            guard let result else { return }
            self?.saveWorkspace(named: result.draft.name, group: result.group)
        }
    }

    func saveWorkspace(named name: String, group: String = "Workspaces") {
        guard let id = AppEnvironment.shared.core.saveWorkspaceIn(group: group, name: name, windowJson: savedJSON()) else {
            let shown = name.trimmingCharacters(in: .whitespaces)
            showProblem(
                title: shown.isEmpty ? "A workspace needs a name." : "The name “\(shown)” is already taken.",
                message: "Choose another name.")
            return
        }
        workspaceId = id
        AppEnvironment.shared.favoritesChanged()
    }

    /// Save the window as it is now over the workspace it came from.
    @objc func updateWorkspace(_ sender: Any?) {
        guard let id = workspaceId else { return }
        _ = AppEnvironment.shared.core.updateWorkspace(id: id, windowJson: savedJSON())
    }

    /// Mount a network share (SMB, AFP, NFS) and show it in the active
    /// pane; a `vnc://` address is handed to Screen Sharing.
    func openShare(_ params: ServerParams, scheme: String, side: PaneSide? = nil, completion: @escaping (String?) -> Void) {
        guard let url = NetworkShares.url(scheme: scheme, host: params.host, port: params.port, share: params.remotePath ?? "") else {
            return completion("The server address is not valid.")
        }
        if params.protocol == .vnc {
            ExternalApps.open(url)
            return completion(nil)
        }
        let side = side ?? layout.active
        NetworkShares.mount(url, user: params.user, password: params.password) { [weak self] result in
            switch result {
            case .success(let path):
                guard let self else { return }
                self.session.setActive(side: side)
                self.session.navigateTo(tab: self.currentTab(side), conn: 0, path: path)
                completion(nil)
            case .failure(let failure):
                completion(failure.message)
            }
        }
    }

    // MARK: Tools, Terminal, Checksum, Source Control

    /// What a tool's placeholders stand for right now, and the folder it
    /// runs in: the active pane's if that is on this Mac, else home.
    private func toolContext() -> (context: ToolContext, cwd: String)? {
        guard let list = activeList, let path = list.content.path else { return nil }
        var (user, host) = ("", "")
        if list.isRemote, !list.isArchive {
            let label = AppEnvironment.shared.core.connections().first { $0.id == list.conn }?.label ?? ""
            let parts = label.split(separator: "@", maxSplits: 1).map(String.init)
            (user, host) = parts.count == 2 ? (parts[0], parts[1]) : ("", label)
        }
        let context = ToolContext(
            sourcePath: path, targetPath: otherList?.content.path ?? "",
            sourceSelection: list.selectedPaths, targetSelection: otherList?.selectedPaths ?? [],
            hostname: host, username: user)
        return (context, list.isRemote ? NSHomeDirectory() : path)
    }

    @objc func runTool(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? UInt64, let tool = ToolCenter.shared.tool(id: id),
              let (context, cwd) = toolContext()
        else { return }
        ToolCenter.shared.run(tool, context: context, cwd: cwd, window: session.id())
    }

    /// Open the Terminal app at the selected folder, else at the pane's.
    @objc func openInTerminal(_ sender: Any?) {
        if let list = activeList, list.isRemote {
            openRemoteTerminal(list)
            return
        }
        guard let list = activeList, !list.isRemote, let folder = list.selectedFolder ?? list.content.path else { return }
        ExternalApps.open([URL(fileURLWithPath: folder)], withAppAt: ExternalApps.terminalApp)
    }

    @objc func checksum(_ sender: Any?) {
        guard let list = activeList else { return }
        let files = list.inspectedSelection.filter { !$0.row.isDir }
        guard !files.isEmpty else { return }
        ChecksumWindowController.show(items: files, conn: list.conn)
    }

    @objc func toggleScmStatus(_ sender: Any?) {
        Prefs.set("view.scmStatus", !Prefs.bool("view.scmStatus"))
        lists.values.forEach { $0.refreshScm() }
    }

    /// Run git in the active pane's folder as an activity.
    private func git(_ what: String, _ arguments: [String]) {
        guard let list = activeList, !list.isRemote, let cwd = list.content.path else { return }
        let command = (["git"] + arguments).map { shellEscape(path: $0) }.joined(separator: " ")
        ActivityCenter.shared.submit(
            .shell(
                name: "git \(arguments.first ?? "")", title: "SCM Operation: \(what)",
                summary: "SCM Operation: \(what)", commands: [command], cwd: cwd),
            window: session.id())
    }

    @objc func scmAdd(_ sender: Any?) {
        guard let paths = activeList?.selectedPaths, !paths.isEmpty else { return }
        git("Add", ["add", "--"] + paths)
    }

    @objc func scmAddAll(_ sender: Any?) { git("Add All", ["add", "-A"]) }
    @objc func scmPull(_ sender: Any?) { git("Pull", ["pull"]) }
    @objc func scmPush(_ sender: Any?) { git("Push", ["push"]) }

    @objc func scmCommit(_ sender: Any?) {
        guard let window else { return }
        CommitSheet().begin(on: window) { [weak self] message in
            guard let message else { return }
            self?.git("Commit", ["commit", "-m", message])
        }
    }

    // MARK: Archives

    /// True if any tab of this window shows `connection`.
    func shows(connection: UInt64) -> Bool {
        (layout.left.tabs + layout.right.tabs).contains { $0.conn == connection }
    }

    /// Show an archive's contents in the tab that holds it.
    private func browseArchive(_ path: String, in list: FileListViewController) {
        let tab = list.tabId
        ArchiveCenter.shared.open(connection: list.conn, path: path, window: window, windowId: session.id()) { [weak self] record in
            guard let self, let record else { return }
            self.session.navigateTo(tab: tab, conn: record.id, path: "/")
        }
    }

    /// The selected archives of the active pane, as (name, path).
    private var selectedArchives: [String] {
        (activeList?.inspectedSelection ?? [])
            .filter { !$0.row.isDir && isArchiveName(name: $0.row.name) }
            .map(\.path)
    }

    @objc func showArchiveContents(_ sender: Any?) {
        guard let list = activeList, list.selectedPaths.count == 1, let path = selectedArchives.first else { return }
        browseArchive(path, in: list)
    }

    /// Extract each selected archive into the folder it is in.
    @objc func extractHere(_ sender: Any?) {
        guard let list = activeList else { return }
        if handOffArchives(selectedArchives, local: !list.isRemote) { return }
        for path in selectedArchives {
            ArchiveCenter.shared.extract(
                connection: list.conn, path: path, destConnection: list.conn,
                destDir: (path as NSString).deletingLastPathComponent, window: window, windowId: session.id())
        }
    }

    /// Extract each selected archive into the other pane's folder.
    @objc func extractTo(_ sender: Any?) {
        guard let list = activeList, let target = otherPanePath else { return }
        for path in selectedArchives {
            ArchiveCenter.shared.extract(
                connection: list.conn, path: path, destConnection: otherPaneConn, destDir: target,
                window: window, windowId: session.id())
        }
    }

    /// What Compress would pack, and the name it would give the archive.
    private var compressPlan: (list: FileListViewController, sources: [String], dir: String, base: String)? {
        guard let list = activeList, let dir = list.content.path, !list.isArchive else { return nil }
        let items = list.inspectedSelection
        guard let first = items.first else { return nil }
        let base = defaultArchiveName(
            names: items.map { ($0.path as NSString).lastPathComponent }, firstIsDir: first.row.isDir)
        return (list, items.map(\.path), dir, base)
    }

    /// Zip the selection next to it, no questions asked.
    @objc func compress(_ sender: Any?) {
        guard let plan = compressPlan else { return }
        if handOffArchives(plan.sources, local: !plan.list.isRemote) { return }
        ActivityCenter.shared.submit(
            .compress(
                sources: plan.sources, destDir: plan.dir, name: plan.base + ".zip", format: .zip,
                level: .normal, password: nil, excludeMetadata: true),
            window: session.id(), src: plan.list.conn, dst: plan.list.conn)
    }

    @objc func compressWithOptions(_ sender: Any?) {
        guard let plan = compressPlan, let window else { return }
        let windowId = session.id()
        let conn = plan.list.conn
        CompressSheet(baseName: plan.base).begin(on: window) { choice in
            guard let choice else { return }
            ActivityCenter.shared.submit(
                .compress(
                    sources: plan.sources, destDir: plan.dir, name: choice.name, format: choice.format,
                    level: choice.level, password: choice.password, excludeMetadata: choice.excludeMetadata),
                window: windowId, src: conn, dst: conn)
        }
    }

    /// "Handle archives with RustyFinder" off: Archive Utility extracts
    /// and compresses local items instead.
    private func handOffArchives(_ paths: [String], local: Bool) -> Bool {
        guard !AppEnvironment.shared.settings.handleArchives, local, !paths.isEmpty else { return false }
        ExternalApps.open(paths.map { URL(fileURLWithPath: $0) }, withAppAt: "/System/Library/CoreServices/Applications/Archive Utility.app")
        return true
    }

    // MARK: Sync and Compare

    var otherList: FileListViewController? {
        layout.singlePane ? nil : fileList(for: currentTab(other(layout.active)))
    }

    /// A pane's folder as one side of a sync.
    private func syncSide(_ list: FileListViewController) -> SyncWindowController.Side? {
        guard let path = list.content.path else { return nil }
        let server = AppEnvironment.shared.core.connections().first { $0.id == list.conn }?.label
        return .init(conn: list.conn, root: path, label: server.map { "\($0):\(path)" } ?? path)
    }

    /// "Sync to…": compare the active pane's folder (the source) with the
    /// other pane's (the target). The left pane is always the left side.
    @objc func syncTo(_ sender: Any?) {
        guard let active = activeList, let other = otherList,
              let source = syncSide(active), let target = syncSide(other)
        else { return }
        let sourceIsLeft = layout.active == .left
        SyncWindowController.show(
            left: sourceIsLeft ? source : target, right: sourceIsLeft ? target : source,
            leftToRight: sourceIsLeft, windowId: session.id())
    }

    /// The two files Compare… acts on: two selected in the active pane, or
    /// one in each pane. Folders are for the Sync window.
    private var compareTargets: [(conn: UInt64, path: String)]? {
        guard let active = activeList else { return nil }
        var picked = active.inspectedSelection.map { (active.conn, $0) }
        if picked.count == 1, let other = otherList, other.inspectedSelection.count == 1 {
            picked += other.inspectedSelection.map { (other.conn, $0) }
        }
        guard picked.count == 2, !picked.contains(where: { $0.1.row.isDir }) else { return nil }
        return picked.map { (conn: $0.0, path: $0.1.path) }
    }

    @objc func compare(_ sender: Any?) {
        guard let targets = compareTargets else { return }
        guard let tool = CompareTool.resolve() else {
            let chosen = CompareTool.chosen
            showProblem(
                title: "\(chosen?.name ?? "The compare tool") isn’t available.",
                message: "Comparing files uses \(chosen?.name ?? "the compare tool chosen in Settings"). \(chosen?.installHint ?? "")")
            return
        }
        // Remote files are fetched to temporary copies first.
        var local = [String?](repeating: nil, count: targets.count)
        var waiting = 0
        var failed = false
        let launch = { [weak self] in
            guard waiting == 0 else { return }
            let paths = local.compactMap { $0 }
            guard !failed, paths.count == targets.count else { return }
            if let message = tool.run(paths[0], paths[1]) {
                self?.showProblem(title: "The files could not be compared.", message: message)
            }
        }
        for (index, target) in targets.enumerated() {
            if target.conn == 0 {
                local[index] = target.path
                continue
            }
            waiting += 1
            RemoteFiles.shared.fetch(connection: target.conn, remotePath: target.path, window: session.id()) { path in
                waiting -= 1
                local[index] = path
                if path == nil { failed = true }
                launch()
            }
        }
        launch()
    }

    /// A plain alert on this window.
    func showProblem(title: String, message: String) {
        guard let window else { return }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = title
        alert.informativeText = message
        problemAlert = alert
        alert.beginSheetModal(for: window) { [weak self] _ in self?.problemAlert = nil }
    }

    // MARK: Connections

    private var connectSheet: ConnectSheetController?

    /// Cmd-K: the Connect panel. The new connection opens in the active pane.
    @objc func connect(_ sender: Any?) {
        guard let window, window.attachedSheet == nil else { return }
        let controller = ConnectSheetController()
        // "Open remote connections in": the active pane, or always one side
        // (which then shows, even in single-pane mode).
        let side = remoteSide
        controller.onConnected = { [weak self] connection in
            guard let self else { return }
            self.session.setActive(side: side)
            self.session.navigateTo(tab: self.currentTab(side), conn: connection.id, path: connection.home)
            self.connectSheet = nil
        }
        controller.onOpenShare = { [weak self] params, scheme, done in
            guard let self else { return done("The window is gone.") }
            self.openShare(params, scheme: scheme, side: side) { message in
                if message == nil { self.connectSheet = nil }
                done(message)
            }
        }
        connectSheet = controller
        let sheet = NSWindow(contentViewController: controller)
        sheet.styleMask = [.titled]
        window.beginSheet(sheet) { [weak self] _ in self?.connectSheet = nil }
    }

    /// Servers send no change notifications, so panes showing one are
    /// re-read after each of RustyFinder's own operations.
    func reloadRemoteTabs() {
        for side in [PaneSide.left, .right] {
            let tab = currentTab(side)
            if fileList(for: tab)?.isRemote == true { session.reload(tab: tab) }
        }
    }

    /// Close the connection the active pane is on.
    @objc func disconnect(_ sender: Any?) {
        guard let conn = activeList?.conn, conn != 0 else { return }
        ConnectionCenter.shared.disconnect(conn)
    }

    @objc func goToConnection(_ sender: NSMenuItem) {
        guard let connection = sender.representedObject as? ConnectionRecord else { return }
        session.navigateTo(tab: activeTab, conn: connection.id, path: connection.home)
    }

    /// Open a connection's start folder in the active pane (sidebar click).
    func open(connection: ConnectionRecord, inOtherPane: Bool) {
        let side = inOtherPane && !layout.singlePane ? other(layout.active) : layout.active
        session.navigateTo(tab: currentTab(side), conn: connection.id, path: connection.home)
    }

    /// Where "Copy to" and "Move to" put things: the other pane's folder.
    /// Unavailable in single-pane mode, where that pane cannot be seen.
    var otherPanePath: String? {
        guard !layout.singlePane else { return nil }
        let side = other(layout.active)
        return fileList(for: currentTab(side))?.content.path
    }

    /// "Copy to as…": copy the single selected item to the other pane
    /// under a new name. Esc cancels.
    @objc func copyToAs(_ sender: Any?) {
        guard let source = activeList?.selectedPaths.first, activeList?.selectedPaths.count == 1,
              let target = otherPanePath, let window
        else { return }
        let alert = NSAlert()
        alert.messageText = "Copy to “\(target.folderDisplayName)” as"
        alert.addButton(withTitle: "Copy")
        alert.addButton(withTitle: "Cancel")
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 320, height: 24))
        field.stringValue = (source as NSString).lastPathComponent
        alert.accessoryView = field
        alert.window.initialFirstResponder = field
        let windowId = session.id()
        let (from, to) = (activeList?.conn ?? 0, otherPaneConn)
        alert.beginSheetModal(for: window) { response in
            let name = field.stringValue
            guard response == .alertFirstButtonReturn, !name.isEmpty else { return }
            ActivityCenter.shared.submit(
                .transfer(mode: .copy, sources: [source], destDir: target, renameTo: name),
                window: windowId, src: from, dst: to)
        }
    }

    /// Asks first, every time: emptying the Trash cannot be undone.
    @objc func emptyTrash(_ sender: Any?) {
        guard let window else { return }
        let alert = NSAlert()
        alert.alertStyle = .critical
        alert.messageText = "Empty the Trash?"
        alert.informativeText = "Everything in the Trash is erased. This can’t be undone."
        alert.addButton(withTitle: "Empty Trash").hasDestructiveAction = true
        alert.addButton(withTitle: "Cancel").keyEquivalent = "\r"
        alert.buttons[0].keyEquivalent = ""
        alert.beginSheetModal(for: window) { response in
            guard response == .alertFirstButtonReturn else { return }
            // Only Finder can empty every volume's Trash; ask it to.
            var error: NSDictionary?
            NSAppleScript(source: "tell application \"Finder\" to empty trash")?.executeAndReturnError(&error)
            if let message = error?[NSAppleScript.errorMessage] as? String {
                let failure = NSAlert()
                failure.alertStyle = .warning
                failure.messageText = "The Trash could not be emptied."
                failure.informativeText = message
                failure.beginSheetModal(for: window)
            }
        }
    }

    /// Select freshly created or moved items in whichever pane shows their
    /// folder (the active pane first).
    func reveal(_ paths: [String], renameAfter: Bool) {
        guard let first = paths.first else { return }
        let dir = (first as NSString).deletingLastPathComponent
        let names = paths.filter { ($0 as NSString).deletingLastPathComponent == dir }
            .map { ($0 as NSString).lastPathComponent }
        for side in [layout.active, other(layout.active)] {
            guard let list = fileList(for: currentTab(side)), list.content.path == dir else { continue }
            list.reveal(names: names, renameAfter: renameAfter && side == layout.active)
            if renameAfter { break }
        }
    }

    /// An operation was refused or some of its items failed.
    func showOperationProblem(title: String, message: String, partial: Bool) {
        guard let window else { return }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = partial ? "\(title): some items could not be processed." : "\(title) could not be completed."
        alert.informativeText = message
        problemAlert = alert
        // Queue behind any sheet that is already up (a conflict prompt).
        if window.attachedSheet == nil {
            alert.beginSheetModal(for: window) { [weak self] _ in self?.problemAlert = nil }
        } else {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
                self?.showOperationProblem(title: title, message: message, partial: partial)
            }
        }
    }

    @objc private func activitiesChanged() {
        let busy = AppEnvironment.shared.core.hasActive(window: nil)
        busy ? activitiesSpinner.startAnimation(nil) : activitiesSpinner.stopAnimation(nil)
        activitiesSpinner.isHidden = !busy
        activitiesButton.contentTintColor = busy ? .controlAccentColor : nil
        window?.toolbar?.validateVisibleItems()
    }

    @objc func showActivities(_ sender: Any?) {
        if activitiesPopover.isShown {
            activitiesPopover.close()
        } else if activitiesButton.window != nil {
            activitiesPopover.show(relativeTo: activitiesButton.bounds, of: activitiesButton, preferredEdge: .maxY)
        }
    }

    /// Closing a window with work in flight would abort it: ask first.
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        let core = AppEnvironment.shared.core
        guard core.hasActive(window: session.id()) else { return true }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "Activities are still running in this window."
        alert.informativeText = "Closing the window stops them. Files copied so far stay where they are."
        alert.addButton(withTitle: "Cancel")
        alert.addButton(withTitle: "Close")
        alert.beginSheetModal(for: sender) { [weak self] response in
            guard let self, response == .alertSecondButtonReturn else { return }
            core.stopAllActivities(window: self.session.id())
            sender.close()
        }
        return false
    }

    // MARK: Layout from Rust

    func tabs(_ side: PaneSide) -> PaneTabsRecord { side == .left ? layout.left : layout.right }
    func container(_ side: PaneSide) -> PaneContainerViewController { side == .left ? left : right }
    func other(_ side: PaneSide) -> PaneSide { side == .left ? .right : .left }

    /// Id of the selected tab in `side`.
    func currentTab(_ side: PaneSide) -> UInt64 {
        let pane = tabs(side)
        return pane.tabs[Int(pane.selected)].id
    }

    var activeTab: UInt64 { currentTab(layout.active) }
    var activeList: FileListViewController? { fileList(for: activeTab) }

    /// The list view for a tab, created on first use. Returns nil for a tab
    /// that no longer exists (a late event for a closed tab).
    func fileList(for tab: UInt64) -> FileListViewController? {
        if let existing = lists[tab] { return existing }
        let live = session.layout()
        guard (live.left.tabs + live.right.tabs).contains(where: { $0.id == tab }) else { return nil }

        let list = FileListViewController(session: session, tabId: tab)
        list.onActivate = { [weak self, weak list] in
            guard let self, let list, let side = self.side(of: list.tabId), side != self.layout.active else { return }
            self.session.setActive(side: side)
        }
        list.onChange = { [weak self, weak list] in
            guard let self else { return }
            // A tab that starts or stops showing search results is renamed.
            if let list, self.searchTitles[list.tabId] != list.search?.term {
                self.searchTitles[list.tabId] = list.search?.term
                for side in [PaneSide.left, .right] {
                    if let current = self.fileList(for: self.currentTab(side)) {
                        self.container(side).update(pane: self.tabs(side), list: current, active: self.layout.active == side)
                    }
                }
            }
            self.refreshChrome()
        }
        list.onOpenInOtherPane = { [weak self] path in self?.open(path, inOtherPane: true) }
        list.onOpenInNewTab = { [weak self, weak list] path in
            guard let self, let list, let side = self.side(of: list.tabId) else { return }
            _ = self.session.newTab(side: side, path: path)
        }
        list.onSwitchPane = { [weak self] in self?.session.switchActive() }
        list.onOpenArchive = { [weak self, weak list] path in
            guard let self, let list else { return false }
            self.browseArchive(path, in: list)
            return true
        }
        list.onQuickOpen = { [weak self] in self?.quickOpen(nil) }
        list.onCycleTab = { [weak self, weak list] delta in
            guard let self, let list, let side = self.side(of: list.tabId) else { return }
            self.session.cycleTab(side: side, delta: Int32(delta))
        }
        lists[tab] = list
        return list
    }

    func side(of tab: UInt64) -> PaneSide? {
        if layout.left.tabs.contains(where: { $0.id == tab }) { return .left }
        if layout.right.tabs.contains(where: { $0.id == tab }) { return .right }
        return nil
    }

    func apply(_ new: WindowLayout) {
        layout = new
        let alive = Set((new.left.tabs + new.right.tabs).map(\.id))
        for id in lists.keys where !alive.contains(id) {
            lists[id]?.view.removeFromSuperview()
            lists[id]?.removeFromParent()
            lists[id] = nil
        }

        for side in [PaneSide.left, .right] {
            if let list = fileList(for: currentTab(side)) {
                container(side).update(pane: tabs(side), list: list, active: new.active == side)
            }
        }

        panesSplit.splitView.isVertical = !new.horizontal
        // In single-pane mode only the active pane shows; the other keeps its tabs.
        let items = panesSplit.splitViewItems
        items[0].isCollapsed = new.single_pane_hides(.left)
        items[1].isCollapsed = new.single_pane_hides(.right)

        // Move keyboard focus only when the active pane or its tab changed,
        // so typing in a sheet or text field is never interrupted.
        let key = (new.active, activeTab)
        if focusKey?.0 != key.0 || focusKey?.1 != key.1 {
            focusKey = key
            activeList?.focus()
            NSAccessibility.post(
                element: window as Any, notification: .announcementRequested,
                userInfo: [.announcement: new.active == .left ? "Left pane" : "Right pane", .priority: NSAccessibilityPriorityLevel.medium.rawValue])
        }
        refreshChrome()
        // Recent Folders follow the active pane's data source.
        let activeConn = activeList.map { $0.isArchive ? 0 : $0.conn } ?? 0
        sidebar.recentsConnection = activeConn
        sidebar.reloadRecents()
        for id in chrome.tabIcons.keys where !alive.contains(id) { chrome.tabIcons[id] = nil }
        reconnectVisibleTabs()
        scheduleSave()
    }

    /// Title, subtitle, status lines and toolbar state.
    func refreshChrome() {
        left.refresh()
        right.refresh()
        guard let window else { return }
        let pane = tabs(layout.active)
        let tab = pane.tabs[Int(pane.selected)]
        let path = activeList?.content.path ?? tab.path
        searchField.isRemote = tab.conn != 0
        searchField.show(activeList?.search?.term ?? activeList?.currentFilter ?? "")
        if let term = activeList?.search?.term {
            window.title = "Search: “\(term)”"
            window.subtitle = tab.conn == 0 ? path.abbreviatingHome : "\(tab.label):\(path)"
            window.representedURL = nil
        } else if tab.conn == 0 {
            window.title = path.folderDisplayName
            window.subtitle = path.abbreviatingHome
            window.representedURL = URL(fileURLWithPath: path)
        } else {
            // On a server: its name and the path there. No file proxy.
            let name = (path as NSString).lastPathComponent
            window.title = name.isEmpty || path == "/" ? tab.label : name
            window.subtitle = "\(tab.label):\(path)"
            window.representedURL = nil
        }
        window.toolbar?.validateVisibleItems()
        if let tag = chrome.tagTabs[tab.id], activeList?.search != nil {
            sidebar.highlight(tag: tag)
        } else {
            sidebar.highlight(path: tab.conn == 0 ? path : nil)
        }
        updatePreview()
    }

    // MARK: Preview pane and Get Info

    /// Feed the active pane's selection to the preview pane, if it shows.
    private func updatePreview(immediately: Bool = false) {
        guard !previewItem.isCollapsed else { return }
        // Items on a server or in an archive are previewed from a local copy.
        preview.setSelection(activeList?.inspectedSelection ?? [], immediately: immediately)
    }

    @objc func togglePreview(_ sender: Any?) {
        previewItem.isCollapsed.toggle()
        // Opened with something selected: show it right away.
        updatePreview(immediately: true)
        scheduleSave()
    }

    private func showPreviewPane(mode: PreviewPaneViewController.Mode) {
        previewItem.isCollapsed = false
        preview.show(mode: mode)
        updatePreview(immediately: true)
        scheduleSave()
    }

    /// Cmd-Shift-U: the preview pane, switched to the activity list.
    @objc func showActivitiesPane(_ sender: Any?) { showPreviewPane(mode: .activities) }
    @objc func showLog(_ sender: Any?) { showPreviewPane(mode: .log) }

    /// Cmd-I: an Info window per selected item.
    @objc func getInfo(_ sender: Any?) {
        guard let items = activeList?.inspectedSelection, !items.isEmpty else { return }
        InfoWindowController.show(items, windowId: session.id()) { [weak self] path in
            self?.open(path, inOtherPane: false)
        }
    }

    @objc private func settingsChanged() {
        apply(layout)
    }

    // MARK: Opening locations

    /// Open `path` in the active pane, or the other one. With a single pane
    /// visible, "other" means the visible pane.
    func open(_ path: String, inOtherPane: Bool) {
        let side = inOtherPane && !layout.singlePane ? other(layout.active) : layout.active
        session.navigate(tab: currentTab(side), path: path)
    }

    private func navigateByMouse(forward: Bool, at point: NSPoint) {
        let side: PaneSide = [PaneSide.left, .right].first { side in
            let view = container(side).view
            return !view.isHiddenOrHasHiddenAncestor && view.bounds.contains(view.convert(point, from: nil))
        } ?? layout.active
        forward ? session.goForward(tab: currentTab(side)) : session.goBack(tab: currentTab(side))
    }

    // MARK: Navigation actions

    @objc func goBack(_ sender: Any?) { session.goBack(tab: activeTab) }
    @objc func goForward(_ sender: Any?) { session.goForward(tab: activeTab) }
    @objc func goUp(_ sender: Any?) { session.goUp(tab: activeTab) }
    @objc func reload(_ sender: Any?) {
        session.reload(tab: activeTab)
        // On a server, Refresh also fetches the previewed item again.
        if activeList?.isRemote == true, !previewItem.isCollapsed { preview.refreshRemote() }
    }

    @objc func goToPlace(_ sender: NSMenuItem) {
        let home = NSHomeDirectory()
        let places = [
            home, "\(home)/Desktop", "\(home)/Documents", "\(home)/Downloads", "\(home)/Library",
            "/Applications", "/Applications/Utilities",
        ]
        guard let path = places[safe: sender.tag] else { return }
        session.navigate(tab: activeTab, path: path)
    }

    @objc func goToRecent(_ sender: NSMenuItem) {
        guard let path = sender.representedObject as? String else { return }
        session.navigate(tab: activeTab, path: path)
    }

    // MARK: Keyboard pop-ups

    var palette: PaletteViewController?
    private var quickSelectSheet: QuickSelectViewController?

    func present(_ controller: NSViewController) {
        guard let window else { return }
        let sheet = NSWindow(contentViewController: controller)
        sheet.styleMask = [.titled]
        window.beginSheet(sheet) { [weak self] _ in
            self?.palette = nil
            self?.quickSelectSheet = nil
        }
    }

    /// Esc in the file list: favorites, devices, connections and every
    /// menu command in one searchable list.
    @objc func quickOpen(_ sender: Any?) {
        guard let window, window.attachedSheet == nil else { return }
        // Built before the pop-up opens, so commands are judged against
        // the panes, not against the pop-up itself.
        let candidates = quickOpenCandidates(devicesOnly: false)
        let controller = PaletteViewController()
        controller.placeholder = "Open a favorite or run a command"
        controller.rowsProvider = { query in
            // Empty query: the first entries of each section, as built.
            guard !query.isEmpty else { return Array(candidates.prefix(50)) }
            return fuzzyRank(query: query, candidates: candidates.map(\.title), limit: 50).map { hit in
                var row = candidates[Int(hit.index)]
                row.highlights = hit.positions.map(Int.init)
                return row
            }
        }
        palette = controller
        present(controller)
        _ = window
    }

    /// Ctrl-D: only the devices, shares and open connections.
    @objc func showDevices(_ sender: Any?) {
        guard window?.attachedSheet == nil else { return }
        let candidates = quickOpenCandidates(devicesOnly: true)
        let controller = PaletteViewController()
        controller.placeholder = "Open a device or connection"
        controller.rowsProvider = { query in
            guard !query.isEmpty else { return candidates }
            return fuzzyRank(query: query, candidates: candidates.map(\.title), limit: 50).map { hit in
                var row = candidates[Int(hit.index)]
                row.highlights = hit.positions.map(Int.init)
                return row
            }
        }
        palette = controller
        present(controller)
    }

    private func quickOpenCandidates(devicesOnly: Bool) -> [PaletteRow] {
        guard let window else { return [] }
        let core = AppEnvironment.shared.core
        var rows: [PaletteRow] = []
        // Actions run after the pop-up has closed.
        func later(_ work: @escaping () -> Void) -> () -> Void {
            { DispatchQueue.main.async(execute: work) }
        }
        if !devicesOnly {
            // Folders, files, servers (and paths under them), shares,
            // workspaces, synclets and saved searches. Presets have their
            // own section; tags are not favorites here.
            for group in core.favoriteGroups() {
                for favorite in group.items where favorite.kind != .renamePreset {
                    let place = favorite.server.map { serverSpecUrl(spec: $0) + favorite.path } ?? favorite.path.abbreviatingHome
                    rows.append(PaletteRow(
                        title: favorite.name, detail: "Favorite · \(place.isEmpty ? group.name : place)",
                        icon: FavoriteIcons.image(for: favorite),
                        activate: later { [weak self] in self?.openFavorite(favorite) }))
                    for child in favorite.children {
                        rows.append(PaletteRow(
                            title: "\(favorite.name) › \(child.name)", detail: "Favorite · \(child.path)",
                            icon: NSImage(systemSymbolName: "folder", accessibilityDescription: nil),
                            activate: later { [weak self] in self?.openFavoritePath(child, of: favorite) }))
                    }
                }
            }
        }
        let keys: [URLResourceKey] = [.volumeNameKey]
        for url in FileManager.default.mountedVolumeURLs(includingResourceValuesForKeys: keys, options: [.skipHiddenVolumes]) ?? [] {
            let name = (try? url.resourceValues(forKeys: Set(keys)))?.volumeName ?? url.lastPathComponent
            let free = core.space(path: url.path).map { " · \(RowPresenter.shared.bytes($0.free)) free" } ?? ""
            rows.append(PaletteRow(
                title: name, detail: "Device\(free)", icon: RowPresenter.shared.folderIcon(atPath: url.path),
                activate: later { [weak self] in self?.open(url.path, inOtherPane: false) }))
        }
        for connection in core.connections() {
            rows.append(PaletteRow(
                title: connection.label, detail: "Connection",
                icon: NSImage(systemSymbolName: "network", accessibilityDescription: nil),
                activate: later { [weak self] in self?.open(connection: connection, inOtherPane: false) }))
        }
        guard !devicesOnly else { return rows }
        rows += quickOpenSelectionRows(later: later)
        // Every enabled menu command, alphabetical, with its menu path
        // and current key binding. Disabled ones are left out.
        let commands = CommandCatalog.commands(in: window).sorted { $0.path < $1.path }
        for command in commands {
            rows.append(PaletteRow(
                title: command.path, detail: "", icon: NSImage(systemSymbolName: "command", accessibilityDescription: nil),
                shortcut: command.shortcut,
                activate: later { [weak self] in
                    guard let window = self?.window else { return }
                    CommandCatalog.perform(command.item, in: window)
                }))
        }
        return rows
    }

    /// Cmd-Shift-G: a path field with completion. Works on whatever file
    /// system the active tab shows.
    @objc func goToFolder(_ sender: Any?) {
        guard window?.attachedSheet == nil, let list = activeList else { return }
        let core = AppEnvironment.shared.core
        let tab = activeTab
        let conn = list.conn
        let current = list.content.path ?? "/"
        let controller = PaletteViewController()
        controller.placeholder = "Go to folder (~ is the home folder)"
        controller.tabCompletes = true
        controller.rowsProvider = { text in
            if text.isEmpty {
                // Nothing typed yet: earlier destinations, newest first.
                return core.gotoHistory().map {
                    PaletteRow(title: $0.abbreviatingHome, detail: "Recent",
                               icon: NSImage(systemSymbolName: "clock", accessibilityDescription: nil), completion: $0)
                }
            }
            return core.gotoComplete(connection: conn, text: text, currentDir: current).map {
                PaletteRow(
                    title: $0.name, detail: $0.isDir ? "Folder" : "",
                    icon: NSImage(systemSymbolName: $0.isDir ? "folder" : "doc", accessibilityDescription: nil),
                    completion: $0.text)
            }
        }
        controller.onReturn = { [weak self, weak list] text, row in
            // An empty field with a recent destination highlighted goes there.
            let typed = text.isEmpty ? (row?.completion ?? "") : text
            guard !typed.isEmpty, let self else { return "Enter a path." }
            let result = core.gotoResolve(connection: conn, text: typed, currentDir: current)
            switch result.kind {
            case .folder:
                self.session.navigateTo(tab: tab, conn: conn, path: result.path)
            case .file:
                // A file is shown in its folder and selected, never opened.
                self.session.navigateTo(tab: tab, conn: conn, path: result.parent)
                list?.reveal(names: [result.name], renameAfter: false)
            case .missing:
                return "“\(result.path)” can’t be found."
            }
            core.recordGoto(path: result.path)
            return nil
        }
        palette = controller
        present(controller)
    }

    /// Cmd-S: select by pattern in the active pane.
    @objc func quickSelect(_ sender: Any?) {
        guard window?.attachedSheet == nil, let list = activeList else { return }
        let controller = QuickSelectViewController(list: list)
        quickSelectSheet = controller
        present(controller)
    }

    // MARK: Tab actions

    @objc func newTab(_ sender: Any?) {
        _ = session.newTab(side: layout.active, path: nil)
    }

    @objc func closeTab(_ sender: Any?) {
        close(tab: activeTab)
    }

    private func close(tab: UInt64) {
        switch session.closeTab(tab: tab) {
        case .lastTab: window?.performClose(nil)
        case .pinned: NSSound.beep()
        case .closed, .missing: break
        }
    }

    @objc func closeOtherTabs(_ sender: Any?) { session.closeOtherTabs(side: layout.active) }
    @objc func nextTab(_ sender: Any?) { session.cycleTab(side: layout.active, delta: 1) }
    @objc func previousTab(_ sender: Any?) { session.cycleTab(side: layout.active, delta: -1) }
    @objc func moveTabToOtherPane(_ sender: Any?) { session.moveTabToOtherPane(tab: tab(from: sender)) }

    @objc func selectTabNumber(_ sender: NSMenuItem) {
        session.selectTabIndex(side: layout.active, index: UInt32(sender.tag))
    }

    @objc func togglePinTab(_ sender: Any?) {
        let id = tab(from: sender)
        let pinned = (layout.left.tabs + layout.right.tabs).first { $0.id == id }?.pinned ?? false
        session.setPinned(tab: id, pinned: !pinned)
    }

    @objc func closeTabFromMenu(_ sender: Any?) { close(tab: tab(from: sender)) }

    @objc func closeOtherTabsFromMenu(_ sender: Any?) {
        let id = tab(from: sender)
        guard let side = side(of: id) else { return }
        session.selectTab(tab: id)
        session.closeOtherTabs(side: side)
    }

    @objc func openTabInNewWindow(_ sender: Any?) {
        let id = tab(from: sender)
        guard let path = (layout.left.tabs + layout.right.tabs).first(where: { $0.id == id })?.path,
              let delegate = NSApp.delegate as? AppDelegate
        else { return }
        delegate.openWindow(restoring: nil, leftPath: path)
    }

    /// The tab a context-menu item was built for, else the active tab.
    private func tab(from sender: Any?) -> UInt64 {
        ((sender as? NSMenuItem)?.representedObject as? UInt64) ?? activeTab
    }

    // MARK: Pane and view actions

    @objc func switchPane(_ sender: Any?) { session.switchActive() }
    @objc func toggleSinglePane(_ sender: Any?) { session.toggleSinglePane() }
    @objc func toggleHorizontalPanes(_ sender: Any?) { session.setHorizontal(horizontal: !layout.horizontal) }
    @objc func swapPanes(_ sender: Any?) { session.swapPanes() }
    @objc func clonePane(_ sender: Any?) { session.clonePane() }
    @objc func toggleSyncBrowsing(_ sender: Any?) { session.toggleSyncBrowsing() }

    @objc func toggleHidden(_ sender: Any?) {
        (NSApp.delegate as? AppDelegate)?.setShowHidden(!AppEnvironment.shared.settings.showHidden)
    }

    @objc func toggleSidebarVisible(_ sender: Any?) {
        sidebarItem.animator().isCollapsed.toggle()
        scheduleSave()
    }

    @objc func toggleTabBar(_ sender: Any?) {
        AppEnvironment.shared.update { $0.alwaysShowTabBar.toggle() }
    }

    @objc func toggleAlternatingRows(_ sender: Any?) {
        AppEnvironment.shared.update { $0.alternatingRows.toggle() }
    }

    @objc func addToFavorites(_ sender: Any?) {
        // The clicked item (path bar), else the one selected item (file or
        // folder), else the current location.
        let selected = activeList?.selectedPaths.count == 1 ? activeList?.selectedPaths.first : nil
        let path = ((sender as? NSMenuItem)?.representedObject as? String) ?? selected ?? activeList?.content.path
        guard let path else { return }
        if activeList?.isRemote == true {
            return addRemotePathToFavorites(path)
        }
        let name = FileManager.default.displayName(atPath: path)
        askFavorite(.path(name.isEmpty ? path.folderDisplayName : name, path))
    }

    @objc func copyPath(_ sender: NSMenuItem) {
        guard let path = sender.representedObject as? String else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(path, forType: .string)
    }

    @objc func openPathInNewTab(_ sender: NSMenuItem) {
        guard let path = sender.representedObject as? String else { return }
        _ = session.newTab(side: layout.active, path: path)
    }

    // MARK: Session

    private func restoreUI(_ ui: String?) {
        guard let window else { return }
        var split = 0.5  // panes start equal
        if let ui, let data = ui.data(using: .utf8),
           let state = try? JSONDecoder().decode(UIState.self, from: data)
        {
            let frame = NSRectFromString(state.frame)
            if frame.width > 100, frame.height > 100 { window.setFrame(frame, display: false) }
            sidebarItem.isCollapsed = state.sidebarCollapsed
            window.toolbar?.isVisible = state.toolbarVisible
            if let hidden = state.titleHidden { setTitleHidden(hidden) }
            if let hidden = state.pathBarHidden { setPathBarHidden(hidden) }
            if state.fullScreen == true {
                // Full screen once the window is on screen.
                DispatchQueue.main.async { [weak self] in
                    guard let window = self?.window, !window.styleMask.contains(.fullScreen) else { return }
                    window.toggleFullScreen(nil)
                }
            }
            previewItem.isCollapsed = !(state.previewVisible ?? false)
            if let mode = state.previewMode.flatMap(PreviewPaneViewController.Mode.init(rawValue:)), !previewItem.isCollapsed {
                preview.show(mode: mode)
            }
            if let saved = state.paneSplit, saved > 0.05, saved < 0.95 { split = saved }
        }
        // Applied after the first layout pass, when the split view has a size.
        DispatchQueue.main.async { [weak self] in
            guard let self, !self.layout.singlePane else { return }
            let view = self.panesSplit.splitView
            let length = view.isVertical ? view.bounds.width : view.bounds.height
            view.setPosition(length * split, ofDividerAt: 0)
        }
    }

    func savedJSON() -> String {
        let view = panesSplit.splitView
        let length = view.isVertical ? view.bounds.width : view.bounds.height
        let first = view.arrangedSubviews.first.map { view.isVertical ? $0.frame.width : $0.frame.height } ?? 0
        let state = UIState(
            frame: NSStringFromRect(window?.frame ?? .zero),
            sidebarCollapsed: sidebarItem.isCollapsed,
            toolbarVisible: window?.toolbar?.isVisible ?? true,
            paneSplit: layout.singlePane || length <= 0 ? nil : Double(first / length),
            previewVisible: !previewItem.isCollapsed,
            previewMode: preview.mode.rawValue,
            fullScreen: window?.styleMask.contains(.fullScreen) == true,
            titleHidden: chrome.titleHidden,
            pathBarHidden: chrome.pathBarHidden)
        let ui = (try? JSONEncoder().encode(state)).flatMap { String(data: $0, encoding: .utf8) } ?? ""
        return session.savedJson(ui: ui)
    }

    /// Save the session shortly after a change, so a crash loses little.
    func scheduleSave() {
        guard !saveScheduled else { return }
        saveScheduled = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) { [weak self] in
            guard let self else { return }
            self.saveScheduled = false
            if self.window?.isVisible == true { (NSApp.delegate as? AppDelegate)?.saveSession() }
        }
    }

    func windowWillClose(_ notification: Notification) {
        preview.tearDown()
        onClose?()
    }

    // MARK: Toolbar

    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [.toggleSidebar, .sidebarTrackingSeparator, Item.back, Item.forward, Item.up, .flexibleSpace,
         Item.copyTo, Item.moveTo, Item.delete]
            + areaDefaultToolbarItems
            + [Item.activities, Item.groupBy, Item.singlePane, Item.hidden, Item.reload, SearchFieldController.itemId]
    }

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        toolbarDefaultItemIdentifiers(toolbar) + [.space] + areaAllowedToolbarItems
    }

    func toolbar(
        _ toolbar: NSToolbar, itemForItemIdentifier id: NSToolbarItem.Identifier,
        willBeInsertedIntoToolbar flag: Bool
    ) -> NSToolbarItem? {
        switch id {
        case Item.back: return make(id, "Back", "chevron.left", #selector(goBack(_:)), navigational: true)
        case Item.forward: return make(id, "Forward", "chevron.right", #selector(goForward(_:)), navigational: true)
        case Item.up: return make(id, "Enclosing Folder", "arrow.up", #selector(goUp(_:)), navigational: true)
        case Item.reload: return make(id, "Refresh", "arrow.clockwise", #selector(reload(_:)))
        case Item.hidden: return make(id, "Show Invisible Files", "eye", #selector(toggleHidden(_:)))
        case Item.singlePane: return make(id, "Single Pane Mode", "rectangle.split.2x1", #selector(toggleSinglePane(_:)))
        case Item.activities: return makeActivitiesItem(id)
        case Item.copyTo: return make(id, "Copy to", "doc.on.doc", #selector(copyToOtherPane(_:)))
        case Item.moveTo: return make(id, "Move to", "arrow.right.doc.on.clipboard", #selector(moveToOtherPane(_:)))
        case Item.delete:
            let item = make(id, "Delete", "trash", #selector(toolbarDelete(_:)))
            item.toolTip = "Move to Trash (Option-click: Delete Immediately)"
            return item
        case Item.groupBy:
            let item = NSMenuToolbarItem(itemIdentifier: id)
            item.label = "Group By"
            item.toolTip = "Group By (Option-click: Sort By)"
            item.image = NSImage(systemSymbolName: "square.grid.3x1.below.line.grid.1x2", accessibilityDescription: "Group By")
            item.showsIndicator = true
            let menu = NSMenu(title: "Group By")
            menu.delegate = groupByFiller
            item.menu = menu
            return item
        case SearchFieldController.itemId: return searchField.item
        default: return areaToolbarItem(id)
        }
    }

    /// Button that opens the activity list; a spinner beside it turns
    /// while anything is running.
    private func makeActivitiesItem(_ id: NSToolbarItem.Identifier) -> NSToolbarItem {
        let item = NSToolbarItem(itemIdentifier: id)
        item.label = "Activities"
        item.toolTip = "Activities"
        activitiesButton.image = NSImage(systemSymbolName: "arrow.up.arrow.down.circle", accessibilityDescription: "Activities")
        activitiesButton.bezelStyle = .toolbar
        activitiesButton.target = self
        activitiesButton.action = #selector(showActivities(_:))
        activitiesSpinner.style = .spinning
        activitiesSpinner.controlSize = .small
        activitiesSpinner.isDisplayedWhenStopped = false
        activitiesSpinner.isHidden = true
        let row = NSStackView(views: [activitiesSpinner, activitiesButton])
        row.orientation = .horizontal
        row.spacing = 4
        item.view = row
        return item
    }

    private func make(
        _ id: NSToolbarItem.Identifier, _ label: String, _ symbol: String, _ action: Selector,
        navigational: Bool = false
    ) -> NSToolbarItem {
        let item = NSToolbarItem(itemIdentifier: id)
        item.label = label
        item.toolTip = label
        item.image = NSImage(systemSymbolName: symbol, accessibilityDescription: label)
        item.target = self
        item.action = action
        item.isBordered = true
        item.isNavigational = navigational
        return item
    }
}

private extension WindowLayout {
    /// True if `side` is the pane hidden by single-pane mode.
    func single_pane_hides(_ side: PaneSide) -> Bool {
        singlePane && active != side
    }
}

// MARK: - Pane callbacks

extension MainWindowController: PaneContainerDelegate {
    func pane(_ side: PaneSide, select tab: UInt64) { session.selectTab(tab: tab) }
    func pane(_ side: PaneSide, close tab: UInt64) { close(tab: tab) }
    func paneNewTab(_ side: PaneSide) { _ = session.newTab(side: side, path: nil) }

    func pane(_ side: PaneSide, navigate path: String) {
        session.setActive(side: side)
        session.navigate(tab: currentTab(side), path: path)
    }

    func pane(_ side: PaneSide, menuFor tab: UInt64) -> NSMenu? {
        let pinned = tabs(side).tabs.first { $0.id == tab }?.pinned ?? false
        let menu = NSMenu()
        func add(_ title: String, _ action: Selector) {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
            item.target = self
            item.representedObject = tab
            menu.addItem(item)
        }
        add("Close Tab", #selector(closeTabFromMenu(_:)))
        add("Close Other Tabs", #selector(closeOtherTabsFromMenu(_:)))
        menu.addItem(.separator())
        add(pinned ? "Unpin Tab" : "Pin Tab", #selector(togglePinTab(_:)))
        add("Move Tab to Other Pane", #selector(moveTabToOtherPane(_:)))
        add("Open in New Window", #selector(openTabInNewWindow(_:)))
        return menu
    }

    /// Every mounted volume, so any of them is one click away.
    func paneVolumesMenu(_ side: PaneSide) -> NSMenu {
        let menu = NSMenu()
        let keys: [URLResourceKey] = [.volumeNameKey]
        let volumes = FileManager.default.mountedVolumeURLs(includingResourceValuesForKeys: keys, options: [.skipHiddenVolumes]) ?? []
        for url in volumes {
            let name = (try? url.resourceValues(forKeys: Set(keys)))?.volumeName ?? url.lastPathComponent
            let item = NSMenuItem(title: name, action: #selector(volumeChosen(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = [side == .left ? "left" : "right", url.path]
            let icon = RowPresenter.shared.folderIcon(atPath: url.path).copy() as? NSImage
            icon?.size = NSSize(width: 16, height: 16)
            item.image = icon
            menu.addItem(item)
        }
        // Open remote locations, so a server is one click away as well.
        let servers = ConnectionCenter.shared.connections
        if !servers.isEmpty {
            menu.addItem(.separator())
            for connection in servers {
                let item = NSMenuItem(title: connection.label, action: #selector(connectionChosen(_:)), keyEquivalent: "")
                item.target = self
                item.representedObject = [side == .left ? "left" : "right", String(connection.id)]
                item.image = NSImage(systemSymbolName: "network", accessibilityDescription: nil)
                menu.addItem(item)
            }
        }
        return menu
    }

    @objc private func connectionChosen(_ sender: NSMenuItem) {
        guard let info = sender.representedObject as? [String], info.count == 2, let id = UInt64(info[1]),
              let connection = ConnectionCenter.shared.connections.first(where: { $0.id == id })
        else { return }
        let side: PaneSide = info[0] == "left" ? .left : .right
        session.setActive(side: side)
        session.navigateTo(tab: currentTab(side), conn: connection.id, path: connection.home)
    }

    @objc private func volumeChosen(_ sender: NSMenuItem) {
        guard let info = sender.representedObject as? [String], info.count == 2 else { return }
        pane(info[0] == "left" ? .left : .right, navigate: info[1])
    }

    func pane(_ side: PaneSide, pathMenuFor path: String) -> NSMenu? {
        let menu = NSMenu()
        func add(_ title: String, _ action: Selector) {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
            item.target = self
            item.representedObject = path
            menu.addItem(item)
        }
        add("Copy Path", #selector(copyPath(_:)))
        add("Open in New Tab", #selector(openPathInNewTab(_:)))
        add("Add Path to Favorite", #selector(addToFavorites(_:)))
        return menu
    }
}

// MARK: - Menu and toolbar state

extension MainWindowController: NSUserInterfaceValidations {
    func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
        let menuItem = item as? NSMenuItem
        let info = activeList?.content.info
        let settings = AppEnvironment.shared.settings
        func check(_ on: Bool) { menuItem?.state = on ? .on : .off }
        // Nothing acts on the panes behind a sheet or during a rename.
        let blocked = window?.attachedSheet != nil || activeList?.isRenaming == true

        switch item.action {
        case #selector(copyToOtherPane(_:)), #selector(moveToOtherPane(_:)):
            // Nothing goes into an archive, and nothing is moved out of one.
            if otherList?.isArchive == true { return false }
            if item.action == #selector(moveToOtherPane(_:)), activeList?.isArchive == true { return false }
            return !blocked && otherPanePath != nil && !(activeList?.selectedPaths.isEmpty ?? true)
        case #selector(ejectVolume(_:)):
            return !blocked && volumeToEject != nil
        case #selector(toolbarDelete(_:)):
            return !blocked && activeList?.isArchive != true && !(activeList?.selectedPaths.isEmpty ?? true)
        case #selector(makeAliasInOtherPane(_:)):
            return !blocked && canLinkInOtherPane(alias: true)
        case #selector(makeSymlinkInOtherPane(_:)):
            return !blocked && canLinkInOtherPane(alias: false)
        case #selector(copyToAs(_:)):
            return !blocked && otherPanePath != nil && activeList?.selectedPaths.count == 1
                && otherList?.isArchive != true
        case #selector(showArchiveContents(_:)):
            return !blocked && activeList?.selectedPaths.count == 1 && selectedArchives.count == 1
        case #selector(extractHere(_:)):
            return !blocked && !selectedArchives.isEmpty && activeList?.isArchive != true
        case #selector(extractTo(_:)):
            return !blocked && !selectedArchives.isEmpty && otherPanePath != nil && otherList?.isArchive != true
        case #selector(compress(_:)), #selector(compressWithOptions(_:)):
            return !blocked && compressPlan != nil
        case #selector(updateWorkspace(_:)):
            return !blocked && workspaceId != nil
        case #selector(saveWorkspace(_:)):
            return !blocked
        case #selector(runTool(_:)):
            return !blocked && activeList?.content.path != nil
        case #selector(openInTerminal(_:)):
            return !blocked && (activeList?.isRemote == false || activeList.map(canOpenRemoteTerminal) == true)
        case #selector(checksum(_:)):
            return !blocked && activeList?.inspectedSelection.contains { !$0.row.isDir } == true
        case #selector(toggleScmStatus(_:)):
            check(Prefs.bool("view.scmStatus"))
            return true
        case #selector(scmAdd(_:)):
            return !blocked && activeList?.isRemote == false && !(activeList?.selectedPaths.isEmpty ?? true)
        case #selector(scmAddAll(_:)), #selector(scmCommit(_:)), #selector(scmPull(_:)), #selector(scmPush(_:)):
            return !blocked && activeList?.isRemote == false
        case #selector(find(_:)):
            // Without the field in the toolbar there is nowhere to type.
            return window?.attachedSheet == nil && hasSearchField
        case #selector(syncTo(_:)):
            // Needs both panes on screen, each showing a folder, not
            // search results.
            return !blocked && otherList?.content.path != nil && activeList?.content.path != nil
                && otherList?.search == nil && activeList?.search == nil
        case #selector(compare(_:)):
            return !blocked && compareTargets != nil
        case #selector(getInfo(_:)):
            return !blocked && !(activeList?.selectedPaths.isEmpty ?? true)
        case #selector(disconnect(_:)):
            return !blocked && activeList?.isRemote == true
        case #selector(connect(_:)), #selector(quickOpen(_:)), #selector(goToFolder(_:)),
             #selector(quickSelect(_:)), #selector(showDevices(_:)):
            return window?.attachedSheet == nil
        case #selector(addToFavorites(_:)):
            return !blocked && activeList?.isArchive != true
        case #selector(togglePreview(_:)):
            menuItem?.title = previewItem.isCollapsed ? "Show Preview" : "Hide Preview"
            return true
        case #selector(emptyTrash(_:)):
            return !blocked
        case #selector(goBack(_:)): return info?.canGoBack ?? false
        case #selector(goForward(_:)): return info?.canGoForward ?? false
        case #selector(goUp(_:)): return info?.canGoUp ?? false
        case #selector(closeOtherTabs(_:)): return tabs(layout.active).tabs.count > 1
        case #selector(nextTab(_:)), #selector(previousTab(_:)): return tabs(layout.active).tabs.count > 1
        case #selector(selectTabNumber(_:)):
            return (menuItem?.tag ?? 0) < tabs(layout.active).tabs.count
        case #selector(switchPane(_:)), #selector(swapPanes(_:)), #selector(clonePane(_:)):
            return !layout.singlePane
        case #selector(togglePinTab(_:)):
            let pane = tabs(layout.active)
            menuItem?.title = pane.tabs[Int(pane.selected)].pinned ? "Unpin Tab" : "Pin Tab"
            return true
        case #selector(toggleHidden(_:)):
            check(settings.showHidden)
            return true
        case #selector(toggleSinglePane(_:)):
            check(layout.singlePane)
            return true
        case #selector(toggleHorizontalPanes(_:)):
            menuItem?.title = layout.horizontal ? "Show Vertical Panes" : "Show Horizontal Panes"
            return !layout.singlePane
        case #selector(toggleSyncBrowsing(_:)):
            check(layout.syncBrowsing)
            return true
        case #selector(toggleSidebarVisible(_:)):
            menuItem?.title = sidebarItem.isCollapsed ? "Show Sidebar" : "Hide Sidebar"
            return true
        case #selector(toggleTabBar(_:)):
            menuItem?.title = settings.alwaysShowTabBar ? "Hide Tab Bar" : "Show Tab Bar"
            return true
        case #selector(toggleAlternatingRows(_:)):
            check(settings.alternatingRows)
            return true
        default:
            return validateChrome(item) ?? true
        }
    }
}

// MARK: - Scripted checks

extension MainWindowController {
    var debugPreview: PreviewPaneViewController { preview }
    var debugConnectSheet: ConnectSheetController? { connectSheet }
    var debugPalette: PaletteViewController? { palette }
    var debugQuickSelect: QuickSelectViewController? { quickSelectSheet }
    var debugSearch: SearchFieldController { searchField }
    var debugActiveList: FileListViewController? { activeList }
    var debugActivePane: PaneContainerViewController { layout.active == .left ? left : right }

    /// Connect by host alias without the panel, accepting an unknown host
    /// key, and show the server in the active pane.
    func debugConnect(_ text: String) {
        let parsed = parseConnectionUrl(text: text)
        // As the Connect panel does: "Open remote connections in".
        let side = remoteSide
        let params = ServerParams(
            protocol: parsed?.protocol ?? .sftp, host: parsed?.host ?? text, port: parsed?.port,
            user: parsed?.user ?? "", password: parsed?.password,
            remotePath: parsed.flatMap { $0.path.isEmpty ? nil : $0.path }, passive: true, encoding: nil,
            acceptInvalidCerts: false, region: nil)
        // Shares are mounted by the system, not spoken by RustyFinder.
        let shares: [RemoteProtocol: String] = [.smb: "smb", .afp: "afp", .nfs: "nfs", .vnc: "vnc"]
        if let scheme = shares[params.protocol] {
            openShare(params, scheme: scheme, side: side) { [weak self] message in
                if let message { self?.showOperationProblem(title: "Connecting", message: message, partial: false) }
            }
            return
        }
        ConnectionCenter.shared.connect(params, window: window, scriptedAnswers: [.hostKey: "yes"]) { [weak self] result in
            switch result {
            case .success(let connection):
                guard let self else { return }
                self.session.setActive(side: side)
                self.session.navigateTo(tab: self.currentTab(side), conn: connection.id, path: connection.home)
            case .failure(let failure):
                self?.showOperationProblem(title: "Connecting", message: failure.message, partial: false)
            }
        }
    }
}

extension MainWindowController: DebugDumping {
    func debugDump() -> String {
        func describe(_ pane: PaneTabsRecord) -> String {
            pane.tabs.enumerated().map { i, tab in
                let name = searchTitles[tab.id].map { "Search: “\($0)”" } ?? (tab.path as NSString).lastPathComponent
                return (i == Int(pane.selected) ? "*" : "") + name + (tab.pinned ? "(pinned)" : "")
            }.joined(separator: "|")
        }
        var lines = [
            "title: \(window?.title ?? "")",
            "active: \(layout.active == .left ? "left" : "right")",
            "layout: \(layout.singlePane ? "single" : "dual") \(layout.horizontal ? "horizontal" : "vertical") sync=\(layout.syncBrowsing)",
            "left.tabs: \(describe(layout.left))",
            "right.tabs: \(describe(layout.right))",
            "tabbar: left=\(left.isTabBarVisible) right=\(right.isTabBarVisible)",
            "sidebar: \(sidebarItem.isCollapsed ? "hidden" : "shown") \(sidebar.debugDump())",
            "focus: \(window?.firstResponder === activeList?.primaryView ? "active-list" : "elsewhere")",
        ]
        let core = AppEnvironment.shared.core
        lines += [
            "undo: \(core.undoTitle(window: session.id()) ?? "-")",
            "redo: \(core.redoTitle(window: session.id()) ?? "-")",
            "problem: \(problemAlert.map { "\($0.messageText) \($0.informativeText)" } ?? "-")",
        ]
        lines += ActivityCenter.shared.debugDump()
        lines.append("preview: \(previewItem.isCollapsed ? "hidden" : "shown")")
        if !previewItem.isCollapsed { lines += preview.debugDump() }
        lines.append("infowindows: \(InfoWindowController.openCount)")
        lines.append("connections: \(ConnectionCenter.shared.connections.map(\.label).joined(separator: ","))")
        lines.append("subtitle: \(window?.subtitle ?? "")")
        lines.append("ssh.prompt: \(ConnectionCenter.shared.lastPrompt.replacingOccurrences(of: "\n", with: " ").prefix(200))")
        lines.append("remote.editing: \(RemoteFiles.shared.debugWatched.joined(separator: ","))")
        lines += RemoteFiles.shared.debugDump()
        lines.append("connect.decision: \(ConnectionCenter.shared.lastDecision)")
        lines.append("connectsheet: \(connectSheet?.debugDump() ?? "-")")
        lines.append("palette: \(palette?.debugDump() ?? "-")")
        lines.append("quickselect: \(quickSelectSheet?.debugCount ?? "-")")
        lines.append("workspaces: \(core.workspaces().map(\.name).joined(separator: "|")) from=\(workspaceId.map(String.init) ?? "-")")
        lines.append("windows: \((NSApp.delegate as? AppDelegate)?.windowControllers.count ?? 0)")
        lines.append("settingswindow: \(SettingsWindowController.current?.debugDump() ?? "-")")
        lines.append("viewoptions: \(ViewOptionsPanel.shared.debugState)")
        lines.append("shortcuts: \(ShortcutCenter.shared.customSummary)")
        lines.append("shortcuts.bare: \(ShortcutCenter.shared.bareSummary)")
        lines.append("tool.output: \(ToolCenter.shared.lastOutput)")
        lines.append("tool.windows: \(OutputWindowController.openCount)")
        lines.append("toolswindow: \(ToolsWindowController.current?.debugDump() ?? "-")")
        lines.append("checksum: \(ChecksumWindowController.current?.debugDump() ?? "-")")
        lines.append("commitsheet: \(CommitSheet.current?.debugDump() ?? "-")")
        lines.append("external: \(ExternalApps.lastRequest)")
        lines.append("archive.prompt: \(ArchiveCenter.shared.lastPrompt)")
        lines.append("archive.problem: \(ArchiveCenter.shared.lastProblem)")
        lines.append("compresssheet: \(CompressSheet.current?.debugDump() ?? "-")")
        lines.append("searchfield: \(searchField.debugDump())")
        lines.append("multirename: \(MultiRenameWindowController.current?.debugDump() ?? "-")")
        lines.append("sync: \(SyncWindowController.current?.debugDump() ?? "-")")
        if let first = InfoWindowController.debugControllers.first {
            lines += first.debugInfo.debugDump().map { "window." + $0 }
        }
        lines += LocalOpsDebug.dumpLines()
        lines += chromeDump()
        lines += AreaDebug.dump(self)
        lines += activeList?.debugDump(prefix: "") ?? []
        lines += fileList(for: currentTab(other(layout.active)))?.debugDump(prefix: "other.") ?? []
        return lines.joined(separator: "\n")
    }
}
