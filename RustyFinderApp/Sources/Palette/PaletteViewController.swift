import AppKit

/// One row of a pop-up list.
struct PaletteRow {
    var title: String
    /// Secondary text on the right: a section name or a path.
    var detail: String = ""
    var icon: NSImage?
    /// Key binding shown at the far right ("⌃⌘6").
    var shortcut: String = ""
    /// Character indices of `title` to emphasise (the matched letters).
    var highlights: [Int] = []
    /// Text the field takes when this row is accepted with Tab.
    var completion: String?
    var activate: () -> Void = {}
}

/// The shared keyboard pop-up: a field over a list. Used by Quick Open and
/// Go to Folder. Up/Down move the highlight, Return activates, Esc closes,
/// double-click activates.
final class PaletteViewController: NSViewController, NSTextFieldDelegate, NSTableViewDataSource, NSTableViewDelegate {
    var placeholder = ""
    /// Rows for the current text. Called on every edit.
    var rowsProvider: (String) -> [PaletteRow] = { _ in [] }
    /// Return pressed. Return a message to show inline and stay open, or
    /// nil when done (the pop-up then closes).
    var onReturn: (String, PaletteRow?) -> String? = { _, row in
        row?.activate()
        return nil
    }
    /// Tab completes from the highlighted row instead of leaving the field.
    var tabCompletes = false

    private let field = NSTextField()
    private let table = NSTableView()
    private let scroll = NSScrollView()
    private let message = NSTextField(labelWithString: "")
    private(set) var rows: [PaletteRow] = []

    override func loadView() {
        let root = NSView()
        field.placeholderString = placeholder
        field.font = .systemFont(ofSize: 16)
        field.isBordered = false
        field.focusRingType = .none
        field.drawsBackground = false
        field.delegate = self
        field.translatesAutoresizingMaskIntoConstraints = false

        let column = NSTableColumn(identifier: .init("row"))
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = 26
        table.style = .inset
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.doubleAction = #selector(activateClicked(_:))
        table.refusesFirstResponder = true  // the field keeps the keyboard
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        scroll.translatesAutoresizingMaskIntoConstraints = false

        message.textColor = .systemRed
        message.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        message.lineBreakMode = .byTruncatingMiddle
        message.isHidden = true
        message.translatesAutoresizingMaskIntoConstraints = false

        let separator = NSBox()
        separator.boxType = .separator
        separator.translatesAutoresizingMaskIntoConstraints = false
        for v in [field, separator, scroll, message] { root.addSubview(v) }
        NSLayoutConstraint.activate([
            root.widthAnchor.constraint(equalToConstant: 620),
            field.topAnchor.constraint(equalTo: root.topAnchor, constant: 14),
            field.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 16),
            field.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            separator.topAnchor.constraint(equalTo: field.bottomAnchor, constant: 10),
            separator.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            separator.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: separator.bottomAnchor, constant: 4),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scroll.heightAnchor.constraint(equalToConstant: 320),
            message.topAnchor.constraint(equalTo: scroll.bottomAnchor, constant: 4),
            message.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 16),
            message.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            message.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -8),
        ])
        view = root
        refresh()
    }

    override func viewDidAppear() {
        super.viewDidAppear()
        view.window?.makeFirstResponder(field)
    }

    // MARK: Text and keys

    func controlTextDidChange(_ notification: Notification) {
        message.isHidden = true
        refresh()
    }

    private func refresh() {
        rows = rowsProvider(field.stringValue)
        table.reloadData()
        if !rows.isEmpty { table.selectRowIndexes([0], byExtendingSelection: false) }
    }

    private var highlighted: PaletteRow? { rows[safe: table.selectedRow] }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.moveDown(_:)): move(by: 1)
        case #selector(NSResponder.moveUp(_:)): move(by: -1)
        case #selector(NSResponder.insertNewline(_:)): submit()
        case #selector(NSResponder.cancelOperation(_:)): close()
        case #selector(NSResponder.insertTab(_:)) where tabCompletes: complete()
        case #selector(NSResponder.moveRight(_:)) where tabCompletes:
            // Right arrow at the very end accepts the completion too.
            let atEnd = textView.selectedRange().location == (field.stringValue as NSString).length
            guard atEnd, highlighted?.completion != nil else { return false }
            complete()
        default: return false
        }
        return true
    }

    private func move(by delta: Int) {
        guard !rows.isEmpty else { return }
        let next = min(max(table.selectedRow + delta, 0), rows.count - 1)
        table.selectRowIndexes([next], byExtendingSelection: false)
        table.scrollRowToVisible(next)
    }

    /// Put the highlighted row's completion into the field, cursor at the
    /// end, nothing selected.
    private func complete() {
        guard let text = highlighted?.completion else { return }
        field.stringValue = text
        field.currentEditor()?.selectedRange = NSRange(location: (text as NSString).length, length: 0)
        message.isHidden = true
        refresh()
    }

    private func submit() {
        if let problem = onReturn(field.stringValue, highlighted) {
            message.stringValue = problem
            message.isHidden = false
        } else {
            close()
        }
    }

    @objc private func activateClicked(_ sender: Any?) {
        guard table.clickedRow >= 0 else { return }
        table.selectRowIndexes([table.clickedRow], byExtendingSelection: false)
        submit()
    }

    func close() {
        guard let window = view.window else { return }
        window.sheetParent?.endSheet(window)
    }

    // MARK: Table

    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row index: Int) -> NSView? {
        guard let row = rows[safe: index] else { return nil }
        let id = NSUserInterfaceItemIdentifier("palette")
        let cell = (tableView.makeView(withIdentifier: id, owner: self) as? PaletteCell) ?? PaletteCell(identifier: id)
        cell.configure(row)
        return cell
    }

    // MARK: Scripted checks

    func debugType(_ text: String) {
        loadViewIfNeeded()
        field.stringValue = text
        controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: field))
    }

    func debugKey(_ key: String) {
        switch key {
        case "down": move(by: 1)
        case "up": move(by: -1)
        case "tab": complete()
        case "return": submit()
        case "escape": close()
        default: break
        }
    }

    func debugDump() -> String {
        let listed = rows.prefix(6).enumerated().map { index, row in
            (index == table.selectedRow ? "*" : "") + row.title + (row.shortcut.isEmpty ? "" : " [\(row.shortcut)]")
        }.joined(separator: " | ")
        return "field=\(field.stringValue) rows=\(rows.count) top=\(listed) message=\(message.isHidden ? "-" : message.stringValue)"
    }
}

