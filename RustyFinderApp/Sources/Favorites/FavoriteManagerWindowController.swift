import AppKit

/// One row of the Favorite Manager tree.
final class ManagerNode: NSObject {
    enum Kind {
        case group(FavoriteGroupRecord)
        case favorite(FavoriteRecord, group: UInt64)
        case path(FavoritePathRecord, parent: FavoriteRecord)
    }

    let kind: Kind
    var children: [ManagerNode] = []

    init(_ kind: Kind) { self.kind = kind }

    var title: String {
        switch kind {
        case .group(let g): return g.name
        case .favorite(let f, _): return f.name
        case .path(let p, _): return p.name
        }
    }

    var id: UInt64 {
        switch kind {
        case .group(let g): return g.id
        case .favorite(let f, _): return f.id
        case .path(let p, _): return p.id
        }
    }
}

/// An outline that deletes with the Delete key.
private final class ManagerOutline: NSOutlineView {
    var onDelete: (() -> Void)?

    override func keyDown(with event: NSEvent) {
        if event.keyCode == 51 || event.keyCode == 117 { onDelete?() } else { super.keyDown(with: event) }
    }
}

/// The Favorite Manager (Opt-Cmd-F): every group and favorite as a tree,
/// with an editor for the selection. Drag to reorder or move between
/// groups.
final class FavoriteManagerWindowController: NSWindowController, NSWindowDelegate, NSOutlineViewDataSource, NSOutlineViewDelegate {
    private(set) static var current: FavoriteManagerWindowController?
    private static let dragType = NSPasteboard.PasteboardType("dev.rustyfinder.favorite")

    private let outline = ManagerOutline()
    private var roots: [ManagerNode] = []
    private var core: RustyFinderApp { AppEnvironment.shared.core }

    // Editor
    private let editor = NSStackView()
    private let nameField = NSTextField()
    private let iconPopup = NSPopUpButton()
    private let groupPopup = NSPopUpButton()
    private let pathField = NSTextField()
    private let protocolPopup = NSPopUpButton()
    private let hostField = NSTextField()
    private let portField = NSTextField()
    private let userField = NSTextField()
    private let passwordField = NSSecureTextField()
    private let sidebarBox = NSButton(checkboxWithTitle: "Show in sidebar", target: nil, action: nil)
    private let saveButton = NSButton(title: "Save", target: nil, action: nil)
    private var serverRows: [NSView] = []
    private var pathRow: NSView?
    private var favoriteRows: [NSView] = []
    private(set) var lastProblem = "-"
    /// The confirmation on screen, for scripted checks.
    private var pendingAlert: NSAlert?

    static func show(selecting id: UInt64? = nil) {
        if current == nil { current = FavoriteManagerWindowController() }
        current?.showWindow(nil)
        current?.window?.makeKeyAndOrderFront(nil)
        if let id { current?.select(id: id) }
    }

