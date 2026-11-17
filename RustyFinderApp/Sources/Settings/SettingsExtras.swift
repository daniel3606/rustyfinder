import AppKit

/// The parts of Settings for navigation, favorites, tags and themes:
/// General's remote-pane, archive, Full Disk Access and starting-favorite
/// choices, the Tags page and the Themes page.
@MainActor
final class SettingsExtras: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSMenuDelegate {
    weak var window: NSWindow?
    private var env: AppEnvironment { AppEnvironment.shared }
    private var core: RustyFinderApp { env.core }

    // General
    private let remotePopup = NSPopUpButton()
    private let startFavoritePopup = NSPopUpButton()
    private let fdaLabel = NSTextField(labelWithString: "")

    // Tags
    private let tagTable = NSTableView()
    private var tags: [TagEntryRecord] = []
    private let tagStatus = NSTextField(labelWithString: "")
    private static let tagDrag = NSPasteboard.PasteboardType("dev.rustyfinder.tagrow")
    private var tagQuery: NSMetadataQuery?

    // Themes
    private let themeTable = NSTableView()
    private var themes: [ThemeRecord] = []
    private var wells: [String: NSColorWell] = [:]
    private let darkBox = NSButton(checkboxWithTitle: "Dark style", target: nil, action: nil)
    private let followBox = NSButton(checkboxWithTitle: "Follow macOS Light and Dark", target: nil, action: nil)
    private let themeNote = NSTextField(labelWithString: "")
    private(set) var pendingConfirmation: NSAlert?

    private static let colorTitles: [(String, String)] = [
        ("base", "Base (accent)"), ("background", "Background"), ("alternate_background", "Alternate rows"),
        ("text", "Text"), ("secondary_text", "Secondary text"), ("selection", "Selection"),
        ("inactive_selection", "Inactive selection"), ("sidebar_background", "Sidebar background"),
        ("sidebar_text", "Sidebar text"), ("sidebar_selection", "Sidebar selection"),
    ]

    // MARK: General