/// A row of the pop-up: icon, title with matched letters emphasised, a
/// secondary text and a key binding. Long text truncates in the middle on
/// one line, never wraps.
final class PaletteCell: NSTableCellView {
    private let icon = NSImageView()
    private let title = NSTextField(labelWithString: "")
    private let detail = NSTextField(labelWithString: "")
    private let shortcut = NSTextField(labelWithString: "")

    init(identifier: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        self.identifier = identifier
        icon.imageScaling = .scaleProportionallyUpOrDown
        title.lineBreakMode = .byTruncatingMiddle
        detail.lineBreakMode = .byTruncatingMiddle
        detail.textColor = .secondaryLabelColor
        detail.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        detail.alignment = .right
        shortcut.textColor = .secondaryLabelColor
        shortcut.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        shortcut.alignment = .right
        title.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        detail.setContentCompressionResistancePriority(.defaultLow - 1, for: .horizontal)
        shortcut.setContentCompressionResistancePriority(.required, for: .horizontal)
        shortcut.setContentHuggingPriority(.required, for: .horizontal)
        let row = NSStackView(views: [icon, title, detail, shortcut])
        row.orientation = .horizontal
        row.spacing = 8
        row.translatesAutoresizingMaskIntoConstraints = false
        addSubview(row)
        NSLayoutConstraint.activate([
            icon.widthAnchor.constraint(equalToConstant: 16),
            icon.heightAnchor.constraint(equalToConstant: 16),
            row.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 6),
            row.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -6),
            row.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func configure(_ row: PaletteRow) {
        icon.image = row.icon
        detail.stringValue = row.detail
        shortcut.stringValue = row.shortcut
        let text = NSMutableAttributedString(string: row.title, attributes: [.font: NSFont.systemFont(ofSize: NSFont.systemFontSize)])
        let characters = Array(row.title)
        for index in row.highlights where index < characters.count {
            // Highlights are character indices; attributed strings count UTF-16.
            let start = String(characters[..<index]).utf16.count
            let length = String(characters[index]).utf16.count
            text.addAttribute(.font, value: NSFont.systemFont(ofSize: NSFont.systemFontSize, weight: .bold), range: NSRange(location: start, length: length))
        }
        title.attributedStringValue = text
    }
}

/// The menu bar as a list of commands: for Quick Open, and for running a
/// command the way a shortcut would.
@MainActor
enum CommandCatalog {
    struct Command {
        /// "View › Sort By › Size"
        let path: String
        let shortcut: String
        let item: NSMenuItem
    }

