import AppKit

/// One row of the sidebar: a group header or an item under it.
final class SidebarNode: NSObject {
    enum Kind {
        case devices, connections, shares, favorites(groupId: UInt64), recents, tags
        case device(ejectable: Bool), connection(ConnectionRecord), share, server(ShareBrowser.Server)
        case favorite(FavoriteRecord), favoritePath(FavoritePathRecord, parent: FavoriteRecord)
        case recent(conn: UInt64), tag
    }

    let kind: Kind
    let title: String
    let path: String?
    let detail: String?
    let symbol: String?
    var image: NSImage?
    var children: [SidebarNode] = []

    init(kind: Kind, title: String, path: String? = nil, detail: String? = nil, symbol: String? = nil) {
        self.kind = kind
        self.title = title
        self.path = path
        self.detail = detail
        self.symbol = symbol
    }

    var isGroup: Bool {
        switch kind {
        case .devices, .connections, .shares, .favorites, .recents, .tags: return true
        default: return false
        }
    }

    /// Key under which a built-in group's collapsed state is stored.
    var collapseKey: String? {
        switch kind {
        case .devices: return "RustyFinderSidebarDevicesCollapsed"
        case .connections: return "RustyFinderSidebarConnectionsCollapsed"
        case .shares: return "RustyFinderSidebarSharesCollapsed"
        case .recents: return "RustyFinderSidebarRecentsCollapsed"
        case .tags: return "RustyFinderSidebarTagsCollapsed"
        default: return nil
        }
    }

    /// Key used by "Hide from Sidebar" and the Sidebar Editor.
    var hideKey: String? {
        switch kind {
        case .devices: return "group:devices"
        case .connections: return "group:connections"
        case .shares: return "group:shares"
        case .recents: return "group:recents"
        case .tags: return "group:tags"
        case .favorites(let id): return "favgroup:\(id)"
        case .device: return path.map { "device:\($0)" }
        case .share: return path.map { "share:\($0)" }
        case .server(let server): return "share:\(server.name)"
        default: return nil
        }
    }

    /// A local folder that items can be dropped into.
    var dropFolder: String? {
        switch kind {
        case .device, .share, .recent(conn: 0):
            return path
        case .favorite(let f) where f.kind == .path:
            var isDir: ObjCBool = false
            guard FileManager.default.fileExists(atPath: f.path, isDirectory: &isDir), isDir.boolValue,
                  !NSWorkspace.shared.isFilePackage(atPath: f.path)
            else { return nil }
            return f.path
        default:
            return nil
        }
    }

    /// An application in the sidebar: dropped items open with it.
    var application: String? {
        guard case .favorite(let f) = kind, f.kind == .path, f.path.hasSuffix(".app") else { return nil }
        return f.path
    }
}

/// Devices, connections, shares, favorites, recent folders and tags.
/// Clicking opens in the active pane; Option-click in the other pane.
final class SidebarViewController: NSViewController, NSOutlineViewDataSource, NSOutlineViewDelegate, NSMenuDelegate {
    var onOpen: ((_ path: String, _ otherPane: Bool) -> Void)?
    var onOpenInNewTab: ((String) -> Void)?
    var onOpenConnection: ((_ connection: ConnectionRecord, _ otherPane: Bool) -> Void)?
    /// A tag was clicked: list everything that carries it.
    var onOpenTag: ((String) -> Void)?
    /// The window the sidebar is in, for favorites, drops and ejecting.
    weak var host: MainWindowController?
    /// The data source whose Recent Folders are listed (0: this Mac).
    var recentsConnection: UInt64 = 0 {
        didSet { if recentsConnection != oldValue { reloadRecents() } }
    }

    private let outline = NSOutlineView()
    private var groups: [SidebarNode] = []
    private var isReloading = false
    private static let dragType = NSPasteboard.PasteboardType("dev.rustyfinder.sidebar")
    /// The last drop accepted (scripted checks).
    private(set) var lastDrop = "-"

