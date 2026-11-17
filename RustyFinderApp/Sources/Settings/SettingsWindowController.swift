import AppKit

/// Settings (Cmd-,): General, Transfers, Shortcuts. Every change applies
/// at once and is saved; each section can be put back to its defaults.
final class SettingsWindowController: NSWindowController, NSWindowDelegate, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
    private(set) static var current: SettingsWindowController?
    private var env: AppEnvironment { AppEnvironment.shared }
    private let tabs = NSTabView()

    // General
    private let startField = NSTextField(labelWithString: "")
    private var toggles: [(key: String, box: NSButton)] = []
    private let terminalPopup = NSPopUpButton()

    // Transfers
    private let transfersField = NSTextField(labelWithString: "")
    private let transfersStepper = NSStepper()
    private let replacePopup = NSPopUpButton()
    private let mergePopup = NSPopUpButton()
    private let errorPopup = NSPopUpButton()
    private let resumePopup = NSPopUpButton()
    private let retryBox = NSButton(checkboxWithTitle: "Automatically retry failed transfers", target: nil, action: nil)
    private let retriesField = NSTextField(labelWithString: "")
    private let retriesStepper = NSStepper()
    private let limitDownBox = NSButton(checkboxWithTitle: "Limit downloads to", target: nil, action: nil)
    private let limitDownField = NSTextField()
    private let limitUpBox = NSButton(checkboxWithTitle: "Limit uploads to", target: nil, action: nil)
    private let limitUpField = NSTextField()

    // Editing
    private let editing = EditingSettingsView()

    // Shortcuts
    private let shortcutTable = NSTableView()
    private let shortcutSearch = NSSearchField()
    private var shownCommands: [ShortcutCenter.Command] = []
    private var pendingConflict: (command: String, binding: String, other: String)?
    /// Remote pane, Full Disk Access, starting favorite, Tags and Themes.
    let extras = SettingsExtras()

    static func show() {
        if current == nil { current = SettingsWindowController() }
        current?.showWindow(nil)
        current?.window?.makeKeyAndOrderFront(nil)
    }

    private init() {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 640, height: 560),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "Settings"
        window.isReleasedWhenClosed = false
        window.isRestorable = false
        window.center()
        super.init(window: window)
        window.delegate = self
        extras.window = window
        buildUI()
        refresh()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func windowWillClose(_ notification: Notification) {
        if Self.current === self { Self.current = nil }
    }

    // MARK: Layout

    private static let generalToggles: [(String, String)] = [
        ("restore_windows", "Restore windows and tabs on startup"),
        ("show_hidden", "Show hidden files"),
        ("treat_packages_as_folders", "Treat packages as folders"),
        ("treat_archives_as_folders", "Treat archives as folders"),
        ("handle_archives", "Handle archives with RustyFinder"),
        ("confirm_drag_operations", "Ask for confirmation for drag operations"),
        ("play_sounds", "Play sounds"),
        ("relative_dates", "Use relative dates"),
        ("always_show_tab_bar", "Always show tab bar"),
        ("detect_app_when_deleting", "Detect application when deleting (App Deleter)"),
    ]

    private static let replaceChoices: [(String, ConflictPolicyChoice)] = [
        ("Ask", .ask), ("Replace", .replace), ("Replace Older", .replaceOlder), ("Keep Both", .keepBoth), ("Skip", .skip),
    ]
    private static let mergeChoices: [(String, ConflictPolicyChoice)] = [
        ("Ask", .ask), ("Merge", .merge), ("Replace", .replace), ("Keep Both", .keepBoth), ("Skip", .skip),
    ]
    private static let errorChoices: [(String, ErrorPolicyChoice)] = [("Ask", .ask), ("Skip", .skip), ("Stop", .stop)]
    private static let resumeChoices: [(String, ResumePolicyChoice)] = [
        ("Ask", .ask), ("Resume", .resume), ("Replace", .replace), ("Skip", .skip),
    ]

    /// Terminal apps RustyFinder knows by name; only installed ones are offered.
    private static let terminals: [(String, [String])] = [
        ("Terminal", ["/System/Applications/Utilities/Terminal.app"]),
        ("iTerm", ["/Applications/iTerm.app"]),
        ("Ghostty", ["/Applications/Ghostty.app"]),
        ("Warp", ["/Applications/Warp.app"]),
        ("kitty", ["/Applications/kitty.app"]),
        ("Hyper", ["/Applications/Hyper.app"]),
    ]

    private func page(_ title: String, _ views: [NSView]) {
        let stack = NSStackView(views: views)
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 9
        stack.edgeInsets = NSEdgeInsets(top: 16, left: 20, bottom: 16, right: 20)
        let item = NSTabViewItem(identifier: title)
        item.label = title
        item.view = stack
        tabs.addTabViewItem(item)
    }

    private func row(_ label: String, _ views: NSView...) -> NSStackView {
        let text = NSTextField(labelWithString: label)
        text.alignment = .right
        text.widthAnchor.constraint(equalToConstant: 190).isActive = true
        let row = NSStackView(views: [text] + views)
        row.spacing = 8
        row.alignment = .firstBaseline
        return row
    }

    private func buildUI() {
        guard let content = window?.contentView else { return }

        // General
        startField.lineBreakMode = .byTruncatingMiddle
        startField.widthAnchor.constraint(equalToConstant: 220).isActive = true
        let choose = NSButton(title: "Choose…", target: self, action: #selector(chooseStart(_:)))
        let home = NSButton(title: "Home", target: self, action: #selector(startAtHome(_:)))
        var general: [NSView] = [row("Starting directory:", startField, choose, home)]
        for (key, title) in Self.generalToggles {
            let box = NSButton(checkboxWithTitle: title, target: self, action: #selector(toggleChanged(_:)))
            box.identifier = .init(key)
            toggles.append((key, box))
            general.append(row("", box))
        }
        terminalPopup.target = self
        terminalPopup.action = #selector(terminalChanged(_:))
        general.append(row("Terminal application:", terminalPopup))
        general += extras.generalRows { label, views in
            let row = self.row(label)
            views.forEach { row.addArrangedSubview($0) }
            return row
        }
        let tools = NSButton(title: "Edit Tools…", target: NSApp.delegate, action: #selector(AppDelegate.editTools(_:)))
        general.append(row("Custom tools:", tools))
        general.append(row("", ToolSettingsRows.inheritPathBox()))
        general.append(row("Compare tool:", ToolSettingsRows.comparePopup()))
        general.append(row("", NSButton(title: "Restore Defaults", target: self, action: #selector(restoreGeneral(_:)))))
        page("General", general)

        // Transfers
        transfersStepper.minValue = 1
        transfersStepper.maxValue = 10
        transfersStepper.target = self
        transfersStepper.action = #selector(transfersChanged(_:))
        replacePopup.addItems(withTitles: Self.replaceChoices.map(\.0))
        mergePopup.addItems(withTitles: Self.mergeChoices.map(\.0))
        errorPopup.addItems(withTitles: Self.errorChoices.map(\.0))
        resumePopup.addItems(withTitles: Self.resumeChoices.map(\.0))
        for popup in [replacePopup, mergePopup, errorPopup, resumePopup] {
            popup.target = self
            popup.action = #selector(policyChanged(_:))
        }
        retriesStepper.minValue = 1
        retriesStepper.maxValue = 10
        for control in [retryBox, retriesStepper, limitDownBox, limitUpBox] as [NSControl] {
            control.target = self
            control.action = #selector(retryAndLimitsChanged(_:))
        }
        for field in [limitDownField, limitUpField] {
            field.target = self
            field.action = #selector(retryAndLimitsChanged(_:))
            field.widthAnchor.constraint(equalToConstant: 70).isActive = true
            field.alignment = .right
            field.formatter = {
                let number = NumberFormatter()
                number.minimum = 1
                number.maximum = 10_000_000
                number.allowsFloats = false
                return number
            }()
        }
        page("Transfers", [
            row("Max. simultaneous transfers:", transfersField, transfersStepper),
            row("When a file already exists:", replacePopup),
            row("When a folder already exists:", mergePopup),
            row("When a partial file exists:", resumePopup),
            row("When an item fails:", errorPopup),
            row("", retryBox),
            row("Max. number of retries:", retriesField, retriesStepper),
            row("Bandwidth:", limitDownBox, limitDownField, NSTextField(labelWithString: "KB/s")),
            row("", limitUpBox, limitUpField, NSTextField(labelWithString: "KB/s")),
            row("", NSButton(title: "Restore Defaults", target: self, action: #selector(restoreTransfers(_:)))),
        ])

        page("Editing", [editing])

        // Shortcuts
        // The page is 640 wide less the tab view's and the page's margins.
        let pageWidth: CGFloat = 540
        for (id, title, width) in [("command", "Command", 250.0), ("menu", "Menu", 110.0), ("shortcut", "Shortcut", 150.0)] {
            let column = NSTableColumn(identifier: .init(id))
            column.title = title
            column.width = width
            shortcutTable.addTableColumn(column)
        }
        shortcutTable.dataSource = self
        shortcutTable.delegate = self
        shortcutTable.rowHeight = 26
        shortcutTable.selectionHighlightStyle = .none
        shortcutTable.setAccessibilityLabel("Shortcuts")
        let scroll = NSScrollView()
        scroll.documentView = shortcutTable
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        scroll.heightAnchor.constraint(equalToConstant: 340).isActive = true
        scroll.widthAnchor.constraint(equalToConstant: pageWidth).isActive = true
        scroll.hasHorizontalScroller = false
        shortcutSearch.placeholderString = "Filter commands"
        shortcutSearch.delegate = self
        shortcutSearch.widthAnchor.constraint(equalToConstant: 240).isActive = true
        let restore = NSButton(title: "Restore Defaults", target: self, action: #selector(restoreShortcuts(_:)))
        let top = NSStackView(views: [shortcutSearch, NSView(), restore])
        top.widthAnchor.constraint(equalToConstant: pageWidth).isActive = true
        page("Shortcuts", [top, scroll])
        page("Tags", extras.tagsPage())
        page("Themes", extras.themesPage())

        tabs.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(tabs)
        NSLayoutConstraint.activate([
            tabs.topAnchor.constraint(equalTo: content.topAnchor, constant: 10),
            tabs.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -10),
            tabs.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 10),
            tabs.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -10),
        ])
    }

    // MARK: Showing the current values

    private func value(_ key: String) -> Bool {
        let s = env.settings
        switch key {
        case "restore_windows": return s.restoreWindows
        case "show_hidden": return s.showHidden
        case "treat_packages_as_folders": return s.treatPackagesAsFolders
        case "treat_archives_as_folders": return s.treatArchivesAsFolders
        case "handle_archives": return s.handleArchives
        case "confirm_drag_operations": return s.confirmDragOperations
        case "play_sounds": return s.playSounds
        case "relative_dates": return s.relativeDates
        case "always_show_tab_bar": return s.alwaysShowTabBar
        case "detect_app_when_deleting": return s.detectAppWhenDeleting
        default: return false
        }
    }

    private func set(_ key: String, _ on: Bool) {
        if key == "show_hidden" {
            // Shared with the View menu: every tab of every window follows.
            (NSApp.delegate as? AppDelegate)?.setShowHidden(on)
            return
        }
        env.update { s in
            switch key {
            case "restore_windows": s.restoreWindows = on
            case "treat_packages_as_folders": s.treatPackagesAsFolders = on
            case "treat_archives_as_folders": s.treatArchivesAsFolders = on
            case "handle_archives": s.handleArchives = on
            case "confirm_drag_operations": s.confirmDragOperations = on
            case "play_sounds": s.playSounds = on
            case "relative_dates": s.relativeDates = on
            case "always_show_tab_bar": s.alwaysShowTabBar = on
            case "detect_app_when_deleting": s.detectAppWhenDeleting = on
            default: break
            }
        }
    }

    private func refresh() {
        let s = env.settings
        startField.stringValue = s.startingDirectory.isEmpty ? "Home" : s.startingDirectory.abbreviatingHome
        for (key, box) in toggles { box.state = value(key) ? .on : .off }

        terminalPopup.removeAllItems()
        let installed = Self.terminals.compactMap { name, paths in
            paths.first { FileManager.default.fileExists(atPath: $0) }.map { (name, $0) }
        }
        var choices = installed
        if !s.terminalApp.isEmpty, !choices.contains(where: { $0.1 == s.terminalApp }) {
            choices.append((FileManager.default.displayName(atPath: s.terminalApp), s.terminalApp))
        }
        for (name, path) in choices {
            terminalPopup.addItem(withTitle: name)
            terminalPopup.lastItem?.representedObject = path
        }
        terminalPopup.menu?.addItem(.separator())
        terminalPopup.addItem(withTitle: "Other…")
        let current = s.terminalApp.isEmpty ? Self.terminals[0].1[0] : s.terminalApp
        if let index = choices.firstIndex(where: { $0.1 == current }) { terminalPopup.selectItem(at: index) }

        transfersField.stringValue = "\(s.maxTransfers)"
        transfersStepper.integerValue = Int(s.maxTransfers)
        replacePopup.selectItem(at: Self.replaceChoices.firstIndex { $0.1 == s.replacePolicy } ?? 0)
        mergePopup.selectItem(at: Self.mergeChoices.firstIndex { $0.1 == s.mergePolicy } ?? 0)
        errorPopup.selectItem(at: Self.errorChoices.firstIndex { $0.1 == s.errorPolicy } ?? 0)
        resumePopup.selectItem(at: Self.resumeChoices.firstIndex { $0.1 == s.resumePolicy } ?? 0)
        retryBox.state = s.autoRetry ? .on : .off
        retriesField.stringValue = "\(s.maxRetries)"
        retriesStepper.integerValue = Int(s.maxRetries)
        retriesStepper.isEnabled = s.autoRetry
        limitDownBox.state = s.limitDownloads ? .on : .off
        limitDownField.integerValue = Int(s.downloadLimitKbps)
        limitDownField.isEnabled = s.limitDownloads
        limitUpBox.state = s.limitUploads ? .on : .off
        limitUpField.integerValue = Int(s.uploadLimitKbps)
        limitUpField.isEnabled = s.limitUploads
        reloadShortcuts()
        extras.refreshGeneral()
        extras.refreshTags()
        extras.refreshThemes()
    }

    // MARK: General

    @objc private func toggleChanged(_ sender: NSButton) {
        guard let key = sender.identifier?.rawValue else { return }
        set(key, sender.state == .on)
    }

    @objc private func chooseStart(_ sender: Any?) {
        guard let window else { return }
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.prompt = "Choose"
        panel.beginSheetModal(for: window) { [weak self] response in
            guard response == .OK, let path = panel.url?.path else { return }
            self?.env.update { $0.startingDirectory = path }
            self?.refresh()
        }
    }

    @objc private func startAtHome(_ sender: Any?) {
        env.update { $0.startingDirectory = "" }
        refresh()
    }

    @objc private func terminalChanged(_ sender: Any?) {
        if let path = terminalPopup.selectedItem?.representedObject as? String {
            env.update { $0.terminalApp = path }
            return
        }
        // "Other…": any application.
        guard let window else { return }
        let panel = NSOpenPanel()
        panel.directoryURL = URL(fileURLWithPath: "/Applications")
        panel.allowedContentTypes = [.application]
        panel.beginSheetModal(for: window) { [weak self] response in
            if response == .OK, let path = panel.url?.path { self?.env.update { $0.terminalApp = path } }
            self?.refresh()
        }
    }

    @objc private func restoreGeneral(_ sender: Any?) {
        let defaults = env.core.defaultSettings()
        (NSApp.delegate as? AppDelegate)?.setShowHidden(defaults.showHidden)
        env.update { s in
            s.startingDirectory = defaults.startingDirectory
            s.restoreWindows = defaults.restoreWindows
            s.treatPackagesAsFolders = defaults.treatPackagesAsFolders
            s.treatArchivesAsFolders = defaults.treatArchivesAsFolders
            s.confirmDragOperations = defaults.confirmDragOperations
            s.playSounds = defaults.playSounds
            s.relativeDates = defaults.relativeDates
            s.alwaysShowTabBar = defaults.alwaysShowTabBar
            s.terminalApp = defaults.terminalApp
            s.detectAppWhenDeleting = defaults.detectAppWhenDeleting
        }
        extras.restoreGeneralDefaults(defaults)
        refresh()
    }

    // MARK: Transfers

    @objc private func transfersChanged(_ sender: Any?) {
        env.update { $0.maxTransfers = UInt32(transfersStepper.integerValue) }
        refresh()
    }

    @objc private func policyChanged(_ sender: Any?) {
        env.update { s in
            s.replacePolicy = Self.replaceChoices[max(0, replacePopup.indexOfSelectedItem)].1
            s.mergePolicy = Self.mergeChoices[max(0, mergePopup.indexOfSelectedItem)].1
            s.errorPolicy = Self.errorChoices[max(0, errorPopup.indexOfSelectedItem)].1
            s.resumePolicy = Self.resumeChoices[max(0, resumePopup.indexOfSelectedItem)].1
        }
    }

    @objc private func retryAndLimitsChanged(_ sender: Any?) {
        env.update { s in
            s.autoRetry = retryBox.state == .on
            s.maxRetries = UInt32(retriesStepper.integerValue)
            s.limitDownloads = limitDownBox.state == .on
            s.downloadLimitKbps = UInt32(max(1, limitDownField.integerValue))
            s.limitUploads = limitUpBox.state == .on
            s.uploadLimitKbps = UInt32(max(1, limitUpField.integerValue))
        }
        refresh()
    }

    @objc private func restoreTransfers(_ sender: Any?) {
        let defaults = env.core.defaultSettings()
        env.update { s in
            s.maxTransfers = defaults.maxTransfers
            s.replacePolicy = defaults.replacePolicy
            s.mergePolicy = defaults.mergePolicy
            s.errorPolicy = defaults.errorPolicy
            s.resumePolicy = defaults.resumePolicy
            s.autoRetry = defaults.autoRetry
            s.maxRetries = defaults.maxRetries
            s.limitDownloads = defaults.limitDownloads
            s.downloadLimitKbps = defaults.downloadLimitKbps
            s.limitUploads = defaults.limitUploads
            s.uploadLimitKbps = defaults.uploadLimitKbps
        }
        refresh()
    }

    // MARK: Shortcuts

    private func reloadShortcuts() {
        let needle = shortcutSearch.stringValue.lowercased()
        shownCommands = (ShortcutCenter.shared.commands + ToolShortcuts.commands).filter {
            needle.isEmpty || $0.title.lowercased().contains(needle) || $0.menu.lowercased().contains(needle)
        }
        shortcutTable.reloadData()
    }

    func controlTextDidChange(_ notification: Notification) { reloadShortcuts() }

    func numberOfRows(in tableView: NSTableView) -> Int { shownCommands.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let id = tableColumn?.identifier, let command = shownCommands[safe: row] else { return nil }
        if id.rawValue == "shortcut" {
            let field = (tableView.makeView(withIdentifier: id, owner: self) as? ShortcutRecorderField) ?? {
                let field = ShortcutRecorderField(frame: .zero)
                field.identifier = id
                return field
            }()
            field.binding = ShortcutCenter.shared.binding(of: command.id)
            field.onChange = { [weak self] binding in self?.assign(command.id, binding) }
            return field
        }
        let cell = (tableView.makeView(withIdentifier: id, owner: self) as? NSTableCellView) ?? {
            let cell = NSTableCellView()
            cell.identifier = id
            let text = NSTextField(labelWithString: "")
            text.lineBreakMode = .byTruncatingTail
            text.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(text)
            cell.textField = text
            NSLayoutConstraint.activate([
                text.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 4),
                text.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -4),
                text.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
            return cell
        }()
        cell.textField?.stringValue = id.rawValue == "command" ? command.title : command.menu
        cell.textField?.textColor = id.rawValue == "command" ? .labelColor : .secondaryLabelColor
        return cell
    }

    /// Bind a command. If another command has the combination, say which
    /// and offer to take it over.
    private func assign(_ command: String, _ binding: String) {
        guard let other = ShortcutCenter.shared.assign(command, binding) else {
            reloadShortcuts()
            return
        }
        pendingConflict = (command, binding, other)
        reloadShortcuts()
        guard let window else { return }
        let alert = NSAlert()
        alert.messageText = "“\(shortcutDisplay(binding: binding))” is already used by “\(ShortcutCenter.shared.title(of: other))”."
        alert.informativeText = "Reassign gives the shortcut to “\(ShortcutCenter.shared.title(of: command))” and leaves the other command without one."
        alert.addButton(withTitle: "Reassign")
        alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: window) { [weak self] response in
            self?.pendingConflict = nil
            if response == .alertFirstButtonReturn { ShortcutCenter.shared.assign(command, binding, force: true) }
            self?.reloadShortcuts()
        }
    }

    @objc private func restoreShortcuts(_ sender: Any?) {
        ShortcutCenter.shared.restoreDefaults()
        reloadShortcuts()
    }

    // MARK: Scripted checks

    /// `tab=<General|Transfers|Shortcuts>`, `toggle=<key>`, `transfers=<n>`,
    /// `replace=<index>`, `bind=<command>=<binding>`, `restore=<general|transfers|shortcuts>`.
    func debugSet(_ what: String) {
        let parts = what.split(separator: "=", maxSplits: 1).map(String.init)
        let value = parts.count > 1 ? parts[1] : ""
        switch parts[0] {
        case "tab": tabs.selectTabViewItem(withIdentifier: value)
        case "toggle": toggles.first { $0.key == value }?.box.performClick(nil)
        case "transfers":
            transfersStepper.integerValue = Int(value) ?? 3
            transfersChanged(nil)
        case "replace":
            replacePopup.selectItem(at: Int(value) ?? 0)
            policyChanged(nil)
        case "editor": editing.debugSet(value)
        case "resume":
            resumePopup.selectItem(at: Int(value) ?? 0)
            policyChanged(nil)
        case "retry":
            retryBox.state = value == "on" ? .on : .off
            retryAndLimitsChanged(nil)
        case "retries":
            retriesStepper.integerValue = Int(value) ?? 3
            retryAndLimitsChanged(nil)
        case "limitdown", "limitup":
            let (box, field) = parts[0] == "limitdown" ? (limitDownBox, limitDownField) : (limitUpBox, limitUpField)
            box.state = value == "off" ? .off : .on
            if let kbps = Int(value) { field.integerValue = kbps }
            retryAndLimitsChanged(nil)
        case "bind":
            let pair = value.split(separator: "=", maxSplits: 1).map(String.init)
            if pair.count == 2 { assign(pair[0], pair[1]) } else if pair.count == 1 { assign(pair[0], "") }
        case "restore":
            switch value {
            case "general": restoreGeneral(nil)
            case "transfers": restoreTransfers(nil)
            default: restoreShortcuts(nil)
            }
        default: _ = extras.debugSet(parts[0], value)
        }
    }

    func debugDump() -> String {
        let s = env.settings
        let on = toggles.filter { $0.box.state == .on }.map(\.key).joined(separator: ",")
        let conflict = pendingConflict.map { "\($0.command) wants \($0.binding) held by \($0.other)" } ?? "-"
        return "tab=\(tabs.selectedTabViewItem?.label ?? "") on=[\(on)] transfers=\(s.maxTransfers) "
            + "replace=\(s.replacePolicy) start=\(startField.stringValue) commands=\(shownCommands.count) conflict=\(conflict)"
            + " editors=\(editing.debugSummary)"
            + " resume=\(s.resumePolicy) retry=\(s.autoRetry ? "on" : "off")/\(s.maxRetries)"
            + " down=\(s.limitDownloads ? "\(s.downloadLimitKbps)" : "off") up=\(s.limitUploads ? "\(s.uploadLimitKbps)" : "off") "
            + extras.debugDump()
    }
}