    func generalRows(_ row: (String, [NSView]) -> NSView) -> [NSView] {
        remotePopup.addItems(withTitles: ["Active pane", "Left pane", "Right pane"])
        remotePopup.target = self
        remotePopup.action = #selector(remoteChanged(_:))
        startFavoritePopup.target = self
        startFavoritePopup.action = #selector(startFavoriteChanged(_:))
        let fdaButton = NSButton(title: "Show Reminder Again", target: self, action: #selector(resetFda(_:)))
        fdaLabel.textColor = .secondaryLabelColor
        return [
            row("Open remote connections in:", [remotePopup]),
            row("Start at favorite:", [startFavoritePopup]),
            row("Full Disk Access:", [fdaLabel, fdaButton]),
        ]
    }

    func refreshGeneral() {
        let s = env.settings
        remotePopup.selectItem(at: [RemoteSideChoice.active, .left, .right].firstIndex(of: s.openRemoteIn) ?? 0)
        startFavoritePopup.removeAllItems()
        startFavoritePopup.addItem(withTitle: "None (use the starting directory)")
        startFavoritePopup.lastItem?.representedObject = UInt64(0)
        for group in core.favoriteGroups() {
            for favorite in group.items where favorite.kind == .path {
                startFavoritePopup.addItem(withTitle: "\(group.name) › \(favorite.name)")
                startFavoritePopup.lastItem?.representedObject = favorite.id
            }
        }
        startFavoritePopup.selectItem(at: startFavoritePopup.itemArray.firstIndex { ($0.representedObject as? UInt64) == s.startingFavorite } ?? 0)
        fdaLabel.stringValue = FullDiskAccess.hasAccess ? "Granted" : (s.fdaDismissed ? "Not granted; reminder off" : "Not granted")
    }

    @objc private func remoteChanged(_ sender: Any?) {
        let choice = [RemoteSideChoice.active, .left, .right][max(0, remotePopup.indexOfSelectedItem)]
        env.update { $0.openRemoteIn = choice }
    }

    @objc private func startFavoriteChanged(_ sender: Any?) {
        let id = startFavoritePopup.selectedItem?.representedObject as? UInt64 ?? 0
        env.update { $0.startingFavorite = id }
    }

    @objc private func resetFda(_ sender: Any?) {
        env.update { $0.fdaDismissed = false }
        refreshGeneral()
    }

    func restoreGeneralDefaults(_ defaults: AppSettings) {
        env.update { s in
            s.openRemoteIn = defaults.openRemoteIn
            s.handleArchives = defaults.handleArchives
            s.startingFavorite = defaults.startingFavorite
        }
        refreshGeneral()
    }

    // MARK: Tags

    func tagsPage() -> [NSView] {
        for (id, title, width) in [("color", "Colour", 110.0), ("name", "Tag", 270.0), ("sidebar", "In Sidebar", 90.0)] {
            let column = NSTableColumn(identifier: .init(id))
            column.title = title
            column.width = width
            tagTable.addTableColumn(column)
        }
        tagTable.dataSource = self
        tagTable.delegate = self
        tagTable.rowHeight = 24
        tagTable.allowsMultipleSelection = true
        tagTable.registerForDraggedTypes([Self.tagDrag])
        tagTable.setAccessibilityLabel("Tags")
        let menu = NSMenu()
        menu.delegate = self
        tagTable.menu = menu
        let scroll = NSScrollView()
        scroll.documentView = tagTable
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        scroll.heightAnchor.constraint(equalToConstant: 300).isActive = true
        scroll.widthAnchor.constraint(equalToConstant: 540).isActive = true
        tagStatus.textColor = .secondaryLabelColor
        let rename = NSButton(title: "Rename…", target: self, action: #selector(renameSelectedTag(_:)))
        let delete = NSButton(title: "Delete…", target: self, action: #selector(deleteSelectedTags(_:)))
        let note = NSTextField(wrappingLabelWithString: "Drag to change the order of the sidebar and of tag dots. Renaming changes the tag on every item that has it.")
        note.textColor = .secondaryLabelColor
        note.widthAnchor.constraint(equalToConstant: 540).isActive = true
        return [scroll, NSStackView(views: [rename, delete, tagStatus]), note]
    }

    func refreshTags() {
        tags = TagStore.list
        tagTable.reloadData()
    }

    private var selectedTagNames: [String] {
        let rows = tagTable.clickedRow >= 0 && !tagTable.selectedRowIndexes.contains(tagTable.clickedRow)
            ? IndexSet(integer: tagTable.clickedRow) : tagTable.selectedRowIndexes
        return rows.compactMap { tags[safe: $0]?.name }
    }

    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        guard tagTable.clickedRow >= 0 else { return }
        menu.addItem(NSMenuItem(title: "Rename…", action: #selector(renameSelectedTag(_:)), keyEquivalent: ""))
        menu.addItem(NSMenuItem(title: selectedTagNames.count > 1 ? "Delete \(selectedTagNames.count) Tags…" : "Delete…",
                                action: #selector(deleteSelectedTags(_:)), keyEquivalent: ""))
        let sidebar = NSMenuItem(title: "Hide from Sidebar", action: #selector(hideSelectedTags(_:)), keyEquivalent: "")
        menu.addItem(sidebar)
        menu.items.forEach { $0.target = self }
    }

    @objc private func hideSelectedTags(_ sender: Any?) {
        let names = Set(selectedTagNames)
        TagStore.setList(tags.map { names.contains($0.name) ? TagEntryRecord(name: $0.name, color: $0.color, sidebar: false) : $0 })
        refreshTags()
    }

    func numberOfRows(in tableView: NSTableView) -> Int {
        tableView === tagTable ? tags.count : themes.count
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        if tableView === themeTable {
            let theme = themes[row]
            let field = NSTextField(labelWithString: theme.name + (theme.builtin ? "" : "  (custom)"))
            return field
        }
        guard let id = tableColumn?.identifier.rawValue, let tag = tags[safe: row] else { return nil }
        switch id {
        case "color":
            let popup = NSPopUpButton()
            popup.controlSize = .small
            let labels = NSWorkspace.shared.fileLabels
            let colors = NSWorkspace.shared.fileLabelColors
            for (index, name) in labels.enumerated() {
                popup.addItem(withTitle: index == 0 ? "No Colour" : name)
                if index > 0, colors.indices.contains(index) {
                    let swatch = NSImage(size: NSSize(width: 10, height: 10), flipped: false) { rect in
                        colors[index].setFill()
                        NSBezierPath(ovalIn: rect).fill()
                        return true
                    }
                    popup.lastItem?.image = swatch
                }
            }
            popup.selectItem(at: Int(tag.color))
            popup.tag = row
            popup.target = self
            popup.action = #selector(tagColorChanged(_:))
            return popup
        case "sidebar":
            let box = NSButton(checkboxWithTitle: "", target: self, action: #selector(tagSidebarChanged(_:)))
            box.state = tag.sidebar ? .on : .off
            box.tag = row
            box.setAccessibilityLabel("Show \(tag.name) in the sidebar")
            return box
        default:
            return NSTextField(labelWithString: tag.name)
        }
    }

    @objc private func tagColorChanged(_ sender: NSPopUpButton) {
        guard tags.indices.contains(sender.tag) else { return }
        tags[sender.tag].color = UInt8(max(0, sender.indexOfSelectedItem))
        TagStore.setList(tags)
        refreshTags()
    }

    @objc private func tagSidebarChanged(_ sender: NSButton) {
        guard tags.indices.contains(sender.tag) else { return }
        tags[sender.tag].sidebar = sender.state == .on
        TagStore.setList(tags)
    }

    // Drag to reorder.
    func tableView(_ tableView: NSTableView, pasteboardWriterForRow row: Int) -> NSPasteboardWriting? {
        guard tableView === tagTable else { return nil }
        let item = NSPasteboardItem()
        item.setString(String(row), forType: Self.tagDrag)
        return item
    }

    func tableView(_ tableView: NSTableView, validateDrop info: NSDraggingInfo, proposedRow row: Int, proposedDropOperation op: NSTableView.DropOperation) -> NSDragOperation {
        guard tableView === tagTable else { return [] }
        tableView.setDropRow(row, dropOperation: .above)
        return .move
    }

    func tableView(_ tableView: NSTableView, acceptDrop info: NSDraggingInfo, row: Int, dropOperation: NSTableView.DropOperation) -> Bool {
        guard let text = info.draggingPasteboard.string(forType: Self.tagDrag), let from = Int(text) else { return false }
        moveTag(from: from, to: row)
        return true
    }

    func moveTag(from: Int, to: Int) {
        guard tags.indices.contains(from) else { return }
        let tag = tags.remove(at: from)
        tags.insert(tag, at: min(from < to ? to - 1 : to, tags.count))
        TagStore.setList(tags)
        refreshTags()
    }

    @objc private func renameSelectedTag(_ sender: Any?) {
        guard let old = selectedTagNames.first, let window else { return }
        let alert = NSAlert()
        alert.messageText = "Rename the tag “\(old)”"
        alert.informativeText = "Every item that has the tag gets the new name."
        alert.addButton(withTitle: "Rename")
        alert.addButton(withTitle: "Cancel")
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 260, height: 24))
        field.stringValue = old
        alert.accessoryView = field
        alert.window.initialFirstResponder = field
        alert.beginSheetModal(for: window) { [weak self] response in
            guard response == .alertFirstButtonReturn else { return }
            self?.renameTag(old, to: field.stringValue)
        }
    }

    /// TG-05: rename in the list, then on every item carrying the tag.
    func renameTag(_ old: String, to new: String) {
        let new = new.trimmingCharacters(in: .whitespaces)
        guard !new.isEmpty, new != old else { return }
        do {
            TagStore.listChanged(try core.renameTagInList(old: old, new: new))
        } catch {
            tagStatus.stringValue = (error as? RustyFinderError)?.message ?? "The tag could not be renamed."
            return
        }
        refreshTags()
        tagStatus.stringValue = "Renaming “\(old)”…"
        var changed = 0
        itemsTagged(old) { [weak self] paths, done in
            changed += TagStore.rename(old, to: new, on: paths)
            self?.tagStatus.stringValue = done
                ? "“\(old)” is now “\(new)” on \(changed == 1 ? "1 item" : "\(changed) items")."
                : "Renaming “\(old)”… \(changed) so far"
            Self.reloadLocalPanes()
        }
    }

    @objc private func deleteSelectedTags(_ sender: Any?) {
        let names = selectedTagNames
        guard !names.isEmpty, let window else { return }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = names.count == 1 ? "Delete the tag “\(names[0])”?" : "Delete \(names.count) tags?"
        alert.informativeText = "The tag leaves the list. Items keep it unless you also remove it from them."
        let box = NSButton(checkboxWithTitle: "Also remove from all items", target: nil, action: nil)
        alert.accessoryView = box
        alert.addButton(withTitle: "Delete").hasDestructiveAction = true
        alert.addButton(withTitle: "Cancel")
        pendingConfirmation = alert
        alert.beginSheetModal(for: window) { [weak self] response in
            self?.pendingConfirmation = nil
            guard let self, response == .alertFirstButtonReturn else { return }
            self.deleteTags(names, fromItems: box.state == .on)
        }
    }

    func deleteTags(_ names: [String], fromItems: Bool) {
        TagStore.listChanged(core.deleteTags(names: names))
        refreshTags()
        guard fromItems else { return }
        for name in names {
            var changed = 0
            itemsTagged(name) { [weak self] paths, done in
                changed += TagStore.remove(name, from: paths)
                if done { self?.tagStatus.stringValue = "“\(name)” removed from \(changed == 1 ? "1 item" : "\(changed) items")." }
                Self.reloadLocalPanes()
            }
        }
    }

    /// Items carrying `tag`, in two rounds: first the items of every folder
    /// open in a pane (at once; Spotlight may not have indexed them yet),
    /// then what Spotlight finds in the home folder and on indexed volumes
    /// (`done` true).
    private func itemsTagged(_ tag: String, found: @escaping ([String], _ done: Bool) -> Void) {
        var paths = Set<String>()
        for controller in (NSApp.delegate as? AppDelegate)?.windowControllers ?? [] {
            for side in [PaneSide.left, .right] {
                guard let list = controller.fileList(for: controller.currentTab(side)), !list.isRemote,
                      let folder = list.content.path,
                      let names = try? FileManager.default.contentsOfDirectory(atPath: folder)
                else { continue }
                for name in names {
                    let path = (folder as NSString).appendingPathComponent(name)
                    if TagStore.tags(at: path).contains(where: { $0.caseInsensitiveCompare(tag) == .orderedSame }) { paths.insert(path) }
                }
            }
        }
        found(paths.sorted(), false)
        let seen = paths
        let query = NSMetadataQuery()
        query.predicate = NSPredicate(format: "kMDItemUserTags == %@", tag)
        query.searchScopes = [NSMetadataQueryUserHomeScope, NSMetadataQueryIndexedLocalComputerScope]
        var token: NSObjectProtocol?
        var finished = false
        let finish: () -> Void = { [weak self] in
            guard !finished else { return }
            finished = true
            query.stop()
            if let token { NotificationCenter.default.removeObserver(token) }
            var more = Set<String>()
            for item in query.results.compactMap({ $0 as? NSMetadataItem }) {
                if let path = item.value(forAttribute: NSMetadataItemPathKey) as? String, !seen.contains(path) { more.insert(path) }
            }
            self?.tagQuery = nil
            found(more.sorted(), true)
        }
        token = NotificationCenter.default.addObserver(forName: .NSMetadataQueryDidFinishGathering, object: query, queue: .main) { _ in
            MainActor.assumeIsolated { finish() }
        }
        tagQuery = query
        if !query.start() { finish() }
        // Spotlight can be slow or off: never wait for it forever.
        DispatchQueue.main.asyncAfter(deadline: .now() + 10) { finish() }
    }

    private static func reloadLocalPanes() {
        for controller in (NSApp.delegate as? AppDelegate)?.windowControllers ?? [] {
            controller.tagsChangedOnDisk()
        }
    }

    // MARK: Themes

    func themesPage() -> [NSView] {
        let column = NSTableColumn(identifier: .init("theme"))
        column.width = 200
        themeTable.addTableColumn(column)
        themeTable.headerView = nil
        themeTable.dataSource = self
        themeTable.delegate = self
        themeTable.rowHeight = 22
        themeTable.setAccessibilityLabel("Themes")
        let scroll = NSScrollView()
        scroll.documentView = themeTable
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        scroll.widthAnchor.constraint(equalToConstant: 200).isActive = true
        scroll.heightAnchor.constraint(equalToConstant: 300).isActive = true
        let buttons = NSStackView(views: [
            NSButton(title: "Duplicate", target: self, action: #selector(duplicateTheme(_:))),
            NSButton(title: "Rename…", target: self, action: #selector(renameTheme(_:))),
            NSButton(title: "Delete…", target: self, action: #selector(deleteTheme(_:))),
        ])
        buttons.spacing = 4
        let left = NSStackView(views: [scroll, buttons])
        left.orientation = .vertical
        left.alignment = .leading

        let editor = NSStackView()
        editor.orientation = .vertical
        editor.alignment = .leading
        editor.spacing = 5
        followBox.target = self
        followBox.action = #selector(themeEdited(_:))
        darkBox.target = self
        darkBox.action = #selector(themeEdited(_:))
        editor.addArrangedSubview(followBox)
        editor.addArrangedSubview(darkBox)
        for (field, title) in Self.colorTitles {
            let well = NSColorWell(style: .minimal)
            well.target = self
            well.action = #selector(themeEdited(_:))
            well.widthAnchor.constraint(equalToConstant: 44).isActive = true
            well.heightAnchor.constraint(equalToConstant: 20).isActive = true
            wells[field] = well
            let system = NSButton(title: "System", target: self, action: #selector(useSystemColor(_:)))
            system.identifier = .init(field)
            system.controlSize = .small
            let label = NSTextField(labelWithString: title)
            label.widthAnchor.constraint(equalToConstant: 130).isActive = true
            let row = NSStackView(views: [label, well, system])
            row.spacing = 6
            editor.addArrangedSubview(row)
        }
        themeNote.textColor = .secondaryLabelColor
        themeNote.lineBreakMode = .byWordWrapping
        themeNote.widthAnchor.constraint(equalToConstant: 300).isActive = true
        editor.addArrangedSubview(themeNote)
        let page = NSStackView(views: [left, editor])
        page.alignment = .top
        page.spacing = 18
        return [page]
    }

    func refreshThemes() {
        themes = core.themes()
        themeTable.reloadData()
        let current = core.currentTheme()
        if let index = themes.firstIndex(where: { $0.id == current.id }) {
            themeTable.selectRowIndexes([index], byExtendingSelection: false)
        }
        showThemeEditor(current)
    }

    private func showThemeEditor(_ theme: ThemeRecord) {
        let editable = !theme.builtin
        followBox.state = theme.appearance == .system ? .on : .off
        darkBox.state = theme.appearance == .dark ? .on : .off
        darkBox.isEnabled = editable && theme.appearance != .system
        followBox.isEnabled = editable
        for (field, well) in wells {
            let hex = Self.value(of: field, in: theme)
            well.color = ThemeCenter.color(hex) ?? .clear
            well.isEnabled = editable
        }
        themeNote.stringValue = editable
            ? "Changes apply at once. “System” uses macOS’s own colour."
            : "Built-in themes can’t be changed. Duplicate one to edit it."
    }

    private static func value(of field: String, in t: ThemeRecord) -> String {
        switch field {
        case "base": return t.base
        case "background": return t.background
        case "alternate_background": return t.alternateBackground
        case "text": return t.text
        case "secondary_text": return t.secondaryText
        case "selection": return t.selection
        case "inactive_selection": return t.inactiveSelection
        case "sidebar_background": return t.sidebarBackground
        case "sidebar_text": return t.sidebarText
        default: return t.sidebarSelection
        }
    }

    private static func set(_ field: String, _ value: String, in t: inout ThemeRecord) {
        switch field {
        case "base": t.base = value
        case "background": t.background = value
        case "alternate_background": t.alternateBackground = value
        case "text": t.text = value
        case "secondary_text": t.secondaryText = value
        case "selection": t.selection = value
        case "inactive_selection": t.inactiveSelection = value
        case "sidebar_background": t.sidebarBackground = value
        case "sidebar_text": t.sidebarText = value
        default: t.sidebarSelection = value
        }
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard (notification.object as? NSTableView) === themeTable, let theme = themes[safe: themeTable.selectedRow] else { return }
        selectTheme(theme.id)
    }

    /// Use a theme now, in every window (SD-04).
    func selectTheme(_ id: String) {
        guard core.selectTheme(id: id) else { return }
        ThemeCenter.shared.reload()
        if let theme = core.themes().first(where: { $0.id == id }) { showThemeEditor(theme) }
    }

    private var selectedTheme: ThemeRecord? { themes[safe: themeTable.selectedRow] }

    @objc private func themeEdited(_ sender: Any?) {
        guard var theme = selectedTheme, !theme.builtin else { return }
        theme.appearance = followBox.state == .on ? .system : (darkBox.state == .on ? .dark : .light)
        if let well = sender as? NSColorWell, let field = wells.first(where: { $0.value === well })?.key {
            Self.set(field, ThemeCenter.hex(well.color), in: &theme)
        }
        saveTheme(theme)
    }

    @objc private func useSystemColor(_ sender: NSButton) {
        guard var theme = selectedTheme, !theme.builtin, let field = sender.identifier?.rawValue else { return }
        Self.set(field, "", in: &theme)
        saveTheme(theme)
    }

    func saveTheme(_ theme: ThemeRecord) {
        _ = core.saveTheme(theme: theme)
        themes = core.themes()
        ThemeCenter.shared.reload()
        if let fresh = themes.first(where: { $0.id == theme.id }) { showThemeEditor(fresh) }
    }

    @objc func duplicateTheme(_ sender: Any?) {
        guard let source = selectedTheme ?? themes.first, let copy = core.duplicateTheme(id: source.id) else { return }
        refreshThemesSelecting(copy.id)
    }

    private func refreshThemesSelecting(_ id: String) {
        themes = core.themes()
        themeTable.reloadData()
        if let index = themes.firstIndex(where: { $0.id == id }) {
            themeTable.selectRowIndexes([index], byExtendingSelection: false)
        }
    }

    @objc private func renameTheme(_ sender: Any?) {
        guard let theme = selectedTheme, !theme.builtin, let window else { return }
        let alert = NSAlert()
        alert.messageText = "Rename Theme"
        alert.addButton(withTitle: "Rename")
        alert.addButton(withTitle: "Cancel")
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 240, height: 24))
        field.stringValue = theme.name
        alert.accessoryView = field
        alert.beginSheetModal(for: window) { [weak self] response in
            guard response == .alertFirstButtonReturn else { return }
            self?.renameTheme(theme.id, to: field.stringValue)
        }
    }

    func renameTheme(_ id: String, to name: String) {
        do {
            try core.renameTheme(id: id, name: name)
            themeNote.stringValue = ""
        } catch {
            themeNote.stringValue = (error as? RustyFinderError)?.message ?? "The theme could not be renamed."
        }
        refreshThemesSelecting(id)
    }

    /// SD-05: asks first; windows using it fall back to Default.
    @objc private func deleteTheme(_ sender: Any?) {
        guard let theme = selectedTheme, !theme.builtin, let window else { return }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "Delete the theme “\(theme.name)”?"
        alert.informativeText = "Windows using it switch to the Default theme. This can’t be undone."
        alert.addButton(withTitle: "Delete").hasDestructiveAction = true
        alert.addButton(withTitle: "Cancel")
        pendingConfirmation = alert
        alert.beginSheetModal(for: window) { [weak self] response in
            self?.pendingConfirmation = nil
            guard let self, response == .alertFirstButtonReturn else { return }
            _ = self.core.deleteTheme(id: theme.id)
            ThemeCenter.shared.reload()
            self.refreshThemes()
        }
    }

    // MARK: Scripted checks

    /// Handles the Settings tokens this part owns; false if not one.
    func debugSet(_ key: String, _ value: String) -> Bool {
        switch key {
        case "remote":
            remotePopup.selectItem(at: ["active": 0, "left": 1, "right": 2][value] ?? 0)
            remoteChanged(nil)
        case "theme":
            if let id = themes.first(where: { $0.name == value || $0.id == value })?.id {
                if let index = themes.firstIndex(where: { $0.id == id }) { themeTable.selectRowIndexes([index], byExtendingSelection: false) }
                selectTheme(id)
            }
        case "duplicatetheme": duplicateTheme(nil)
        case "renametheme": if let theme = selectedTheme { renameTheme(theme.id, to: value) }
        case "deletetheme": deleteTheme(nil)
        case "themecolor":
            // themecolor=<field>:#RRGGBB
            let pair = value.split(separator: ":", maxSplits: 1).map(String.init)
            guard pair.count == 2, var theme = selectedTheme, !theme.builtin else { return true }
            Self.set(pair[0], normalizeThemeColor(text: pair[1]) ?? "", in: &theme)
            saveTheme(theme)
        case "themedark":
            guard var theme = selectedTheme, !theme.builtin else { return true }
            theme.appearance = value == "system" ? .system : (value == "on" ? .dark : .light)
            saveTheme(theme)
        case "tagrename":
            let pair = value.split(separator: ">", maxSplits: 1).map(String.init)
            if pair.count == 2 { renameTag(pair[0], to: pair[1]) }
        case "tagdelete":
            // tagdelete=<name>[!] (with ! also from items)
            let all = value.hasSuffix("!")
            deleteTags([all ? String(value.dropLast()) : value], fromItems: all)
        case "tagmove":
            let pair = value.split(separator: ">").compactMap { Int($0) }
            if pair.count == 2 { moveTag(from: pair[0], to: pair[1]) }
        case "tagsidebar":
            if let index = tags.firstIndex(where: { $0.name == value }) {
                tags[index].sidebar.toggle()
                TagStore.setList(tags)
                refreshTags()
            }
        case "tagcolor":
            let pair = value.split(separator: "=", maxSplits: 1).map(String.init)
            if pair.count == 2, let index = tags.firstIndex(where: { $0.name == pair[0] }), let color = UInt8(pair[1]) {
                tags[index].color = color
                TagStore.setList(tags)
                refreshTags()
            }
        default:
            return false
        }
        return true
    }

    func debugDump() -> String {
        let tagText = tags.map { "\($0.name)\($0.color > 0 ? ":\($0.color)" : "")\($0.sidebar ? "" : "(hidden)")" }.joined(separator: ",")
        let s = env.settings
        return "remote=\(s.openRemoteIn) startfav=\(s.startingFavorite) themes=\(themes.map(\.name).joined(separator: "|")) "
            + "theme=\(core.currentTheme().name) tags=[\(tagText)] tagstatus=\(tagStatus.stringValue) "
            + "confirm=\(pendingConfirmation?.messageText ?? "-") themenote=\(themeNote.stringValue)"
    }
}
