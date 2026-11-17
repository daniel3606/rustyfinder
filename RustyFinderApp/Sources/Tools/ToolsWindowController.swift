import AppKit

/// The list of custom tools and the editor for the selected one.
/// A new tool is a draft until it has a command; Esc while it is still a
/// draft discards it and touches nothing else.
final class ToolsWindowController: NSWindowController, NSWindowDelegate, NSTableViewDataSource, NSTableViewDelegate, NSTextFieldDelegate, NSTextViewDelegate {
    private(set) static var current: ToolsWindowController?
    private static let rowType = NSPasteboard.PasteboardType("dev.rustyfinder.tool-row")

    /// Saved tools plus any draft, in list order.
    private var tools: [ToolRecord] = []
    private let table = NSTableView()
    private let nameField = NSTextField()
    private let commandView = NSTextView()
    private let shortcutField = ShortcutRecorderField(frame: .zero)
    private let outputBox = NSButton(checkboxWithTitle: "Show output", target: nil, action: nil)
    private let perFileBox = NSButton(checkboxWithTitle: "Run for every selected file", target: nil, action: nil)
    private let contextBox = NSButton(checkboxWithTitle: "Show in context menu", target: nil, action: nil)
    private let toolbarBox = NSButton(checkboxWithTitle: "Show in toolbar", target: nil, action: nil)
    private var palette: NSView!
    /// A shortcut that another command or tool already has, while the
    /// question is on screen (scripted checks).
    private var pendingConflict: String?
    private let removeButton = NSButton(title: "−", target: nil, action: nil)
    private let duplicateButton = NSButton(title: "Duplicate", target: nil, action: nil)
    private var form: NSView!

    static func show() {
        if current == nil { current = ToolsWindowController() }
        current?.showWindow(nil)
        current?.window?.makeKeyAndOrderFront(nil)
    }