    override func loadView() {
        let scroll = NSScrollView()
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true

        let column = NSTableColumn(identifier: .init("sidebar"))
        outline.addTableColumn(column)
        outline.outlineTableColumn = column
        outline.headerView = nil
        outline.style = .sourceList
        outline.rowSizeStyle = .default
        outline.floatsGroupRows = false
        outline.dataSource = self
        outline.delegate = self
        outline.target = self
        outline.action = #selector(rowClicked(_:))
        outline.setAccessibilityLabel("Sidebar")
        outline.registerForDraggedTypes([Self.dragType, .fileURL])
        outline.setDraggingSourceOperationMask(.move, forLocal: true)
        let menu = NSMenu()
        menu.delegate = self
        outline.menu = menu

        scroll.documentView = outline
        view = scroll
        view.widthAnchor.constraint(greaterThanOrEqualToConstant: 150).isActive = true

        let center = NotificationCenter.default
        for name in [Notification.Name.rustyfinderFavoritesChanged, .rustyfinderConnectionsChanged, .rustyfinderSidebarChanged, .rustyfinderTagsChanged] {
            center.addObserver(self, selector: #selector(reload), name: name, object: nil)
        }
        let workspace = NSWorkspace.shared.notificationCenter
        for name in [NSWorkspace.didMountNotification, NSWorkspace.didUnmountNotification, NSWorkspace.didRenameVolumeNotification] {
            workspace.addObserver(self, selector: #selector(reload), name: name, object: nil)
        }
        ShareBrowser.shared.start()
        reload()
    }

    // MARK: Data

    @objc func reload() {
        let core = AppEnvironment.shared.core
        let hidden = SidebarVisibility.hidden
        var result: [SidebarNode] = []

        // Volumes on this Mac under Devices; network mounts under Shares.
        let devices = SidebarNode(kind: .devices, title: "Devices")
        let shares = SidebarNode(kind: .shares, title: "Shares")
        let keys: [URLResourceKey] = [.volumeNameKey, .volumeIsEjectableKey, .volumeIsRemovableKey, .volumeIsInternalKey, .volumeIsLocalKey]
        let volumes = FileManager.default.mountedVolumeURLs(includingResourceValuesForKeys: keys, options: [.skipHiddenVolumes]) ?? []
        for url in SidebarVisibility.ordered(volumes, path: \.path) {
            let values = try? url.resourceValues(forKeys: Set(keys))
            let free = core.space(path: url.path).map { "\(RowPresenter.shared.bytes($0.free)) free" }
            let name = values?.volumeName ?? url.lastPathComponent
            if values?.volumeIsLocal == false {
                shares.children.append(SidebarNode(kind: .share, title: name, path: url.path, detail: free, symbol: "externaldrive.connected.to.line.below"))
                continue
            }
            let ejectable = (values?.volumeIsEjectable ?? false) || (values?.volumeIsRemovable ?? false)
            devices.children.append(SidebarNode(
                kind: .device(ejectable: ejectable), title: name, path: url.path, detail: free,
                symbol: (values?.volumeIsInternal ?? true) ? "internaldrive" : "externaldrive"))
        }
        for server in ShareBrowser.shared.servers where !shares.children.contains(where: { $0.title == server.name }) {
            shares.children.append(SidebarNode(kind: .server(server), title: server.name, detail: server.scheme.uppercased(), symbol: "server.rack"))
        }
        devices.children.removeAll { $0.hideKey.map(hidden.contains) ?? false }
        shares.children.removeAll { $0.hideKey.map(hidden.contains) ?? false }
        result.append(devices)

        // Open remote sessions; the group only shows while there are some.
        let open = core.connections().filter { !$0.archive }
        if !open.isEmpty {
            let connections = SidebarNode(kind: .connections, title: "Connections")
            connections.children = open.map {
                SidebarNode(kind: .connection($0), title: $0.label, detail: nil, symbol: "network")
            }
            result.append(connections)
        }
        if !shares.children.isEmpty { result.append(shares) }

        for group in core.favoriteGroups() {
            let node = SidebarNode(kind: .favorites(groupId: group.id), title: group.name)
            node.children = group.items.map { favorite in
                let item = SidebarNode(
                    kind: .favorite(favorite), title: favorite.name,
                    path: favorite.kind == .path ? favorite.path : nil)
                item.image = FavoriteIcons.image(for: favorite)
                item.children = favorite.children.map {
                    SidebarNode(kind: .favoritePath($0, parent: favorite), title: $0.name, detail: nil, symbol: "folder")
                }
                return item
            }
            result.append(node)
        }

        let recents = SidebarNode(kind: .recents, title: "Recent Folders")
        recents.children = recentNodes()
        result.append(recents)

        let tags = SidebarNode(kind: .tags, title: "Tags")
        tags.children = TagStore.sidebarTags.map { SidebarNode(kind: .tag, title: $0, symbol: "tag") }
        result.append(tags)

        let collapsedFavorites = Set(core.favoriteGroups().filter(\.collapsed).map(\.id))
        isReloading = true
        groups = result.filter { !($0.hideKey.map(hidden.contains) ?? false) }
        outline.reloadData()
        for group in groups {
            let collapsed: Bool
            switch group.kind {
            case .favorites(let id): collapsed = collapsedFavorites.contains(id)
            default: collapsed = group.collapseKey.map { UserDefaults.standard.bool(forKey: $0) } ?? false
            }
            if !collapsed { outline.expandItem(group) }
            // Favorite paths under a server favorite start folded open.
            for child in group.children where !child.children.isEmpty { outline.expandItem(child) }
        }
        isReloading = false
    }

    private func recentNodes() -> [SidebarNode] {
        let conn = recentsConnection
        return AppEnvironment.shared.core.recentFoldersOn(connection: conn).map {
            SidebarNode(kind: .recent(conn: conn), title: conn == 0 ? $0.folderDisplayName : (($0 as NSString).lastPathComponent.isEmpty ? $0 : ($0 as NSString).lastPathComponent),
                        path: $0, symbol: "clock")
        }
    }

    /// Only the Recent Folders group changes with navigation; rebuilding it
    /// alone keeps the rest of the sidebar steady.
    func reloadRecents() {
        guard let recents = groups.first(where: { if case .recents = $0.kind { return true } else { return false } })
        else { return }
        let fresh = recentNodes()
        guard fresh.compactMap(\.path) != recents.children.compactMap(\.path) || {
            if case .recent(let c) = recents.children.first?.kind { return c != recentsConnection }
            return false
        }() else { return }
        recents.children = fresh
        outline.reloadItem(recents, reloadChildren: true)
    }

    /// Select the row for `path` if the sidebar has one, else clear the selection.
    func highlight(path: String?) {
        let match = groups.flatMap(\.children).first { node in
            if case .recent(let conn) = node.kind, conn != 0 { return false }
            return node.path == path
        }
        let row = match.map { outline.row(forItem: $0) } ?? -1
        if row >= 0 {
            outline.selectRowIndexes([row], byExtendingSelection: false)
        } else {
            outline.deselectAll(nil)
        }
    }

    /// Keep a tag selected while its listing is shown.
    func highlight(tag: String) {
        guard let node = groups.flatMap(\.children).first(where: { if case .tag = $0.kind { return $0.title == tag } else { return false } })
        else { return }
        let row = outline.row(forItem: node)
        if row >= 0 { outline.selectRowIndexes([row], byExtendingSelection: false) }
    }

    // MARK: Outline

    func outlineView(_ outlineView: NSOutlineView, numberOfChildrenOfItem item: Any?) -> Int {
        (item as? SidebarNode)?.children.count ?? groups.count
    }

    func outlineView(_ outlineView: NSOutlineView, child index: Int, ofItem item: Any?) -> Any {
        (item as? SidebarNode)?.children[index] ?? groups[index]
    }

    func outlineView(_ outlineView: NSOutlineView, isItemExpandable item: Any) -> Bool {
        guard let node = item as? SidebarNode else { return false }
        return node.isGroup || !node.children.isEmpty
    }

    func outlineView(_ outlineView: NSOutlineView, isGroupItem item: Any) -> Bool {
        (item as? SidebarNode)?.isGroup ?? false
    }

    func outlineView(_ outlineView: NSOutlineView, shouldSelectItem item: Any) -> Bool {
        !((item as? SidebarNode)?.isGroup ?? true)
    }

    func outlineView(_ outlineView: NSOutlineView, viewFor tableColumn: NSTableColumn?, item: Any) -> NSView? {
        guard let node = item as? SidebarNode else { return nil }
        let id = NSUserInterfaceItemIdentifier(node.isGroup ? "group" : "item")
        let cell = (outlineView.makeView(withIdentifier: id, owner: self) as? NSTableCellView) ?? makeCell(id: id, group: node.isGroup)
        let theme = ThemeCenter.shared
        if node.isGroup {
            cell.textField?.stringValue = node.title
            cell.textField?.textColor = theme.sidebarText.map { $0.withAlphaComponent(0.7) } ?? .secondaryLabelColor
            return cell
        }
        let text = NSMutableAttributedString(string: node.title, attributes: theme.sidebarText.map { [.foregroundColor: $0] } ?? [:])
        if let detail = node.detail {
            text.append(NSAttributedString(string: "  \(detail)", attributes: [
                .foregroundColor: NSColor.tertiaryLabelColor,
                .font: NSFont.systemFont(ofSize: NSFont.smallSystemFontSize - 1),
            ]))
        }
        cell.textField?.attributedStringValue = text
        cell.imageView?.image = node.image ?? node.symbol.flatMap { NSImage(systemSymbolName: $0, accessibilityDescription: nil) }
            ?? node.path.map { RowPresenter.shared.folderIcon(atPath: $0) }
        // A colour tag's icon takes its colour.
        cell.imageView?.contentTintColor = { if case .tag = node.kind { return TagStore.color(for: node.title) } else { return nil } }()
        cell.toolTip = node.path?.abbreviatingHome
        return cell
    }

    private func makeCell(id: NSUserInterfaceItemIdentifier, group: Bool) -> NSTableCellView {
        let cell = NSTableCellView()
        cell.identifier = id
        let text = NSTextField(labelWithString: "")
        text.lineBreakMode = .byTruncatingTail
        text.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(text)
        cell.textField = text
        if group {
            NSLayoutConstraint.activate([
                text.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 2),
                text.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -2),
                text.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
            return cell
        }
        let image = NSImageView()
        image.imageScaling = .scaleProportionallyUpOrDown
        image.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(image)
        cell.imageView = image
        NSLayoutConstraint.activate([
            image.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 2),
            image.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            image.widthAnchor.constraint(equalToConstant: 16),
            image.heightAnchor.constraint(equalToConstant: 16),
            text.leadingAnchor.constraint(equalTo: image.trailingAnchor, constant: 6),
            text.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -2),
            text.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        return cell
    }

    func outlineViewItemDidCollapse(_ notification: Notification) { storeCollapsed(notification, true) }
    func outlineViewItemDidExpand(_ notification: Notification) { storeCollapsed(notification, false) }

    private func storeCollapsed(_ notification: Notification, _ collapsed: Bool) {
        guard !isReloading, let node = notification.userInfo?["NSObject"] as? SidebarNode else { return }
        if case .favorites(let id) = node.kind {
            AppEnvironment.shared.core.setGroupCollapsed(groupId: id, collapsed: collapsed)
        } else if let key = node.collapseKey {
            UserDefaults.standard.set(collapsed, forKey: key)
        }
    }

    /// The theme's sidebar colours, if it has any.
    func applyTheme() {
        let theme = ThemeCenter.shared
        if let background = theme.sidebarBackground {
            outline.backgroundColor = background
            (view as? NSScrollView)?.drawsBackground = true
            (view as? NSScrollView)?.backgroundColor = background
        } else {
            outline.backgroundColor = .clear
            (view as? NSScrollView)?.drawsBackground = false
        }
        outline.reloadData()
        reload()
    }

    // MARK: Actions

    @objc private func rowClicked(_ sender: Any?) {
        guard let node = outline.item(atRow: outline.clickedRow) as? SidebarNode else { return }
        open(node, option: NSApp.currentEvent?.modifierFlags.contains(.option) ?? false)
    }

    /// What a click does; Option opens in the other pane.
    func open(_ node: SidebarNode, option: Bool) {
        switch node.kind {
        case .connection(let connection): onOpenConnection?(connection, option)
        case .tag: onOpenTag?(node.title)
        case .favorite(let favorite): host?.openFavorite(favorite, otherPane: option, newWindow: option)
        case .favoritePath(let child, let parent): host?.openFavoritePath(child, of: parent)
        case .server(let server): if let host { ShareBrowser.shared.open(server, in: host) }
        case .recent(let conn) where conn != 0:
            if let path = node.path { host?.openRecent(path, conn: conn) }
        default:
            if let path = node.path { onOpen?(path, option) }
        }
    }

    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        guard let node = outline.item(atRow: outline.clickedRow) as? SidebarNode else { return }
        func add(_ title: String, _ action: Selector, _ object: Any? = nil) {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
            item.target = self
            item.representedObject = object ?? node
            menu.addItem(item)
        }
        if node.isGroup {
            if case .favorites = node.kind {
                add("Rename Group…", #selector(menuRenameGroup(_:)))
                add("Delete Group…", #selector(menuDeleteGroup(_:)))
                add("Reveal in Favorite Manager", #selector(menuRevealInManager(_:)))
                menu.addItem(.separator())
            }
            add("Hide Group", #selector(menuHide(_:)))
            add("Hide Other Groups", #selector(menuHideOthers(_:)))
            add("Show All", #selector(menuShowAll(_:)))
            add("Sidebar Editor…", #selector(menuEditor(_:)))
            return
        }
        switch node.kind {
        case .connection:
            add("Disconnect", #selector(menuDisconnect(_:)))
            return
        case .server:
            add("Open Share…", #selector(menuOpen(_:)))
            add("Hide from Sidebar", #selector(menuHide(_:)))
            return
        case .tag:
            add("Hide from Sidebar", #selector(menuHide(_:)))
            return
        case .favorite(let favorite):
            add("Open", #selector(menuOpen(_:)))
            if favorite.kind == .path { add("Open in New Tab", #selector(menuOpenInNewTab(_:))) }
            if favorite.kind == .path { add("Open in Other Pane", #selector(menuOpenInOtherPane(_:))) }
            if favorite.kind == .workspace { add("Open in New Window", #selector(menuOpenInNewWindow(_:))) }
            if favorite.kind == .path { add("Show in Finder", #selector(menuReveal(_:))) }
            menu.addItem(.separator())
            add("Edit…", #selector(menuRevealInManager(_:)))
            add("Rename…", #selector(menuRename(_:)))
            add("Copy Link", #selector(menuCopyLink(_:)))
            add("Reveal in Favorite Manager", #selector(menuRevealInManager(_:)))
            menu.addItem(.separator())
            add("Remove from Favorites", #selector(menuRemove(_:)))
            return
        case .favoritePath:
            add("Open", #selector(menuOpen(_:)))
            add("Remove from Favorites", #selector(menuRemove(_:)))
            return
        default:
            break
        }
        guard node.path != nil else { return }
        add("Open in New Tab", #selector(menuOpenInNewTab(_:)))
        add("Open in Other Pane", #selector(menuOpenInOtherPane(_:)))
        if case .recent(let conn) = node.kind, conn != 0 { return }
        add("Show in Finder", #selector(menuReveal(_:)))
        switch node.kind {
        case .device(let ejectable):
            menu.addItem(.separator())
            if ejectable { add("Eject", #selector(menuEject(_:))) }
            add("Hide from Sidebar", #selector(menuHide(_:)))
        case .share:
            menu.addItem(.separator())
            add("Eject", #selector(menuEject(_:)))
            add("Hide from Sidebar", #selector(menuHide(_:)))
        default:
            break
        }
    }

    private func node(_ sender: Any?) -> SidebarNode? {
        (sender as? NSMenuItem)?.representedObject as? SidebarNode
    }

    @objc private func menuOpen(_ sender: Any?) {
        if let node = node(sender) { open(node, option: false) }
    }

    @objc private func menuDisconnect(_ sender: Any?) {
        guard let node = node(sender), case .connection(let connection) = node.kind else { return }
        ConnectionCenter.shared.disconnect(connection.id)
    }

    @objc private func menuOpenInNewTab(_ sender: Any?) {
        guard let node = node(sender) else { return }
        if case .favorite(let favorite) = node.kind {
            host?.openFavorite(favorite, newTab: true)
        } else if let path = node.path {
            onOpenInNewTab?(path)
        }
    }

    @objc private func menuOpenInOtherPane(_ sender: Any?) {
        guard let node = node(sender) else { return }
        if case .favorite(let favorite) = node.kind {
            host?.openFavorite(favorite, otherPane: true)
        } else if let path = node.path {
            onOpen?(path, true)
        }
    }

    @objc private func menuOpenInNewWindow(_ sender: Any?) {
        guard let node = node(sender), case .favorite(let favorite) = node.kind else { return }
        host?.openFavorite(favorite, newWindow: true)
    }

    @objc private func menuReveal(_ sender: Any?) {
        if let path = node(sender)?.path {
            NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
        }
    }

    @objc private func menuRemove(_ sender: Any?) {
        guard let node = node(sender) else { return }
        let core = AppEnvironment.shared.core
        switch node.kind {
        case .favorite(let favorite): try? core.removeFavorite(id: favorite.id)
        case .favoritePath(let child, let parent): try? core.removeFavoritePath(parent: parent.id, child: child.id)
        default: return
        }
        AppEnvironment.shared.favoritesChanged()
    }

    @objc private func menuCopyLink(_ sender: Any?) {
        guard let node = node(sender), case .favorite(let favorite) = node.kind else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(favoriteUrl(id: favorite.id), forType: .string)
    }

    @objc private func menuRevealInManager(_ sender: Any?) {
        guard let node = node(sender) else { return }
        switch node.kind {
        case .favorite(let favorite): FavoriteManagerWindowController.show(selecting: favorite.id)
        case .favorites(let id): FavoriteManagerWindowController.show(selecting: id)
        default: FavoriteManagerWindowController.show()
        }
    }

    @objc private func menuHide(_ sender: Any?) {
        guard let node = node(sender) else { return }
        if case .tag = node.kind {
            TagStore.setSidebar(node.title, shown: false)
        } else if let key = node.hideKey {
            SidebarVisibility.set(key, hidden: true)
        }
    }

    /// Hide every group except this one.
    @objc private func menuHideOthers(_ sender: Any?) {
        if let node = node(sender) { hideOthers(than: node) }
    }

    func hideOthers(than node: SidebarNode) {
        var hidden = SidebarVisibility.hidden
        for group in groups where group !== node {
            if let key = group.hideKey { hidden.insert(key) }
        }
        SidebarVisibility.setAll(hidden)
    }

    @objc private func menuShowAll(_ sender: Any?) {
        SidebarVisibility.setAll([])
    }

    @objc func menuEditor(_ sender: Any?) {
        guard let window = view.window, window.attachedSheet == nil else { return }
        SidebarEditorController(entries: editorEntries()).begin(on: window)
    }

    /// Every group and hideable item, hidden ones included.
    func editorEntries() -> [(key: String, title: String, indent: Bool)] {
        var entries: [(key: String, title: String, indent: Bool)] = [("group:devices", "Devices", false)]
        let keys: [URLResourceKey] = [.volumeNameKey, .volumeIsLocalKey]
        let volumes = FileManager.default.mountedVolumeURLs(includingResourceValuesForKeys: keys, options: [.skipHiddenVolumes]) ?? []
        let named = volumes.map { url -> (URL, String, Bool) in
            let values = try? url.resourceValues(forKeys: Set(keys))
            return (url, values?.volumeName ?? url.lastPathComponent, values?.volumeIsLocal ?? true)
        }
        for (url, name, local) in named where local { entries.append(("device:\(url.path)", name, true)) }
        entries.append(("group:connections", "Connections", false))
        entries.append(("group:shares", "Shares", false))
        for (url, name, local) in named where !local { entries.append(("share:\(url.path)", name, true)) }
        for server in ShareBrowser.shared.servers { entries.append(("share:\(server.name)", server.name, true)) }
        for group in AppEnvironment.shared.core.favoriteGroups() { entries.append(("favgroup:\(group.id)", group.name, false)) }
        entries.append(("group:recents", "Recent Folders", false))
        entries.append(("group:tags", "Tags", false))
        return entries
    }

    @objc private func menuRename(_ sender: Any?) {
        guard let node = node(sender), case .favorite(let favorite) = node.kind else { return }
        ask(title: "Rename Favorite", value: node.title) { name in
            try AppEnvironment.shared.core.renameFavorite(id: favorite.id, name: name)
        }
    }

    @objc private func menuRenameGroup(_ sender: Any?) {
        guard let node = node(sender), case .favorites(let id) = node.kind else { return }
        ask(title: "Rename Group", value: node.title) { name in
            try AppEnvironment.shared.core.renameFavoriteGroup(id: id, name: name)
        }
    }

    @objc private func menuDeleteGroup(_ sender: Any?) {
        guard let node = node(sender), case .favorites(let id) = node.kind, let window = view.window else { return }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "Delete the group “\(node.title)”?"
        alert.informativeText = node.children.isEmpty
            ? "The group is empty."
            : "Its \(node.children.count == 1 ? "favorite is" : "\(node.children.count) favorites are") deleted too."
        alert.addButton(withTitle: "Delete").hasDestructiveAction = true
        alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: window) { response in
            guard response == .alertFirstButtonReturn else { return }
            _ = try? AppEnvironment.shared.core.removeFavoriteGroup(id: id)
            AppEnvironment.shared.favoritesChanged()
        }
    }

    private func ask(title: String, value: String, apply: @escaping (String) throws -> Void) {
        guard let window = view.window else { return }
        let alert = NSAlert()
        alert.messageText = title
        alert.addButton(withTitle: "Rename")
        alert.addButton(withTitle: "Cancel")
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 280, height: 24))
        field.stringValue = value
        alert.accessoryView = field
        alert.window.initialFirstResponder = field
        alert.beginSheetModal(for: window) { response in
            guard response == .alertFirstButtonReturn else { return }
            do {
                try apply(field.stringValue)
                AppEnvironment.shared.favoritesChanged()
            } catch {
                let failure = NSAlert()
                failure.messageText = (error as? RustyFinderError)?.message ?? "The name could not be changed."
                failure.beginSheetModal(for: window)
            }
        }
    }

    @objc private func menuEject(_ sender: Any?) {
        guard let path = node(sender)?.path else { return }
        host?.eject(path, modifiers: NSEvent.modifierFlags)
    }

    // MARK: Drag and drop

    /// Devices and favorites can be dragged to a new place.
    func outlineView(_ outlineView: NSOutlineView, pasteboardWriterForItem item: Any) -> NSPasteboardWriting? {
        guard let node = item as? SidebarNode else { return nil }
        let key: String
        switch node.kind {
        case .device: key = "device:\(node.path ?? "")"
        case .favorite(let favorite): key = "favorite:\(favorite.id)"
        default: return nil
        }
        let pasteboardItem = NSPasteboardItem()
        pasteboardItem.setString(key, forType: Self.dragType)
        // A local favorite or device can also be dropped elsewhere as a file.
        if let path = node.path { pasteboardItem.setString(URL(fileURLWithPath: path).absoluteString, forType: .fileURL) }
        return pasteboardItem
    }

    func outlineView(_ outlineView: NSOutlineView, validateDrop info: NSDraggingInfo, proposedItem item: Any?, proposedChildIndex index: Int) -> NSDragOperation {
        let target = item as? SidebarNode
        if let key = info.draggingPasteboard.string(forType: Self.dragType) {
            // Reordering inside the sidebar.
            guard let target, index >= 0 else { return [] }
            if key.hasPrefix("device:"), case .devices = target.kind { return .move }
            if key.hasPrefix("favorite:"), case .favorites = target.kind { return .move }
            return []
        }
        let paths = DropRules.paths(from: info)
        guard !paths.isEmpty, let target else { return [] }
        if case .favorites = target.kind {
            // Folders dropped between favorites become favorites.
            return index >= 0 && paths.allSatisfy(Self.isFolder) ? .link : []
        }
        guard index == NSOutlineViewDropOnItemIndex else { return [] }
        if case .tag = target.kind { return .link }
        if target.application != nil { return .generic }
        if case .favorite(let f) = target.kind, f.kind == .renamePreset { return .generic }
        guard let folder = target.dropFolder,
              let mode = DropRules.mode(sources: paths, target: folder, modifiers: NSEvent.modifierFlags)
        else { return [] }
        return mode == .copy ? .copy : .move
    }

    func outlineView(_ outlineView: NSOutlineView, acceptDrop info: NSDraggingInfo, item: Any?, childIndex index: Int) -> Bool {
        guard let target = item as? SidebarNode else { return false }
        if let key = info.draggingPasteboard.string(forType: Self.dragType) {
            return reorder(key: key, into: target, index: index)
        }
        return drop(paths: DropRules.paths(from: info), on: target, index: index, modifiers: NSEvent.modifierFlags)
    }

    private static func isFolder(_ path: String) -> Bool {
        var isDir: ObjCBool = false
        return FileManager.default.fileExists(atPath: path, isDirectory: &isDir) && isDir.boolValue
    }

    @discardableResult
    func reorder(key: String, into target: SidebarNode, index: Int) -> Bool {
        if key.hasPrefix("device:"), case .devices = target.kind {
            var order = target.children.compactMap(\.path)
            let path = String(key.dropFirst(7))
            guard let from = order.firstIndex(of: path) else { return false }
            order.remove(at: from)
            order.insert(path, at: min(from < index ? index - 1 : index, order.count))
            SidebarVisibility.setDeviceOrder(order)
            lastDrop = "devices \(order.map { ($0 as NSString).lastPathComponent }.joined(separator: ","))"
            return true
        }
        if key.hasPrefix("favorite:"), case .favorites(let groupId) = target.kind, let id = UInt64(key.dropFirst(9)) {
            var to = index
            if let from = target.children.firstIndex(where: { if case .favorite(let f) = $0.kind { return f.id == id } else { return false } }),
               from < to
            { to -= 1 }
            try? AppEnvironment.shared.core.moveFavorite(id: id, groupId: groupId, index: UInt32(max(0, to)))
            AppEnvironment.shared.favoritesChanged()
            lastDrop = "favorite \(id) -> \(target.title):\(to)"
            return true
        }
        return false
    }

    /// Files dropped on a sidebar item: copied or moved into a folder,
    /// opened with an application, tagged, or added as favorites.
    @discardableResult
    func drop(paths: [String], on target: SidebarNode, index: Int, modifiers: NSEvent.ModifierFlags) -> Bool {
        guard !paths.isEmpty else { return false }
        if case .favorites = target.kind {
            let core = AppEnvironment.shared.core
            let groupName = target.title
            for path in paths where Self.isFolder(path) {
                if let added = try? core.addFavoriteDraft(group: groupName, draft: .path(path.folderDisplayName, path)),
                   case .favorites(let groupId) = target.kind, index >= 0
                {
                    try? core.moveFavorite(id: added.id, groupId: groupId, index: UInt32(index))
                }
            }
            AppEnvironment.shared.favoritesChanged()
            lastDrop = "favorites \(paths.count) -> \(groupName)"
            return true
        }
        if case .tag = target.kind {
            TagStore.add(target.title, to: paths)
            host?.tagsChangedOnDisk()
            lastDrop = "tag \(target.title) <- \(paths.count)"
            return true
        }
        if case .favorite(let preset) = target.kind, preset.kind == .renamePreset {
            host?.renameDropped(paths, with: preset)
            lastDrop = "preset \(preset.name) <- \(paths.count)"
            return true
        }
        if let app = target.application {
            ExternalApps.open(paths.map { URL(fileURLWithPath: $0) }, withAppAt: app)
            lastDrop = "app \((app as NSString).lastPathComponent) <- \(paths.count)"
            return true
        }
        guard let folder = target.dropFolder, let host,
              let mode = DropRules.mode(sources: paths, target: folder, modifiers: modifiers)
        else { return false }
        lastDrop = "\(mode == .move ? "move" : "copy") \(paths.count) -> \(folder.folderDisplayName)"
        DropRules.perform(sources: paths, target: folder, mode: mode, window: view.window, windowId: host.session.id())
        return true
    }

    /// The item with this title (scripted checks).
    func node(titled title: String) -> SidebarNode? {
        (groups + groups.flatMap(\.children) + groups.flatMap(\.children).flatMap(\.children)).first { $0.title == title }
    }

    // MARK: Scripted checks

    func debugDump() -> String {
        groups.map { group in
            "\(group.title)[\(group.children.map { child in child.children.isEmpty ? child.title : "\(child.title)(\(child.children.map(\.title).joined(separator: "+")))" }.joined(separator: ","))]"
        }.joined(separator: " ")
    }
}
