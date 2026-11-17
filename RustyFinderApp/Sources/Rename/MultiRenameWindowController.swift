import AppKit

/// Multi-Rename: an ordered list of rules on the left, a live preview on
/// the right. The rules, the preview, every validity and conflict flag and
/// the safe ordering all come from the Rust core; this window edits the
/// rule list and shows what the core says.
final class MultiRenameWindowController: NSWindowController, NSWindowDelegate, NSTableViewDataSource, NSTableViewDelegate {
    /// The window that is open, if any (one at a time).
    private(set) static var current: MultiRenameWindowController?

    private var items: [RenameItem]
    private let conn: UInt64
    private let windowId: UInt64
    /// Rules in the JSON shape the core reads (`rustyfinder_core::rename::Rule`).
    private var rules: [[String: Any]] = []
    private var preview: RenamePreview?
    /// Metadata of each item by path, read in the background (local items only).
    private var metadata: [String: RenameMetadata] = [:]
    private static let ruleType = NSPasteboard.PasteboardType("dev.rustyfinder.rename-rule")

    private let rulesTable = NSTableView()
    private let previewTable = NSTableView()
    private let editor = NSStackView()
    private let footer = NSTextField(labelWithString: "")
    private let renameButton = NSButton(title: "Rename", target: nil, action: nil)
    private let addButton = NSPopUpButton(frame: .zero, pullsDown: true)
    private static let rowType = NSPasteboard.PasteboardType("dev.rustyfinder.rename-row")

    static func show(items: [InspectedItem], conn: UInt64, windowId: UInt64) {
        current?.close()
        let controller = MultiRenameWindowController(items: items, conn: conn, windowId: windowId)
        current = controller
        controller.showWindow(nil)
    }