    /// Every enabled menu command, with titles as they currently read
    /// (validation fills in titles such as "Undo Move to Trash").
    static func commands(in window: NSWindow) -> [Command] {
        var out: [Command] = []
        func walk(_ menu: NSMenu, _ trail: [String]) {
            for item in menu.items where !item.isSeparatorItem && !item.isHidden {
                if let submenu = item.submenu {
                    walk(submenu, trail + [item.title])
                } else if item.action != nil, isEnabled(item, in: window) {
                    out.append(Command(path: (trail + [item.title]).joined(separator: " › "), shortcut: shortcut(of: item), item: item))
                }
            }
        }
        // The first menu is the application menu, with no title of its own.
        for (index, top) in (NSApp.mainMenu?.items ?? []).enumerated() {
            guard let submenu = top.submenu else { continue }
            walk(submenu, [index == 0 ? "RustyFinder" : submenu.title])
        }
        return out
    }

    /// The responders a menu action is offered to, in order: the sheet's
    /// chain if one is up, then the window's, then the app.
    private static func chain(for window: NSWindow) -> [AnyObject] {
        var chain: [AnyObject] = []
        for start in [window.attachedSheet?.firstResponder, window.firstResponder] {
            var responder = start
            while let r = responder {
                chain.append(r)
                responder = r.nextResponder
            }
        }
        return chain + [window.delegate, NSApp, NSApp.delegate].compactMap { $0 as AnyObject? }
    }

    private static func target(for item: NSMenuItem, in window: NSWindow) -> AnyObject? {
        guard let action = item.action else { return nil }
        if let fixed = item.target { return fixed.responds(to: action) ? fixed : nil }
        return chain(for: window).first { $0.responds(to: action) }
    }

    static func isEnabled(_ item: NSMenuItem, in window: NSWindow) -> Bool {
        guard let target = target(for: item, in: window) else { return false }
        if let validator = target as? NSUserInterfaceValidations { return validator.validateUserInterfaceItem(item) }
        return true
    }

    /// Run a menu command as if chosen from the menu. Returns false if it
    /// has no target or is disabled right now.
    @discardableResult
    static func perform(_ item: NSMenuItem, in window: NSWindow) -> Bool {
        guard let action = item.action, let target = target(for: item, in: window), isEnabled(item, in: window) else { return false }
        _ = target.perform(action, with: item)
        return true
    }

    /// "⌃⌘6" for a menu item's key equivalent.
    static func shortcut(of item: NSMenuItem) -> String {
        guard !item.keyEquivalent.isEmpty else { return "" }
        let mods = item.keyEquivalentModifierMask
        var text = ""
        if mods.contains(.control) { text += "⌃" }
        if mods.contains(.option) { text += "⌥" }
        if mods.contains(.shift) { text += "⇧" }
        if mods.contains(.command) { text += "⌘" }
        let names: [String: String] = [
            String(UnicodeScalar(NSUpArrowFunctionKey)!): "↑", String(UnicodeScalar(NSDownArrowFunctionKey)!): "↓",
            String(UnicodeScalar(NSLeftArrowFunctionKey)!): "←", String(UnicodeScalar(NSRightArrowFunctionKey)!): "→",
            String(UnicodeScalar(NSBackspaceCharacter)!): "⌫", "\t": "⇥", "\r": "↩", " ": "Space",
            String(UnicodeScalar(NSF5FunctionKey)!): "F5", String(UnicodeScalar(NSF6FunctionKey)!): "F6",
        ]
        return text + (names[item.keyEquivalent] ?? item.keyEquivalent.uppercased())
    }
}