    private init() {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 720, height: 460),
            styleMask: [.titled, .closable, .resizable, .miniaturizable], backing: .buffered, defer: false)
        window.title = "Favorite Manager"
        window.isReleasedWhenClosed = false
        window.isRestorable = false
        window.minSize = NSSize(width: 600, height: 360)
        window.center()
        super.init(window: window)
        window.delegate = self
        buildUI()
        reload()
        NotificationCenter.default.addObserver(self, selector: #selector(reloadKeepingSelection), name: .rustyfinderFavoritesChanged, object: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func windowWillClose(_ notification: Notification) {
        if Self.current === self { Self.current = nil }
    }

    // MARK: Layout

    private func buildUI() {
        guard let content = window?.contentView else { return }
        let column = NSTableColumn(identifier: .init("name"))
        outline.addTableColumn(column)
        outline.outlineTableColumn = column
        outline.headerView = nil
        outline.dataSource = self
        outline.delegate = self
        outline.style = .sourceList
        outline.registerForDraggedTypes([Self.dragType])
        outline.setAccessibilityLabel("Favorites")
        outline.onDelete = { [weak self] in self?.deleteSelected(nil) }
        let scroll = NSScrollView()
        scroll.documentView = outline
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder

        func tool(_ symbol: String, _ tip: String, _ action: Selector) -> NSButton {
            let button = NSButton(image: NSImage(systemSymbolName: symbol, accessibilityDescription: tip)!, target: self, action: action)
            button.bezelStyle = .smallSquare
            button.toolTip = tip
            button.setAccessibilityLabel(tip)
            return button
        }
        let bar = NSStackView(views: [
            tool("plus", "New Favorite", #selector(newFavorite(_:))),
            tool("folder.badge.plus", "New Favorite Group", #selector(newGroup(_:))),
            tool("plus.square.on.square", "Duplicate", #selector(duplicateSelected(_:))),
            tool("minus", "Delete", #selector(deleteSelected(_:))),
        ])
        bar.spacing = 0
        let left = NSStackView(views: [scroll, bar])
        left.orientation = .vertical
        left.alignment = .leading
        left.spacing = 0
        scroll.widthAnchor.constraint(equalToConstant: 260).isActive = true
        scroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 300).isActive = true

        func row(_ label: String, _ control: NSView) -> NSStackView {
            let text = NSTextField(labelWithString: label)
            text.alignment = .right
            text.widthAnchor.constraint(equalToConstant: 80).isActive = true
            control.widthAnchor.constraint(equalToConstant: 280).isActive = true
            let row = NSStackView(views: [text, control])
            row.alignment = .firstBaseline
            return row
        }
        for (title, symbol) in [("Default", "")] + FavoriteIcons.choices {
            iconPopup.addItem(withTitle: title)
            iconPopup.lastItem?.representedObject = symbol
            if !symbol.isEmpty { iconPopup.lastItem?.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil) }
        }
        for (_, scheme) in ServerSpecs.schemes where scheme != "vnc" { protocolPopup.addItem(withTitle: scheme.uppercased()) }
        passwordField.placeholderString = "Unchanged"
        saveButton.target = self
        saveButton.action = #selector(saveEdits(_:))
        saveButton.keyEquivalent = "\r"
        sidebarBox.target = self
        sidebarBox.action = #selector(sidebarToggled(_:))
        let nameRow = row("Name:", nameField)
        let iconRow = row("Icon:", iconPopup)
        let groupRow = row("Group:", groupPopup)
        let pathRow = row("Location:", pathField)
        serverRows = [row("Protocol:", protocolPopup), row("Server:", hostField), row("Port:", portField),
                      row("User:", userField), row("Password:", passwordField)]
        self.pathRow = pathRow
        favoriteRows = [iconRow, groupRow, pathRow]
        for view in [nameRow, iconRow, groupRow, pathRow] + serverRows + [row("", sidebarBox), row("", saveButton)] {
            editor.addArrangedSubview(view)
        }
        editor.orientation = .vertical
        editor.alignment = .leading
        editor.spacing = 8

        let split = NSStackView(views: [left, editor])
        split.alignment = .top
        split.spacing = 18
        split.edgeInsets = NSEdgeInsets(top: 14, left: 14, bottom: 14, right: 14)
        split.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(split)
        NSLayoutConstraint.activate([
            split.topAnchor.constraint(equalTo: content.topAnchor),
            split.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            split.trailingAnchor.constraint(lessThanOrEqualTo: content.trailingAnchor),
            split.bottomAnchor.constraint(equalTo: content.bottomAnchor),
        ])
    }

    // MARK: Data

    @objc private func reloadKeepingSelection() {
        let selected = selectedNode?.id
        reload()
        if let selected { select(id: selected) }
    }

    func reload() {
        roots = core.favoriteGroups().map { group in
            let node = ManagerNode(.group(group))
            node.children = group.items.map { favorite in
                let item = ManagerNode(.favorite(favorite, group: group.id))
                item.children = favorite.children.map { ManagerNode(.path($0, parent: favorite)) }
                return item
            }
            return node
        }
        groupPopup.removeAllItems()
        for group in core.favoriteGroups() {
            groupPopup.addItem(withTitle: group.name)
            groupPopup.lastItem?.representedObject = group.id
        }
        outline.reloadData()
        outline.expandItem(nil, expandChildren: true)
        showEditor()
    }

    private var selectedNode: ManagerNode? { outline.item(atRow: outline.selectedRow) as? ManagerNode }

    /// Select a group, favorite or favorite path by id.
    func select(id: UInt64) {
        for row in 0..<outline.numberOfRows where (outline.item(atRow: row) as? ManagerNode)?.id == id {
            outline.selectRowIndexes([row], byExtendingSelection: false)
            outline.scrollRowToVisible(row)
            return
        }
    }

    func outlineView(_ outlineView: NSOutlineView, numberOfChildrenOfItem item: Any?) -> Int {
        (item as? ManagerNode)?.children.count ?? roots.count
    }

    func outlineView(_ outlineView: NSOutlineView, child index: Int, ofItem item: Any?) -> Any {
        (item as? ManagerNode)?.children[index] ?? roots[index]
    }

    func outlineView(_ outlineView: NSOutlineView, isItemExpandable item: Any) -> Bool {
        !((item as? ManagerNode)?.children.isEmpty ?? true)
    }

    func outlineView(_ outlineView: NSOutlineView, viewFor tableColumn: NSTableColumn?, item: Any) -> NSView? {
        guard let node = item as? ManagerNode else { return nil }
        let cell = NSTableCellView()
        let text = NSTextField(labelWithString: node.title)
        text.lineBreakMode = .byTruncatingTail
        let image = NSImageView()
        switch node.kind {
        case .group: image.image = NSImage(systemSymbolName: "folder.fill", accessibilityDescription: nil)
        case .favorite(let f, _): image.image = FavoriteIcons.image(for: f)
        case .path: image.image = NSImage(systemSymbolName: "folder", accessibilityDescription: nil)
        }
        let stack = NSStackView(views: [image, text])
        stack.spacing = 5
        stack.translatesAutoresizingMaskIntoConstraints = false
        image.widthAnchor.constraint(equalToConstant: 16).isActive = true
        cell.addSubview(stack)
        cell.textField = text
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 2),
            stack.trailingAnchor.constraint(lessThanOrEqualTo: cell.trailingAnchor),
            stack.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        return cell
    }

    func outlineViewSelectionDidChange(_ notification: Notification) { showEditor() }

    // MARK: Editor

    private func showEditor() {
        guard let node = selectedNode else {
            editor.isHidden = true
            return
        }
        editor.isHidden = false
        passwordField.stringValue = ""
        switch node.kind {
        case .group(let group):
            nameField.stringValue = group.name
            favoriteRows.forEach { $0.isHidden = true }
            serverRows.forEach { $0.isHidden = true }
            sidebarBox.superview?.isHidden = false
            sidebarBox.state = SidebarVisibility.isHidden("favgroup:\(group.id)") ? .off : .on
        case .favorite(let favorite, let group):
            nameField.stringValue = favorite.name
            favoriteRows.forEach { $0.isHidden = false }
            sidebarBox.superview?.isHidden = true
            iconPopup.selectItem(at: iconPopup.itemArray.firstIndex { ($0.representedObject as? String) == favorite.icon } ?? 0)
            groupPopup.selectItem(at: groupPopup.itemArray.firstIndex { ($0.representedObject as? UInt64) == group } ?? 0)
            let editsPath = [.path, .connection, .share, .savedSearch].contains(favorite.kind)
            pathRow?.isHidden = !editsPath
            pathField.stringValue = favorite.path
            let server = favorite.server
            serverRows.forEach { $0.isHidden = server == nil }
            if let server {
                protocolPopup.selectItem(withTitle: server.protocol.uppercased())
                hostField.stringValue = server.host
                portField.stringValue = server.port.map(String.init) ?? ""
                userField.stringValue = server.user
            }
        case .path(let child, _):
            nameField.stringValue = child.name
            favoriteRows.forEach { $0.isHidden = true }
            serverRows.forEach { $0.isHidden = true }
            sidebarBox.superview?.isHidden = true
        }
    }

    @objc private func sidebarToggled(_ sender: NSButton) {
        guard case .group(let group) = selectedNode?.kind else { return }
        SidebarVisibility.set("favgroup:\(group.id)", hidden: sender.state == .off)
    }

    @objc private func saveEdits(_ sender: Any?) {
        guard let node = selectedNode else { return }
        let name = nameField.stringValue
        do {
            switch node.kind {
            case .group(let group):
                try core.renameFavoriteGroup(id: group.id, name: name)
            case .favorite(let favorite, let group):
                var draft = FavoriteDraftRecord(favorite)
                draft.name = name
                draft.icon = iconPopup.selectedItem?.representedObject as? String ?? ""
                draft.path = pathField.stringValue
                if var server = favorite.server {
                    let scheme = protocolPopup.titleOfSelectedItem?.lowercased() ?? server.protocol
                    server.protocol = scheme
                    server.host = hostField.stringValue.trimmingCharacters(in: .whitespaces)
                    server.port = UInt16(portField.stringValue.trimmingCharacters(in: .whitespaces))
                    server.user = userField.stringValue.trimmingCharacters(in: .whitespaces)
                    server.keychain = Keychain.account(host: server.host, user: server.user, port: server.port, scheme: scheme)
                    if !passwordField.stringValue.isEmpty, !ExternalApps.isDryRun {
                        Keychain.store(passwordField.stringValue, for: server.keychain)
                    }
                    draft.server = server
                }
                let target = groupPopup.selectedItem?.representedObject as? UInt64
                try core.updateFavorite(id: favorite.id, draft: draft, group: target == group ? nil : target)
            case .path(let child, let parent):
                // A favorite path is renamed by replacing it.
                try core.removeFavoritePath(parent: parent.id, child: child.id)
                _ = try core.addFavoritePath(parent: parent.id, name: name, path: child.path)
            }
            lastProblem = "-"
            AppEnvironment.shared.favoritesChanged()
        } catch {
            problem((error as? RustyFinderError)?.message ?? "The change could not be saved.")
        }
    }

    private func problem(_ message: String) {
        lastProblem = message
        guard let window else { return }
        let alert = NSAlert()
        alert.messageText = message
        alert.beginSheetModal(for: window)
    }

    // MARK: Actions

    private var selectedGroupId: UInt64? {
        switch selectedNode?.kind {
        case .group(let g): return g.id
        case .favorite(_, let group): return group
        case .path(_, let parent): return core.favoriteGroupOf(id: parent.id)
        case nil: return core.favoriteGroups().first?.id
        }
    }

    /// A favorite for the front browser window's folder, in the selected group.
    @objc func newFavorite(_ sender: Any?) {
        let front = (NSApp.delegate as? AppDelegate)?.windowControllers.first { $0.window?.isMainWindow == true }
            ?? (NSApp.delegate as? AppDelegate)?.windowControllers.last
        let path = front?.activeList.flatMap { $0.isRemote ? nil : $0.content.path } ?? NSHomeDirectory()
        let groupName = core.favoriteGroups().first { $0.id == selectedGroupId }?.name ?? "Favorites"
        let taken = Set(core.favoriteGroups().first { $0.name == groupName }?.items.map { $0.name.lowercased() } ?? [])
        var name = path.folderDisplayName
        var n = 2
        while taken.contains(name.lowercased()) {
            name = "\(path.folderDisplayName) \(n)"
            n += 1
        }
        do {
            let added = try core.addFavoriteDraft(group: groupName, draft: .path(name, path))
            AppEnvironment.shared.favoritesChanged()
            select(id: added.id)
            window?.makeFirstResponder(nameField)
        } catch {
            problem((error as? RustyFinderError)?.message ?? "The favorite could not be added.")
        }
    }

    @objc func newGroup(_ sender: Any?) {
        let names = Set(core.favoriteGroups().map { $0.name.lowercased() })
        var name = "New Group"
        var n = 2
        while names.contains(name.lowercased()) {
            name = "New Group \(n)"
            n += 1
        }
        do {
            let id = try core.addFavoriteGroup(name: name)
            AppEnvironment.shared.favoritesChanged()
            select(id: id)
            window?.makeFirstResponder(nameField)
        } catch {
            problem((error as? RustyFinderError)?.message ?? "The group could not be added.")
        }
    }

    /// FV-03: the copy is selected and opens the same place.
    @objc func duplicateSelected(_ sender: Any?) {
        guard case .favorite(let favorite, _) = selectedNode?.kind else { return }
        do {
            let copy = try core.duplicateFavorite(id: favorite.id)
            AppEnvironment.shared.favoritesChanged()
            select(id: copy.id)
        } catch {
            problem((error as? RustyFinderError)?.message ?? "The favorite could not be duplicated.")
        }
    }

    /// Delete, after confirming; a group goes with its contents (FV-04).
    @objc func deleteSelected(_ sender: Any?) {
        guard let node = selectedNode, let window else { return }
        let alert = NSAlert()
        alert.alertStyle = .warning
        switch node.kind {
        case .group(let group):
            let count = group.items.count
            alert.messageText = "Delete the group “\(group.name)”?"
            alert.informativeText = count == 0
                ? "The group is empty."
                : "Its \(count == 1 ? "favorite is" : "\(count) favorites are") deleted too: \(group.items.map(\.name).joined(separator: ", "))."
        case .favorite(let favorite, _):
            alert.messageText = "Delete the favorite “\(favorite.name)”?"
            alert.informativeText = favorite.kind == .workspace ? "The workspace is deleted too." : "The item itself is not touched."
        case .path(let child, let parent):
            alert.messageText = "Delete the favorite path “\(child.name)”?"
            alert.informativeText = "It is removed from “\(parent.name)”."
        }
        alert.addButton(withTitle: "Delete").hasDestructiveAction = true
        alert.addButton(withTitle: "Cancel")
        pendingAlert = alert
        alert.beginSheetModal(for: window) { [weak self] response in
            self?.pendingAlert = nil
            guard let self, response == .alertFirstButtonReturn else { return }
            switch node.kind {
            case .group(let group): _ = try? self.core.removeFavoriteGroup(id: group.id)
            case .favorite(let favorite, _): try? self.core.removeFavorite(id: favorite.id)
            case .path(let child, let parent): try? self.core.removeFavoritePath(parent: parent.id, child: child.id)
            }
            AppEnvironment.shared.favoritesChanged()
        }
    }

    // MARK: Drag to reorder

    func outlineView(_ outlineView: NSOutlineView, pasteboardWriterForItem item: Any) -> NSPasteboardWriting? {
        guard let node = item as? ManagerNode else { return nil }
        if case .path = node.kind { return nil }
        let pasteboardItem = NSPasteboardItem()
        pasteboardItem.setString(String(node.id), forType: Self.dragType)
        return pasteboardItem
    }

    private func draggedNode(_ info: NSDraggingInfo) -> ManagerNode? {
        guard let text = info.draggingPasteboard.string(forType: Self.dragType), let id = UInt64(text) else { return nil }
        return roots.first { $0.id == id } ?? roots.flatMap(\.children).first { $0.id == id }
    }

    func outlineView(_ outlineView: NSOutlineView, validateDrop info: NSDraggingInfo, proposedItem item: Any?, proposedChildIndex index: Int) -> NSDragOperation {
        guard let dragged = draggedNode(info) else { return [] }
        let target = item as? ManagerNode
        switch dragged.kind {
        case .group:
            // Groups move among groups only.
            return target == nil && index >= 0 ? .move : []
        case .favorite:
            guard let target, case .group = target.kind else { return [] }
            if index < 0 { outlineView.setDropItem(target, dropChildIndex: target.children.count) }
            return .move
        case .path:
            return []
        }
    }

    func outlineView(_ outlineView: NSOutlineView, acceptDrop info: NSDraggingInfo, item: Any?, childIndex index: Int) -> Bool {
        guard let dragged = draggedNode(info) else { return false }
        return move(dragged, to: item as? ManagerNode, index: index)
    }

    @discardableResult
    private func move(_ dragged: ManagerNode, to target: ManagerNode?, index: Int) -> Bool {
        switch dragged.kind {
        case .group(let group):
            let from = roots.firstIndex { $0.id == group.id } ?? 0
            let to = max(0, index > from ? index - 1 : index)
            try? core.moveFavoriteGroup(id: group.id, index: UInt32(to))
        case .favorite(let favorite, let fromGroup):
            guard let target, case .group(let group) = target.kind else { return false }
            var to = index < 0 ? target.children.count : index
            if fromGroup == group.id, let from = target.children.firstIndex(where: { $0.id == favorite.id }), from < to { to -= 1 }
            try? core.moveFavorite(id: favorite.id, groupId: group.id, index: UInt32(to))
        case .path:
            return false
        }
        AppEnvironment.shared.favoritesChanged()
        select(id: dragged.id)
        return true
    }

    // MARK: Scripted checks

    /// `select=<name>`, `duplicate`, `delete`, `newgroup`, `newfavorite`,
    /// `name=<text>` (then `save`), `icon=<symbol>`, `group=<name>`,
    /// `move=<favorite>><group>:<index>`.
    func debugSet(_ what: String) {
        let parts = what.split(separator: "=", maxSplits: 1).map(String.init)
        let value = parts.count > 1 ? parts[1] : ""
        func find(_ name: String) -> ManagerNode? {
            let all = roots + roots.flatMap(\.children) + roots.flatMap(\.children).flatMap(\.children)
            return all.first { $0.title == name }
        }
        switch parts[0] {
        case "select": if let node = find(value) { select(id: node.id) }
        case "duplicate": duplicateSelected(nil)
        case "delete": deleteSelected(nil)
        case "newgroup": newGroup(nil)
        case "newfavorite": newFavorite(nil)
        case "name": nameField.stringValue = value
        case "path": pathField.stringValue = value
        case "icon": iconPopup.selectItem(at: iconPopup.itemArray.firstIndex { ($0.representedObject as? String) == value } ?? 0)
        case "group": groupPopup.selectItem(withTitle: value)
        case "save": saveEdits(nil)
        case "move":
            let sides = value.split(separator: ">", maxSplits: 1).map(String.init)
            let target = sides.count == 2 ? sides[1].split(separator: ":").map(String.init) : []
            guard sides.count == 2, let node = find(sides[0]), let group = target.first.flatMap(find) else { return }
            move(node, to: group, index: target.count > 1 ? Int(target[1]) ?? -1 : -1)
        default: break
        }
    }

    func debugDump() -> String {
        let tree = roots.map { group in
            let items = group.children.map { node -> String in
                node.children.isEmpty ? node.title : "\(node.title)(\(node.children.map(\.title).joined(separator: "+")))"
            }
            return "\(group.title)[\(items.joined(separator: ","))]"
        }.joined(separator: " ")
        let confirm = pendingAlert.map { "\($0.messageText) \($0.informativeText)" } ?? "-"
        return "\(tree) selected=\(selectedNode?.title ?? "-") problem=\(lastProblem) confirm=\(confirm)"
    }
}