    private init(items: [InspectedItem], conn: UInt64, windowId: UInt64) {
        self.items = items.map {
            RenameItem(path: $0.path, isDir: $0.row.isDir, createdMs: $0.row.createdMs, modifiedMs: $0.row.modifiedMs)
        }
        self.conn = conn
        self.windowId = windowId
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 900, height: 520),
            styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.title = "Rename \(items.count) Items"
        window.isReleasedWhenClosed = false
        window.isRestorable = false
        window.minSize = NSSize(width: 700, height: 380)
        // Size and position are remembered between uses.
        window.setFrameAutosaveName("RustyFinderMultiRename")
        super.init(window: window)
        window.delegate = self
        buildUI()
        addRule(Self.defaultRule(kind: "ReplaceText"))
        loadMetadata()
    }

    /// Photo dates, dimensions, artists… for the metadata rules. Read off
    /// the main thread; the preview updates when they arrive.
    private func loadMetadata() {
        guard conn == 0 else { return }
        let paths = items.filter { !$0.isDir }.map(\.path)
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let found = Dictionary(uniqueKeysWithValues: paths.map { ($0, RenameMetadataReader.read(path: $0)) })
            DispatchQueue.main.async {
                guard let self else { return }
                self.metadata = found
                self.refresh()
            }
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func windowWillClose(_ notification: Notification) {
        if Self.current === self { Self.current = nil }
    }

    // MARK: Layout

    private func buildUI() {
        guard let content = window?.contentView else { return }

        rulesTable.addTableColumn(NSTableColumn(identifier: .init("rule")))
        rulesTable.headerView = nil
        rulesTable.dataSource = self
        rulesTable.delegate = self
        rulesTable.rowHeight = 24
        rulesTable.setAccessibilityLabel("Rules")
        // Rules are dragged into another order.
        rulesTable.registerForDraggedTypes([Self.ruleType])
        rulesTable.setDraggingSourceOperationMask(.move, forLocal: true)
        let rulesScroll = NSScrollView()
        rulesScroll.documentView = rulesTable
        rulesScroll.hasVerticalScroller = true
        rulesScroll.borderType = .bezelBorder

        addButton.addItem(withTitle: "+")
        for (title, kind) in Self.kinds { 
            addButton.addItem(withTitle: title)
            addButton.lastItem?.representedObject = kind
        }
        addButton.target = self
        addButton.action = #selector(addChosen(_:))
        let removeButton = NSButton(title: "−", target: self, action: #selector(removeRule(_:)))
        removeButton.bezelStyle = .smallSquare
        let ruleButtons = NSStackView(views: [addButton, removeButton])
        ruleButtons.spacing = 4

        editor.orientation = .vertical
        editor.alignment = .leading
        editor.spacing = 6

        let left = NSStackView(views: [rulesScroll, ruleButtons, editor])
        left.orientation = .vertical
        left.alignment = .leading
        left.spacing = 8
        rulesScroll.heightAnchor.constraint(equalToConstant: 140).isActive = true
        rulesScroll.widthAnchor.constraint(equalToConstant: 320).isActive = true

        for (id, title, width) in [("original", "Original Name", 220.0), ("new", "New Name", 220.0), ("note", "", 160.0)] {
            let column = NSTableColumn(identifier: .init(id))
            column.title = title
            column.width = width
            previewTable.addTableColumn(column)
        }
        previewTable.dataSource = self
        previewTable.delegate = self
        previewTable.rowHeight = 20
        previewTable.usesAlternatingRowBackgroundColors = true
        previewTable.setAccessibilityLabel("Preview")
        // Rows can be dragged into another order, which drives sequences.
        previewTable.registerForDraggedTypes([Self.rowType])
        previewTable.setDraggingSourceOperationMask(.move, forLocal: true)
        let previewScroll = NSScrollView()
        previewScroll.documentView = previewTable
        previewScroll.hasVerticalScroller = true
        previewScroll.borderType = .bezelBorder

        footer.textColor = .secondaryLabelColor
        footer.lineBreakMode = .byTruncatingTail
        renameButton.target = self
        renameButton.action = #selector(rename(_:))
        renameButton.keyEquivalent = "\r"
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancel(_:)))
        cancel.keyEquivalent = "\u{1b}"
        let bottom = NSStackView(views: [footer, NSView(), cancel, renameButton])
        bottom.spacing = 8

        let top = NSStackView(views: [left, previewScroll])
        top.spacing = 12
        top.alignment = .top
        let root = NSStackView(views: [top, bottom])
        root.orientation = .vertical
        root.spacing = 10
        root.edgeInsets = NSEdgeInsets(top: 14, left: 14, bottom: 12, right: 14)
        root.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(root)
        NSLayoutConstraint.activate([
            root.topAnchor.constraint(equalTo: content.topAnchor),
            root.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            root.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            root.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            previewScroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 280),
            bottom.widthAnchor.constraint(equalTo: root.widthAnchor, constant: -28),
        ])
    }

    // MARK: Rules

    private static let kinds: [(String, String)] = [
        ("Replace Text", "ReplaceText"), ("Add Text", "AddText"), ("Change Case", "ChangeCase"),
        ("Add Sequence", "AddSequence"), ("Add Date", "AddDate"), ("Add Metadata", "AddMetadata"),
    ]

    /// Ready-made date formats; the field takes any Unicode pattern.
    private static let dateFormats = [
        "yyyy-MM-dd", "yyyyMMdd", "yyyy-MM-dd HH.mm.ss", "yyyyMMdd_HHmmss", "dd.MM.yyyy", "MM-dd-yyyy", "yyyy-MM",
        "yyyy",
    ]

    private static func defaultRule(kind: String) -> [String: Any] {
        let body: [String: Any]
        switch kind {
        case "ChangeCase": body = ["case": "Lower"]
        case "AddText": body = ["text": "", "position": "End"]
        case "AddSequence": body = ["start": 1, "step": 1, "padding": 1, "position": "Beginning", "prefix": "", "suffix": " "]
        case "AddDate": body = ["source": "Modified", "format": "yyyy-MM-dd", "position": "Beginning", "separator": " "]
        case "AddMetadata": body = ["key": "Dimensions", "position": "End"]
        default:
            body = ["find": "", "replace": "", "case_sensitive": false, "regex": false, "occurrences": "All", "entire_text": false]
        }
        return ["kind": [kind: body], "target": "Name", "enabled": true, "files": true, "folders": true]
    }

    private func kind(of rule: [String: Any]) -> (name: String, body: [String: Any]) {
        guard let wrapper = rule["kind"] as? [String: Any], let name = wrapper.keys.first,
              let body = wrapper[name] as? [String: Any]
        else { return ("", [:]) }
        return (name, body)
    }

    private func addRule(_ rule: [String: Any]) {
        rules.append(rule)
        rulesTable.reloadData()
        rulesTable.selectRowIndexes([rules.count - 1], byExtendingSelection: false)
        rebuildEditor()
        refresh()
    }

    @objc private func addChosen(_ sender: NSPopUpButton) {
        guard let kind = sender.selectedItem?.representedObject as? String else { return }
        addRule(Self.defaultRule(kind: kind))
    }

    @objc private func removeRule(_ sender: Any?) {
        let row = rulesTable.selectedRow
        guard rules.indices.contains(row) else { return }
        rules.remove(at: row)
        rulesTable.reloadData()
        if !rules.isEmpty { rulesTable.selectRowIndexes([min(row, rules.count - 1)], byExtendingSelection: false) }
        rebuildEditor()
        refresh()
    }

    /// Change one field of the selected rule's body and recompute.
    private func set(_ key: String, _ value: Any, inBody: Bool = true) {
        let row = rulesTable.selectedRow
        guard rules.indices.contains(row) else { return }
        if inBody {
            let (name, body) = kind(of: rules[row])
            var changed = body
            changed[key] = value
            rules[row]["kind"] = [name: changed]
        } else {
            rules[row][key] = value
        }
        rulesTable.reloadData(forRowIndexes: [row], columnIndexes: [0])
        refresh()
    }

    private func describe(_ rule: [String: Any]) -> String {
        let (name, body) = kind(of: rule)
        let title = Self.kinds.first { $0.1 == name }?.0 ?? name
        switch name {
        case "ReplaceText": return "\(title): “\(body["find"] ?? "")” → “\(body["replace"] ?? "")”"
        case "AddText": return "\(title): “\(body["text"] ?? "")”"
        case "ChangeCase": return "\(title): \(body["case"] ?? "")"
        case "AddSequence": return "\(title): from \(body["start"] ?? 1)"
        case "AddDate": return "\(title): \(body["format"] ?? "")"
        case "AddMetadata": return "\(title): \(body["key"] ?? "")"
        default: return title
        }
    }

    // MARK: Editor for the selected rule

    private var fieldActions: [ObjectIdentifier: (NSControl) -> Void] = [:]

    private func rebuildEditor() {
        editor.arrangedSubviews.forEach {
            editor.removeArrangedSubview($0)
            $0.removeFromSuperview()
        }
        fieldActions = [:]
        let row = rulesTable.selectedRow
        guard rules.indices.contains(row) else { return }
        let rule = rules[row]
        let (name, body) = kind(of: rule)

        func line(_ label: String, _ control: NSView) {
            let text = NSTextField(labelWithString: label)
            text.alignment = .right
            text.widthAnchor.constraint(equalToConstant: 90).isActive = true
            let stack = NSStackView(views: [text, control])
            stack.spacing = 6
            stack.alignment = .firstBaseline
            editor.addArrangedSubview(stack)
        }
        func field(_ key: String, number: Bool = false) -> NSTextField {
            let f = NSTextField(string: "\(body[key] ?? "")")
            f.widthAnchor.constraint(equalToConstant: 190).isActive = true
            f.target = self
            f.action = #selector(controlChanged(_:))
            f.delegate = textWatcher
            fieldActions[ObjectIdentifier(f)] = { [weak self] control in
                let text = control.stringValue
                self?.set(key, number ? (Int(text) ?? 0) as Any : text as Any)
            }
            return f
        }
        func popup(_ key: String, _ options: [(String, Any)], current: String, inBody: Bool = true) -> NSPopUpButton {
            let p = NSPopUpButton()
            for (title, _) in options { p.addItem(withTitle: title) }
            if let index = options.firstIndex(where: { "\($0.1)" == current }) { p.selectItem(at: index) }
            p.target = self
            p.action = #selector(controlChanged(_:))
            fieldActions[ObjectIdentifier(p)] = { [weak self] control in
                guard let popup = control as? NSPopUpButton else { return }
                self?.set(key, options[popup.indexOfSelectedItem].1, inBody: inBody)
            }
            return p
        }
        func check(_ title: String, _ key: String, on: Bool, inBody: Bool = true) -> NSButton {
            let b = NSButton(checkboxWithTitle: title, target: self, action: #selector(controlChanged(_:)))
            b.state = on ? .on : .off
            fieldActions[ObjectIdentifier(b)] = { [weak self] control in
                self?.set(key, (control as? NSButton)?.state == .on, inBody: inBody)
            }
            return b
        }
        /// Beginning, End or Specific Index (a 0-based character index,
        /// from the start or from the end).
        func position(default fallback: String) {
            let current = body["position"] ?? fallback
            let index = ((current as? [String: Any])?["Index"] as? [String: Any])
            let p = NSPopUpButton()
            p.addItems(withTitles: ["Beginning", "End", "Specific Index"])
            p.selectItem(at: index != nil ? 2 : (("\(current)" == "Beginning") ? 0 : 1))
            let at = NSTextField(string: "\(index?["index"] ?? 0)")
            at.widthAnchor.constraint(equalToConstant: 50).isActive = true
            at.isEnabled = index != nil
            at.delegate = textWatcher
            at.setAccessibilityLabel("Index")
            let fromEnd = NSButton(checkboxWithTitle: "from end", target: self, action: #selector(controlChanged(_:)))
            fromEnd.state = (index?["from_end"] as? Bool) == true ? .on : .off
            fromEnd.isEnabled = index != nil
            let apply: (NSControl) -> Void = { [weak self, weak p, weak at, weak fromEnd] _ in
                guard let p, let at, let fromEnd else { return }
                let value: Any
                switch p.indexOfSelectedItem {
                case 0: value = "Beginning"
                case 1: value = "End"
                default: value = ["Index": ["index": max(0, Int(at.stringValue) ?? 0), "from_end": fromEnd.state == .on]]
                }
                at.isEnabled = p.indexOfSelectedItem == 2
                fromEnd.isEnabled = p.indexOfSelectedItem == 2
                self?.set("position", value)
            }
            for control in [p, at, fromEnd] as [NSControl] { fieldActions[ObjectIdentifier(control)] = apply }
            p.target = self
            p.action = #selector(controlChanged(_:))
            at.target = self
            at.action = #selector(controlChanged(_:))
            let row = NSStackView(views: [p, at, fromEnd])
            row.spacing = 4
            line("Position", row)
        }

        switch name {
        case "ReplaceText":
            line("Find", field("find"))
            line("Replace with", field("replace"))
            line("Occurrences", popup("occurrences", [("All", "All"), ("First", "First"), ("Last", "Last")], current: "\(body["occurrences"] ?? "All")"))
            let options = NSStackView(views: [
                check("Case sensitive", "case_sensitive", on: body["case_sensitive"] as? Bool ?? false),
                check("Regular Expression", "regex", on: body["regex"] as? Bool ?? false),
            ])
            line("", options)
            line("", check("Entire Text", "entire_text", on: body["entire_text"] as? Bool ?? false))
        case "AddText":
            line("Text", field("text"))
            position(default: "End")
        case "ChangeCase":
            line("Case", popup("case", [("Lower Case", "Lower"), ("Upper Case", "Upper"), ("Title Case", "Title"), ("Sentence Case", "Sentence")], current: "\(body["case"] ?? "Lower")"))
        case "AddSequence":
            line("Start", field("start", number: true))
            line("Step", field("step", number: true))
            line("Padding", field("padding", number: true))
            position(default: "Beginning")
            line("Prefix", field("prefix"))
            line("Suffix", field("suffix"))
        case "AddDate":
            // The source: a plain name, or {"Custom": ms} for a picked date.
            let source = body["source"] ?? "Modified"
            let custom = (source as? [String: Any])?["Custom"] as? NSNumber
            let sources = ["Now", "Created", "Modified", "Custom Date", "Metadata"]
            let sourcePopup = NSPopUpButton()
            sourcePopup.addItems(withTitles: sources)
            sourcePopup.selectItem(at: custom != nil ? 3 : (sources.firstIndex(of: "\(source)") ?? 2))
            let picker = NSDatePicker()
            picker.datePickerStyle = .textFieldAndStepper
            picker.datePickerElements = [.yearMonthDay, .hourMinuteSecond]
            picker.dateValue = custom.map { Date(timeIntervalSince1970: $0.doubleValue / 1000) } ?? Date()
            picker.isHidden = custom == nil
            picker.setAccessibilityLabel("Custom date")
            let applySource: (NSControl) -> Void = { [weak self, weak sourcePopup, weak picker] _ in
                guard let sourcePopup, let picker else { return }
                let chosen = sourcePopup.indexOfSelectedItem
                picker.isHidden = chosen != 3
                let value: Any = chosen == 3
                    ? ["Custom": Int64(picker.dateValue.timeIntervalSince1970 * 1000)]
                    : ["Now", "Created", "Modified", "", "Metadata"][chosen]
                self?.set("source", value)
            }
            for control in [sourcePopup, picker] as [NSControl] {
                control.target = self
                control.action = #selector(controlChanged(_:))
                fieldActions[ObjectIdentifier(control)] = applySource
            }
            let sourceRow = NSStackView(views: [sourcePopup, picker])
            sourceRow.spacing = 4
            line("Date", sourceRow)
            if custom == nil, "\(source)" == "Metadata" {
                let hint = NSTextField(labelWithString: "Photo capture date, else the content creation date.")
                hint.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
                hint.textColor = .secondaryLabelColor
                line("", hint)
            }
            let format = field("format")
            let presets = NSPopUpButton(frame: .zero, pullsDown: true)
            presets.addItem(withTitle: "Presets")
            presets.addItems(withTitles: Self.dateFormats)
            presets.setAccessibilityLabel("Date format presets")
            presets.target = self
            presets.action = #selector(controlChanged(_:))
            fieldActions[ObjectIdentifier(presets)] = { [weak self, weak format] control in
                guard let popup = control as? NSPopUpButton, popup.indexOfSelectedItem > 0,
                      let chosen = popup.titleOfSelectedItem
                else { return }
                format?.stringValue = chosen
                self?.set("format", chosen)
            }
            let formatRow = NSStackView(views: [format, presets])
            formatRow.spacing = 4
            line("Format", formatRow)
            position(default: "Beginning")
            line("Separator", field("separator"))
        case "AddMetadata":
            line("Field", popup("key", RenameMetadataReader.keys.map { ($0, $0) }, current: "\(body["key"] ?? "Dimensions")"))
            position(default: "End")
        default: break
        }
        line("Apply to", popup("target", [("Name", "Name"), ("Extension", "Extension"), ("Name with Extension", "NameWithExtension")], current: "\(rule["target"] ?? "Name")", inBody: false))
        let filters = NSStackView(views: [
            check("Files", "files", on: rule["files"] as? Bool ?? true, inBody: false),
            check("Folders", "folders", on: rule["folders"] as? Bool ?? true, inBody: false),
        ])
        line("", filters)
    }

    /// Forwards live typing in the editor's fields.
    private final class TextWatcher: NSObject, NSTextFieldDelegate {
        weak var owner: MultiRenameWindowController?
        func controlTextDidChange(_ notification: Notification) {
            guard let field = notification.object as? NSControl else { return }
            owner?.fieldActions[ObjectIdentifier(field)]?(field)
        }
    }

    private lazy var textWatcher: TextWatcher = {
        let watcher = TextWatcher()
        watcher.owner = self
        return watcher
    }()

    @objc private func controlChanged(_ sender: NSControl) {
        fieldActions[ObjectIdentifier(sender)]?(sender)
    }

    // MARK: Preview

    private var rulesJSON: String {
        (try? JSONSerialization.data(withJSONObject: rules)).flatMap { String(data: $0, encoding: .utf8) } ?? "[]"
    }

    private func refresh() {
        let result = AppEnvironment.shared.core.renamePreviewWithMetadata(
            connection: conn, items: items, metadata: items.map { metadata[$0.path] ?? RenameMetadata(dateMs: nil, values: [:]) },
            rulesJson: rulesJSON)
        preview = result
        previewTable.reloadData()
        footer.stringValue = result.error ?? result.summary
        // Nothing to do, or something flagged: no Rename.
        renameButton.isEnabled = result.canRename
    }

    @objc private func rename(_ sender: Any?) {
        guard let preview, preview.canRename else { return }
        let pairs: [RenamePair] = zip(items, preview.rows).filter { $0.1.changed }.map { item, row in
            let dir = (item.path as NSString).deletingLastPathComponent
            return RenamePair(from: item.path, to: (dir as NSString).appendingPathComponent(row.newName))
        }
        ActivityCenter.shared.submit(.multiRename(pairs: pairs), window: windowId, src: conn, dst: conn)
        close()
    }

    @objc private func cancel(_ sender: Any?) {
        close()
    }

    // MARK: Tables

    func numberOfRows(in tableView: NSTableView) -> Int {
        tableView === rulesTable ? rules.count : (preview?.rows.count ?? 0)
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let id = tableColumn?.identifier ?? .init("rule")
        let cell = (tableView.makeView(withIdentifier: id, owner: self) as? NSTableCellView) ?? {
            let cell = NSTableCellView()
            cell.identifier = id
            let text = NSTextField(labelWithString: "")
            text.lineBreakMode = .byTruncatingMiddle
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
        if tableView === rulesTable {
            let broken = preview?.brokenRules.contains(UInt32(row)) ?? false
            let enabled = rules[row]["enabled"] as? Bool ?? true
            let ruleCell = (tableView.makeView(withIdentifier: RuleCell.id, owner: self) as? RuleCell) ?? RuleCell()
            ruleCell.check.target = self
            ruleCell.check.action = #selector(ruleToggled(_:))
            ruleCell.check.tag = row
            ruleCell.check.state = enabled ? .on : .off
            ruleCell.check.setAccessibilityLabel("Use rule \(row + 1)")
            ruleCell.textField?.stringValue = describe(rules[row])
            // A rule that cannot work (an invalid regex) is marked red; one
            // switched off is dimmed.
            ruleCell.textField?.textColor = broken ? .systemRed : (enabled ? .labelColor : .tertiaryLabelColor)
            return ruleCell
        }
        guard let entry = preview?.rows[safe: row] else { return nil }
        let flagged = entry.invalid || entry.conflict
        switch id.rawValue {
        case "original":
            cell.textField?.stringValue = entry.original
            cell.textField?.textColor = .labelColor
        case "new":
            cell.textField?.stringValue = entry.newName
            cell.textField?.textColor = flagged ? .systemRed : (entry.changed ? .labelColor : .tertiaryLabelColor)
            cell.textField?.font = .systemFont(ofSize: NSFont.systemFontSize, weight: entry.changed ? .medium : .regular)
        default:
            cell.textField?.stringValue = entry.note
            cell.textField?.textColor = flagged ? .systemRed : .secondaryLabelColor
        }
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        if (notification.object as? NSTableView) === rulesTable { rebuildEditor() }
    }

    // Dragging preview rows (or rules) into another order.
    func tableView(_ tableView: NSTableView, pasteboardWriterForRow row: Int) -> NSPasteboardWriting? {
        let item = NSPasteboardItem()
        item.setString(String(row), forType: tableView === previewTable ? Self.rowType : Self.ruleType)
        return item
    }

    func tableView(_ tableView: NSTableView, validateDrop info: NSDraggingInfo, proposedRow row: Int, proposedDropOperation: NSTableView.DropOperation) -> NSDragOperation {
        proposedDropOperation == .above ? .move : []
    }

    func tableView(_ tableView: NSTableView, acceptDrop info: NSDraggingInfo, row: Int, dropOperation: NSTableView.DropOperation) -> Bool {
        let type = tableView === previewTable ? Self.rowType : Self.ruleType
        guard let text = info.draggingPasteboard.pasteboardItems?.first?.string(forType: type), let from = Int(text)
        else { return false }
        if tableView === previewTable { moveItem(from: from, to: row) } else { moveRule(from: from, to: row) }
        return true
    }

    /// Move a rule; `to` is the gap it lands in. Rules apply top to bottom.
    func moveRule(from: Int, to: Int) {
        guard rules.indices.contains(from) else { return }
        let rule = rules.remove(at: from)
        let landed = min(to > from ? to - 1 : to, rules.count)
        rules.insert(rule, at: landed)
        rulesTable.reloadData()
        rulesTable.selectRowIndexes([landed], byExtendingSelection: false)
        rebuildEditor()
        refresh()
    }

    @objc private func ruleToggled(_ sender: NSButton) {
        setEnabled(rule: sender.tag, sender.state == .on)
    }

    func setEnabled(rule row: Int, _ on: Bool) {
        guard rules.indices.contains(row) else { return }
        rules[row]["enabled"] = on
        rulesTable.reloadData(forRowIndexes: [row], columnIndexes: [0])
        refresh()
    }

    /// Type `value` into the editor field or pick it in the pop-up whose
    /// accessibility label (or row label) is `label` (scripted checks).
    func debugEdit(_ label: String, _ value: String) {
        func controls(_ view: NSView) -> [NSView] { view.subviews.flatMap { [$0] + controls($0) } }
        let all = controls(editor)
        // A row's label is the text field just before its control.
        var target: NSControl?
        if let labelled = all.first(where: { ($0 as? NSControl)?.accessibilityLabel() == label }) as? NSControl {
            target = labelled
        } else if let row = editor.arrangedSubviews.first(where: {
            (($0 as? NSStackView)?.arrangedSubviews.first as? NSTextField)?.stringValue == label
        }) as? NSStackView {
            target = controls(row).dropFirst().compactMap { $0 as? NSControl }.first
        }
        switch target {
        case let popup as NSPopUpButton:
            if let index = popup.itemTitles.firstIndex(of: value) { popup.selectItem(at: index) }
            controlChanged(popup)
        case let box as NSButton:
            box.state = value == "on" ? .on : .off
            controlChanged(box)
        case let picker as NSDatePicker:
            if let ms = Double(value) { picker.dateValue = Date(timeIntervalSince1970: ms / 1000) }
            controlChanged(picker)
        case let field as NSTextField:
            field.stringValue = value
            controlChanged(field)
        default: break
        }
    }

    /// Move a row; `to` is the gap it lands in (0...count).
    func moveItem(from: Int, to: Int) {
        guard items.indices.contains(from) else { return }
        let item = items.remove(at: from)
        items.insert(item, at: min(to > from ? to - 1 : to, items.count))
        refresh()
    }

    // MARK: Scripted checks

    /// Replace the rule list with JSON, as if built in the editor.
    func debugSetRules(_ json: String) {
        guard let data = json.data(using: .utf8),
              let parsed = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]]
        else { return }
        rules = parsed
        rulesTable.reloadData()
        if !rules.isEmpty { rulesTable.selectRowIndexes([0], byExtendingSelection: false) }
        rebuildEditor()
        refresh()
    }

    func debugRename() { rename(nil) }

    func debugAddRule(_ kind: String) {
        addRule(Self.defaultRule(kind: kind))
    }

    /// The rules as JSON (scripted checks).
    var debugRulesJSON: String { rulesJSON }

    func debugDump() -> String {
        let rows = (preview?.rows ?? []).map { row in
            "\(row.original)→\(row.newName)" + (row.conflict ? "(conflict)" : "") + (row.invalid ? "(invalid)" : "")
        }.joined(separator: ",")
        return "\(footer.stringValue) | \(rows) | rename=\(renameButton.isEnabled) rules=\(rules.count)"
    }
}