    private init() {
        let window = EscapeWindow(
            contentRect: NSRect(x: 0, y: 0, width: 760, height: 420),
            styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.title = "Tools"
        window.isReleasedWhenClosed = false
        window.isRestorable = false
        window.minSize = NSSize(width: 640, height: 340)
        window.setFrameAutosaveName("RustyFinderTools")
        super.init(window: window)
        window.delegate = self
        window.onEscape = { [weak self] in self?.escapePressed() }
        tools = ToolCenter.shared.tools
        buildUI()
        // Shortcuts can also change in Settings → Shortcuts.
        NotificationCenter.default.addObserver(
            self, selector: #selector(toolsChangedElsewhere(_:)), name: .rustyfinderToolsChanged, object: nil)
        if !tools.isEmpty { table.selectRowIndexes([0], byExtendingSelection: false) }
        showSelected()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func windowWillClose(_ notification: Notification) {
        if Self.current === self { Self.current = nil }
    }

    // MARK: Layout

    private func buildUI() {
        guard let content = window?.contentView else { return }
        table.addTableColumn(NSTableColumn(identifier: .init("name")))
        table.headerView = nil
        table.dataSource = self
        table.delegate = self
        table.allowsMultipleSelection = true
        table.rowHeight = 22
        table.setAccessibilityLabel("Tools")
        table.registerForDraggedTypes([Self.rowType])
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        scroll.widthAnchor.constraint(equalToConstant: 200).isActive = true

        let addButton = NSButton(title: "+", target: self, action: #selector(addTool(_:)))
        addButton.bezelStyle = .smallSquare
        addButton.setAccessibilityLabel("Add tool")
        removeButton.bezelStyle = .smallSquare
        removeButton.target = self
        removeButton.action = #selector(removeTools(_:))
        removeButton.setAccessibilityLabel("Remove tool")
        duplicateButton.bezelStyle = .smallSquare
        duplicateButton.target = self
        duplicateButton.action = #selector(duplicateTool(_:))
        let listButtons = NSStackView(views: [addButton, removeButton, duplicateButton])
        listButtons.spacing = 4
        let left = NSStackView(views: [scroll, listButtons])
        left.orientation = .vertical
        left.alignment = .leading
        left.spacing = 6

        nameField.delegate = self
        nameField.setAccessibilityLabel("Name")
        shortcutField.setAccessibilityLabel("Keyboard shortcut")
        shortcutField.onChange = { [weak self] binding in self?.shortcutRecorded(binding) }
        commandView.delegate = self
        commandView.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        commandView.isRichText = false
        commandView.isAutomaticQuoteSubstitutionEnabled = false
        commandView.isAutomaticDashSubstitutionEnabled = false
        commandView.isAutomaticTextReplacementEnabled = false
        commandView.isVerticallyResizable = true
        commandView.autoresizingMask = [.width]
        commandView.setAccessibilityLabel("Command")
        let commandScroll = NSScrollView()
        commandScroll.documentView = commandView
        commandScroll.hasVerticalScroller = true
        commandScroll.borderType = .bezelBorder
        commandScroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 120).isActive = true

        // Placeholders are dragged into the command (NSTextView takes the
        // text drop) or clicked to insert at the cursor.
        palette = PlaceholderPalette.make { [weak self] token in self?.insert(token) }
        for box in [outputBox, perFileBox, contextBox, toolbarBox] {
            box.target = self
            box.action = #selector(optionChanged(_:))
        }

        func labelled(_ text: String, _ view: NSView) -> NSStackView {
            let label = NSTextField(labelWithString: text)
            label.alignment = .right
            label.widthAnchor.constraint(equalToConstant: 70).isActive = true
            let row = NSStackView(views: [label, view])
            row.alignment = .firstBaseline
            row.spacing = 6
            return row
        }
        let right = NSStackView(views: [
            labelled("Name:", nameField), labelled("Command:", commandScroll), labelled("", palette),
            labelled("", outputBox), labelled("", perFileBox), labelled("", contextBox), labelled("", toolbarBox),
            labelled("Shortcut:", shortcutField),
        ])
        right.orientation = .vertical
        right.alignment = .leading
        right.spacing = 8
        nameField.widthAnchor.constraint(greaterThanOrEqualToConstant: 300).isActive = true
        commandScroll.widthAnchor.constraint(greaterThanOrEqualToConstant: 380).isActive = true
        shortcutField.widthAnchor.constraint(equalToConstant: 160).isActive = true
        commandView.registerForDraggedTypes([.string])
        form = right

        let root = NSStackView(views: [left, right])
        root.alignment = .top
        root.spacing = 14
        root.edgeInsets = NSEdgeInsets(top: 14, left: 14, bottom: 14, right: 14)
        root.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(root)
        NSLayoutConstraint.activate([
            root.topAnchor.constraint(equalTo: content.topAnchor),
            root.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            root.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            root.trailingAnchor.constraint(equalTo: content.trailingAnchor),
        ])
    }

    // MARK: Model

    private var selectedIndex: Int? {
        table.selectedRowIndexes.count == 1 ? table.selectedRow : nil
    }

    private func isDraft(_ tool: ToolRecord) -> Bool { tool.id == 0 }

    /// Save every tool that can be saved, keeping drafts in the list.
    private func persist() {
        let saved = ToolCenter.shared.save(tools.filter { !$0.command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !$0.name.trimmingCharacters(in: .whitespaces).isEmpty })
        // New tools come back with their ids, in the same order.
        var stored = saved.makeIterator()
        for index in tools.indices
        where !tools[index].command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !tools[index].name.trimmingCharacters(in: .whitespaces).isEmpty
        {
            if let next = stored.next() { tools[index].id = next.id }
        }
    }

    private func showSelected() {
        let tool = selectedIndex.map { tools[$0] }
        // With nothing (or several) selected there is nothing to edit.
        let editable = tool != nil
        for control in [nameField, outputBox, perFileBox, contextBox, toolbarBox] as [NSControl] {
            control.isEnabled = editable
        }
        shortcutField.isHidden = !editable
        palette.subviews.forEach { ($0 as? NSStackView)?.arrangedSubviews.forEach { ($0 as? NSControl)?.isEnabled = editable } }
        commandView.isEditable = editable
        removeButton.isEnabled = !table.selectedRowIndexes.isEmpty
        duplicateButton.isEnabled = editable
        nameField.stringValue = tool?.name ?? ""
        commandView.string = tool?.command ?? ""
        shortcutField.binding = tool?.shortcut ?? ""
        outputBox.state = tool?.showOutput == true ? .on : .off
        perFileBox.state = tool?.perFile == true ? .on : .off
        contextBox.state = tool?.inContextMenu == true ? .on : .off
        toolbarBox.state = tool?.inToolbar == true ? .on : .off
    }

    /// Take what the form shows into the selected tool and save.
    private func readForm() {
        guard let index = selectedIndex else { return }
        tools[index].name = nameField.stringValue
        tools[index].command = commandView.string
        tools[index].shortcut = shortcutField.binding
        tools[index].showOutput = outputBox.state == .on
        tools[index].perFile = perFileBox.state == .on
        tools[index].inContextMenu = contextBox.state == .on
        tools[index].inToolbar = toolbarBox.state == .on
        persist()
        // The new name shows in the list (and every menu) at once.
        table.reloadData(forRowIndexes: [index], columnIndexes: [0])
    }

    // MARK: Actions

    @objc private func addTool(_ sender: Any?) {
        tools.append(newTool())
        table.reloadData()
        table.selectRowIndexes([tools.count - 1], byExtendingSelection: false)
        showSelected()
        window?.makeFirstResponder(nameField)
    }

    @objc private func duplicateTool(_ sender: Any?) {
        guard let index = selectedIndex else { return }
        var copy = tools[index]
        copy.id = 0
        copy.name += " copy"
        copy.shortcut = ""
        tools.insert(copy, at: index + 1)
        persist()
        table.reloadData()
        table.selectRowIndexes([index + 1], byExtendingSelection: false)
        showSelected()
    }

    @objc private func removeTools(_ sender: Any?) {
        let rows = table.selectedRowIndexes
        guard !rows.isEmpty, let window else { return }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = rows.count == 1
            ? "Delete the tool “\(tools[rows.first!].name)”?"
            : "Delete \(rows.count) tools?"
        alert.addButton(withTitle: "Delete").hasDestructiveAction = true
        alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: window) { [weak self] response in
            guard response == .alertFirstButtonReturn, let self else { return }
            for row in rows.reversed() { self.tools.remove(at: row) }
            self.persist()
            self.table.reloadData()
            self.showSelected()
        }
    }

    /// Esc: a tool still being created is discarded. Saved tools stay.
    private func escapePressed() {
        guard let index = selectedIndex, isDraft(tools[index]) else { return }
        tools.remove(at: index)
        table.reloadData()
        if !tools.isEmpty { table.selectRowIndexes([min(index, tools.count - 1)], byExtendingSelection: false) }
        showSelected()
    }

    private func insert(_ token: String) {
        guard selectedIndex != nil else { return }
        commandView.insertText(token, replacementRange: commandView.selectedRange())
        readForm()
    }

    /// A shortcut was recorded. If a menu command or another tool has it,
    /// say which and offer to take it over.
    private func shortcutRecorded(_ binding: String) {
        guard let index = selectedIndex else { return }
        let own = isDraft(tools[index]) ? "tool:draft" : ToolShortcuts.commandId(tools[index].id)
        guard let other = ToolShortcuts.holder(of: binding, except: own) else {
            readForm()
            return
        }
        // Show what the tool has until the question is answered.
        shortcutField.binding = tools[index].shortcut
        pendingConflict = other
        guard let window else { return }
        let alert = NSAlert()
        alert.messageText = "“\(shortcutDisplay(binding: binding))” is already used by “\(ShortcutCenter.shared.title(of: other))”."
        alert.informativeText = "Reassign gives the shortcut to “\(tools[index].name)” and leaves the other one without it."
        alert.addButton(withTitle: "Reassign")
        alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: window) { [weak self] response in
            guard let self else { return }
            self.pendingConflict = nil
            guard response == .alertFirstButtonReturn, self.tools.indices.contains(index) else { return }
            ToolShortcuts.release(other)
            self.shortcutField.binding = shortcutNormalize(text: binding) ?? binding
            self.readForm()
        }
    }

    /// The saved list changed (perhaps from Settings → Shortcuts): take
    /// over the shortcuts, which is all that can change from outside.
    @objc private func toolsChangedElsewhere(_ notification: Notification) {
        for (index, tool) in tools.enumerated() where !isDraft(tool) {
            if let saved = ToolCenter.shared.tool(id: tool.id) { tools[index].shortcut = saved.shortcut }
        }
        if let index = selectedIndex { shortcutField.binding = tools[index].shortcut }
    }

    @objc private func optionChanged(_ sender: Any?) { readForm() }

    func controlTextDidChange(_ notification: Notification) { readForm() }

    func textDidChange(_ notification: Notification) { readForm() }

    // MARK: Table

    func numberOfRows(in tableView: NSTableView) -> Int { tools.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let id = NSUserInterfaceItemIdentifier("name")
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
        cell.textField?.stringValue = tools[row].name
        // A draft is not saved yet.
        cell.textField?.textColor = isDraft(tools[row]) ? .secondaryLabelColor : .labelColor
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) { showSelected() }

    func tableView(_ tableView: NSTableView, pasteboardWriterForRow row: Int) -> NSPasteboardWriting? {
        let item = NSPasteboardItem()
        item.setString(String(row), forType: Self.rowType)
        return item
    }

    func tableView(_ tableView: NSTableView, validateDrop info: NSDraggingInfo, proposedRow row: Int, proposedDropOperation: NSTableView.DropOperation) -> NSDragOperation {
        proposedDropOperation == .above ? .move : []
    }

    func tableView(_ tableView: NSTableView, acceptDrop info: NSDraggingInfo, row: Int, dropOperation: NSTableView.DropOperation) -> Bool {
        guard let text = info.draggingPasteboard.pasteboardItems?.first?.string(forType: Self.rowType),
              let from = Int(text), tools.indices.contains(from)
        else { return false }
        let tool = tools.remove(at: from)
        let to = min(row > from ? row - 1 : row, tools.count)
        tools.insert(tool, at: to)
        persist()
        table.reloadData()
        table.selectRowIndexes([to], byExtendingSelection: false)
        return true
    }

    // MARK: Scripted checks

    /// `add`, `escape`, `duplicate`, `select=<row>`, `name=<text>`,
    /// `command=<text>`, `perfile`, `output`.
    func debugSet(_ what: String) {
        let parts = what.split(separator: "=", maxSplits: 1).map(String.init)
        let value = parts.count > 1 ? parts[1] : ""
        switch parts[0] {
        case "add": addTool(nil)
        case "escape": escapePressed()
        case "duplicate": duplicateTool(nil)
        case "select": table.selectRowIndexes([Int(value) ?? 0], byExtendingSelection: false)
        case "name":
            nameField.stringValue = value
            readForm()
        case "command":
            commandView.string = value
            readForm()
        case "perfile": perFileBox.performClick(nil)
        case "toolbar": toolbarBox.performClick(nil)
        case "shortcut": shortcutRecorded(value)
        case "placeholder":
            commandView.setSelectedRange(NSRange(location: (commandView.string as NSString).length, length: 0))
            insert(value)
        case "output": outputBox.performClick(nil)
        default: break
        }
    }

    func debugDump() -> String {
        let list = tools.map { $0.name + (isDraft($0) ? "(draft)" : "") }.joined(separator: "|")
        let shortcut = selectedIndex.map { tools[$0].shortcut } ?? "-"
        return "\(list) selected=\(selectedIndex.map(String.init) ?? "-") remove=\(removeButton.isEnabled)"
            + " shortcut=\(shortcut.isEmpty ? "-" : shortcut) conflict=\(pendingConflict ?? "-")"
    }
}

/// A window that reports Esc.
final class EscapeWindow: NSWindow {
    var onEscape: (() -> Void)?

    override func cancelOperation(_ sender: Any?) {
        onEscape?()
    }
}