// MARK: - Presets as favorites

extension MultiRenameWindowController {
    /// Favorites › Add to Favorites while this window is in front: keep
    /// the rule list as a Multi-Rename preset (MR-08).
    @objc func addToFavorites(_ sender: Any?) {
        guard let window, !rules.isEmpty else { return }
        let draft = FavoriteDraftRecord(name: "Rename Preset", path: "", icon: "", kind: .renamePreset, server: nil, payload: presetJSON)
        AddFavoriteSheet(draft: draft, group: "Rename Presets").begin(on: window) { result in
            guard let result else { return }
            do {
                _ = try AppEnvironment.shared.core.addFavoriteDraft(group: result.group, draft: result.draft)
                AppEnvironment.shared.favoritesChanged()
            } catch {
                let alert = NSAlert()
                alert.messageText = (error as? RustyFinderError)?.message ?? "The preset could not be saved."
                alert.beginSheetModal(for: window)
            }
        }
    }

    /// The rule list as a preset stores it.
    var presetJSON: String {
        (try? JSONSerialization.data(withJSONObject: rules)).flatMap { String(data: $0, encoding: .utf8) } ?? "[]"
    }
}

/// A row of the rules list: a checkbox that turns the rule on or off, and
/// what the rule does.
private final class RuleCell: NSTableCellView {
    static let id = NSUserInterfaceItemIdentifier("ruleCell")
    let check = NSButton(checkboxWithTitle: "", target: nil, action: nil)

    init() {
        super.init(frame: .zero)
        identifier = Self.id
        let text = NSTextField(labelWithString: "")
        text.lineBreakMode = .byTruncatingMiddle
        for view in [check, text] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        textField = text
        NSLayoutConstraint.activate([
            check.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 4),
            check.centerYAnchor.constraint(equalTo: centerYAnchor),
            text.leadingAnchor.constraint(equalTo: check.trailingAnchor, constant: 2),
            text.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -4),
            text.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }
}
