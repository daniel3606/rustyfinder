import AppKit

/// Sync: compare two folders and carry out the plan. The comparison, the
/// rules for what counts as different and the plan itself come from the
/// Rust core; this window edits the options and shows the plan.
final class SyncWindowController: NSWindowController, NSWindowDelegate, NSTableViewDataSource, NSTableViewDelegate, NSTextFieldDelegate {
    /// The window that is open, if any (one at a time).
    private(set) static var current: SyncWindowController?

    /// One side of the pair: a folder on a connection.
    struct Side {
        let conn: UInt64
        let root: String
        /// How the location is shown in the header.
        let label: String
    }

    /// One exclude rule as edited: a kind from `excludeKinds` and its value.
    private struct ExcludeRule {
        var kind = 0
        var value = ""
    }

    private let left: Side
    private let right: Side
    private let windowId: UInt64

    // Options, mirroring `rustyfinder_core::sync::SyncOptions`.
    private var direction = "LeftToRight"
    private var flags: [String: Bool] = [
        "subfolders": true, "hidden": false, "add": true,
        "skip_empty_folders": false, "update": true, "delete_orphans": false,
    ]
    private var offsetMinutes = 0
    private var rightIsAhead = false
    private var excludes: [ExcludeRule] = []

    // The plan and what the user did to it.
    private var rows: [SyncRow] = []
    private var checked: [Bool] = []
    private var filter: SyncAction?
    /// Indices into `rows` that pass the footer filter.
    private var visible: [Int] = []
    /// The row (index into `rows`) the pointer rests on: shown expanded.
    private var hovered: Int?
    private var analyzing = false
    /// Bumped for every analysis, so a late answer to an old one is dropped.
    private var generation = 0
    private var listener: Listener?

    private let directionControl = NSSegmentedControl(
        labels: ["→", "←", "⇄"], trackingMode: .selectOne, target: nil, action: nil)
    private var checkboxes: [String: NSButton] = [:]
    private let offsetField = NSTextField(string: "0")
    private let aheadPopup = NSPopUpButton()
    private let excludeStack = NSStackView()
    private let table = KeyTableView()
    private let status = NSTextField(labelWithString: "")
    private var tallyButtons: [NSButton] = []
    private let syncButton = NSButton(title: "Synchronize", target: nil, action: nil)

    static func show(left: Side, right: Side, leftToRight: Bool, windowId: UInt64) {
        current?.close()
        let controller = SyncWindowController(left: left, right: right, leftToRight: leftToRight, windowId: windowId)
        current = controller
        controller.showWindow(nil)
        controller.analyze()
    }

    private init(left: Side, right: Side, leftToRight: Bool, windowId: UInt64) {
        self.left = left
        self.right = right
        self.windowId = windowId
        direction = leftToRight ? "LeftToRight" : "RightToLeft"
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 920, height: 560),
            styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.title = "Sync"
        window.isReleasedWhenClosed = false
        window.isRestorable = false
        window.minSize = NSSize(width: 760, height: 440)
        window.setFrameAutosaveName("RustyFinderSync")
        super.init(window: window)
        window.delegate = self
        buildUI()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func windowWillClose(_ notification: Notification) {
        generation += 1
        if Self.current === self { Self.current = nil }
    }

    // MARK: Layout

    private static let optionTitles: [(String, String)] = [
        ("subfolders", "Synchronize subfolders"),
        ("hidden", "Synchronize hidden items"),
        ("add", "Add items"),
        ("skip_empty_folders", "Don’t add empty folders"),
        ("update", "Update items"),
        ("delete_orphans", "Delete orphaned items"),
    ]

    private func buildUI() {
        guard let content = window?.contentView else { return }

        func pathLabel(_ text: String) -> NSTextField {
            let label = NSTextField(labelWithString: text)
            label.lineBreakMode = .byTruncatingHead
            label.font = .systemFont(ofSize: NSFont.systemFontSize, weight: .medium)
            label.toolTip = text
            label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            return label
        }
        directionControl.target = self
        directionControl.action = #selector(directionChanged(_:))
        directionControl.selectedSegment = direction == "LeftToRight" ? 0 : 1
        directionControl.setToolTip("Left to right", forSegment: 0)
        directionControl.setToolTip("Right to left", forSegment: 1)
        directionControl.setToolTip("Both ways", forSegment: 2)
        let leftLabel = pathLabel(left.label), rightLabel = pathLabel(right.label)
        rightLabel.alignment = .right
        let header = NSStackView(views: [leftLabel, directionControl, rightLabel])
        header.distribution = .fill
        header.spacing = 12
        // Both paths get the same room, each cut at its start if too long.
        leftLabel.widthAnchor.constraint(equalTo: rightLabel.widthAnchor).isActive = true

        // Options down the left edge.
        let options = NSStackView()
        options.orientation = .vertical
        options.alignment = .leading
        options.spacing = 6
        for (key, title) in Self.optionTitles {
            let box = NSButton(checkboxWithTitle: title, target: self, action: #selector(optionToggled(_:)))
            box.state = flags[key] == true ? .on : .off
            box.identifier = .init(key)
            checkboxes[key] = box
            options.addArrangedSubview(box)
        }
        options.setCustomSpacing(14, after: checkboxes["delete_orphans"]!)

        options.addArrangedSubview(NSTextField(labelWithString: "Time offset correction"))
        offsetField.delegate = self
        offsetField.alignment = .right
        offsetField.widthAnchor.constraint(equalToConstant: 52).isActive = true
        offsetField.setAccessibilityLabel("Time offset in minutes")
        aheadPopup.addItems(withTitles: ["left is ahead (minutes)", "right is ahead (minutes)"])
        aheadPopup.target = self
        aheadPopup.action = #selector(aheadChanged(_:))
        let offsetRow = NSStackView(views: [offsetField, aheadPopup])
        offsetRow.spacing = 6
        options.addArrangedSubview(offsetRow)
        options.setCustomSpacing(14, after: offsetRow)

        let excludeTitle = NSTextField(labelWithString: "Exclude items")
        let addExclude = NSButton(title: "+", target: self, action: #selector(addExcludeRule(_:)))
        addExclude.bezelStyle = .smallSquare
        addExclude.setAccessibilityLabel("Add exclude rule")
        let excludeHeader = NSStackView(views: [excludeTitle, addExclude])
        excludeHeader.spacing = 8
        options.addArrangedSubview(excludeHeader)
        excludeStack.orientation = .vertical
        excludeStack.alignment = .leading
        excludeStack.spacing = 4
        options.addArrangedSubview(excludeStack)
        options.widthAnchor.constraint(equalToConstant: 300).isActive = true

        // The plan.
        for (id, title, width) in [("on", "", 24.0), ("path", "Item", 330.0), ("action", "Action", 70.0), ("arrow", "", 34.0)] {
            let column = NSTableColumn(identifier: .init(id))
            column.title = title
            column.width = width
            if id != "path" { column.resizingMask = [] }
            table.addTableColumn(column)
        }
        table.dataSource = self
        table.delegate = self
        table.rowHeight = 20
        table.allowsMultipleSelection = true
        table.usesAlternatingRowBackgroundColors = true
        table.setAccessibilityLabel("Planned actions")
        // Space switches the selected rows on or off.
        table.onSpace = { [weak self] in self?.toggleSelectedRows() }
        // Resting the pointer on a row expands it to show both sides.
        table.onHover = { [weak self] row in self?.hover(row.flatMap { self?.visible[safe: $0] }) }
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder

        for (tag, title) in ["All", "Added", "Updated", "Deleted"].enumerated() {
            let button = NSButton(title: title, target: self, action: #selector(tallyClicked(_:)))
            button.tag = tag
            button.bezelStyle = .recessed
            button.setButtonType(.pushOnPushOff)
            // The border appears under the pointer, in both appearances.
            button.showsBorderOnlyWhileMouseInside = true
            tallyButtons.append(button)
        }
        let tallies = NSStackView(views: tallyButtons)
        tallies.spacing = 4

        status.textColor = .secondaryLabelColor
        status.lineBreakMode = .byTruncatingTail
        syncButton.target = self
        syncButton.action = #selector(synchronize(_:))
        syncButton.keyEquivalent = "\r"
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancel(_:)))
        cancel.keyEquivalent = "\u{1b}"
        let buttons = NSStackView(views: [status, NSView(), cancel, syncButton])
        buttons.spacing = 8

        let rightColumn = NSStackView(views: [scroll, tallies])
        rightColumn.orientation = .vertical
        rightColumn.alignment = .leading
        rightColumn.spacing = 8
        let middle = NSStackView(views: [options, rightColumn])
        middle.alignment = .top
        middle.spacing = 14

        let root = NSStackView(views: [header, middle, buttons])
        root.orientation = .vertical
        root.spacing = 12
        root.edgeInsets = NSEdgeInsets(top: 14, left: 14, bottom: 12, right: 14)
        root.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(root)
        NSLayoutConstraint.activate([
            root.topAnchor.constraint(equalTo: content.topAnchor),
            root.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            root.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            root.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            header.widthAnchor.constraint(equalTo: root.widthAnchor, constant: -28),
            buttons.widthAnchor.constraint(equalTo: root.widthAnchor, constant: -28),
            scroll.widthAnchor.constraint(equalTo: rightColumn.widthAnchor),
            scroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 260),
        ])
        refreshControls()
    }

    // MARK: Options

    @objc private func directionChanged(_ sender: Any?) {
        direction = ["LeftToRight", "RightToLeft", "BothWays"][max(0, directionControl.selectedSegment)]
        // Deleting orphans only exists one way.
        if direction == "BothWays" { setFlag("delete_orphans", false) }
        refreshControls()
        analyze()
    }

    @objc private func optionToggled(_ sender: NSButton) {
        guard let key = sender.identifier?.rawValue else { return }
        let on = sender.state == .on
        // Switching deletion on asks first: it cannot be undone.
        if key == "delete_orphans", on, let window {
            sender.state = .off
            let alert = NSAlert()
            alert.alertStyle = .warning
            alert.messageText = "Delete orphaned items?"
            alert.informativeText = "Items in the target folder that don’t exist in the source will be permanently deleted when you synchronize. This can’t be undone."
            alert.addButton(withTitle: "Turn On").hasDestructiveAction = true
            alert.addButton(withTitle: "Cancel")
            alert.beginSheetModal(for: window) { [weak self] response in
                guard response == .alertFirstButtonReturn else { return }
                self?.setFlag("delete_orphans", true)
                self?.analyze()
            }
            return
        }
        flags[key] = on
        analyze()
    }

    private func setFlag(_ key: String, _ on: Bool) {
        flags[key] = on
        checkboxes[key]?.state = on ? .on : .off
    }

    @objc private func aheadChanged(_ sender: Any?) {
        rightIsAhead = aheadPopup.indexOfSelectedItem == 1
        analyze()
    }

    /// The offset takes whole minutes from 0 to 1440. An empty field counts
    /// as 0 and stays empty, so the last digit can be cleared.
    func controlTextDidChange(_ notification: Notification) {
        guard let field = notification.object as? NSTextField else { return }
        if field === offsetField {
            let digits = field.stringValue.filter(\.isNumber)
            let value = min(Int(digits) ?? 0, 1440)
            let text = digits.isEmpty ? "" : String(value)
            if field.stringValue != text { field.stringValue = text }
            guard value != offsetMinutes else { return }
            offsetMinutes = value
            analyze()
        } else if excludes.indices.contains(field.tag) {
            excludes[field.tag].value = field.stringValue
            analyze()
        }
    }

    private func refreshControls() {
        checkboxes["delete_orphans"]?.isEnabled = direction != "BothWays"
        directionControl.selectedSegment = ["LeftToRight", "RightToLeft", "BothWays"].firstIndex(of: direction) ?? 0
    }

    // MARK: Exclude rules

    /// What a rule can test, in menu order.
    private static let excludeKinds = [
        "Name is", "Name contains", "Name begins with", "Name ends with",
        "Name matches wildcard", "Name matches regex", "Extension is", "Kind is",
        "Size greater than", "Size less than",
        "Modified before", "Modified after", "Modified within the last (days)",
        "Has tag", "Doesn't have tag",
    ]

    private static let excludePlaceholders: [Int: String] = [
        6: "log, tmp", 7: "Folder, Image, Movie…", 8: "10 MB", 9: "10 MB",
        10: "2026-01-31", 11: "2026-01-31", 12: "7", 13: "Red", 14: "Red",
    ]

    @objc private func addExcludeRule(_ sender: Any?) {
        excludes.append(ExcludeRule(kind: 1))
        rebuildExcludes()
    }

    @objc private func removeExcludeRule(_ sender: NSButton) {
        guard excludes.indices.contains(sender.tag) else { return }
        excludes.remove(at: sender.tag)
        rebuildExcludes()
        analyze()
    }

    @objc private func excludeKindChanged(_ sender: NSPopUpButton) {
        guard excludes.indices.contains(sender.tag) else { return }
        excludes[sender.tag].kind = sender.indexOfSelectedItem
        rebuildExcludes()
        analyze()
    }

    private func rebuildExcludes() {
        excludeStack.arrangedSubviews.forEach {
            excludeStack.removeArrangedSubview($0)
            $0.removeFromSuperview()
        }
        for (index, rule) in excludes.enumerated() {
            let popup = NSPopUpButton()
            popup.controlSize = .small
            popup.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
            popup.addItems(withTitles: Self.excludeKinds)
            popup.selectItem(at: rule.kind)
            popup.tag = index
            popup.target = self
            popup.action = #selector(excludeKindChanged(_:))
            popup.widthAnchor.constraint(equalToConstant: 150).isActive = true
            let field = NSTextField(string: rule.value)
            field.controlSize = .small
            field.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
            field.placeholderString = Self.excludePlaceholders[rule.kind]
            field.tag = index
            field.delegate = self
            field.widthAnchor.constraint(equalToConstant: 100).isActive = true
            let remove = NSButton(title: "−", target: self, action: #selector(removeExcludeRule(_:)))
            remove.bezelStyle = .smallSquare
            remove.controlSize = .small
            remove.tag = index
            remove.setAccessibilityLabel("Remove exclude rule")
            let row = NSStackView(views: [popup, field, remove])
            row.spacing = 4
            excludeStack.addArrangedSubview(row)
        }
    }

    /// The rule in the shape the core reads, or nil while its value is
    /// empty or not yet a valid one (a half-typed rule is simply inactive).
    private func json(for rule: ExcludeRule) -> Any? {
        let value = rule.value.trimmingCharacters(in: .whitespaces)
        guard !value.isEmpty else { return nil }
        switch rule.kind {
        case 0...5:
            let op = ["Is", "Contains", "BeginsWith", "EndsWith", "Wildcard", "Regex"][rule.kind]
            return ["Name": [op, value]]
        case 6:
            let list = value.split(separator: ",")
                .map { $0.trimmingCharacters(in: .whitespaces).trimmingCharacters(in: ["."]) }
                .filter { !$0.isEmpty }
            return list.isEmpty ? nil : ["Extension": list]
        case 7:
            let kinds = ["Folder", "Image", "Movie", "Audio", "Document", "Archive", "Application", "Text"]
            guard let kind = kinds.first(where: { $0.lowercased() == value.lowercased() }) else { return nil }
            return ["Kind": kind]
        case 8, 9:
            guard let bytes = Self.parseSize(value) else { return nil }
            return ["Size": ["greater": rule.kind == 8, "bytes": bytes]]
        case 10, 11:
            let parser = DateFormatter()
            parser.locale = Locale(identifier: "en_US_POSIX")
            parser.dateFormat = "yyyy-MM-dd"
            guard let date = parser.date(from: value) else { return nil }
            return ["Modified": ["before": rule.kind == 10, "ms": Int64(date.timeIntervalSince1970 * 1000)]]
        case 13, 14:
            return ["Tags": ["has": rule.kind == 13, "tag": value]]
        default:
            guard let days = Int(value), days > 0 else { return nil }
            let since = Date().addingTimeInterval(-Double(days) * 86400)
            return ["Modified": ["before": false, "ms": Int64(since.timeIntervalSince1970 * 1000)]]
        }
    }

    /// "10", "10 KB", "1.5mb", "2 GB" as bytes.
    static func parseSize(_ text: String) -> UInt64? {
        let lower = text.lowercased().replacingOccurrences(of: " ", with: "")
        let units: [(String, Double)] = [("gb", 1e9), ("mb", 1e6), ("kb", 1e3), ("bytes", 1), ("b", 1)]
        for (suffix, factor) in units where lower.hasSuffix(suffix) {
            guard let number = Double(lower.dropLast(suffix.count)), number >= 0 else { return nil }
            return UInt64(number * factor)
        }
        guard let number = Double(lower), number >= 0 else { return nil }
        return UInt64(number)
    }

    private var optionsJSON: String {
        var object: [String: Any] = flags
        object["direction"] = direction
        object["offset_minutes"] = offsetMinutes
        object["right_is_ahead"] = rightIsAhead
        object["excludes"] = excludes.compactMap(json(for:))
        return (try? JSONSerialization.data(withJSONObject: object)).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
    }

    // MARK: Analysis

    /// Called by the core on a worker thread; hops to the main thread and
    /// drops answers to an analysis that has been superseded.
    private final class Listener: SyncListener, @unchecked Sendable {
        weak var owner: SyncWindowController?
        let generation: Int

        init(owner: SyncWindowController, generation: Int) {
            self.owner = owner
            self.generation = generation
        }

        func progress(folders: UInt64) {
            DispatchQueue.main.async { [self] in
                guard let owner, owner.generation == generation else { return }
                owner.status.stringValue = "\(folders) folder\(folders == 1 ? "" : "s") analyzed"
            }
        }

        func finished(plan: SyncPlan) {
            DispatchQueue.main.async { [self] in
                guard let owner, owner.generation == generation else { return }
                owner.show(plan)
            }
        }
    }

    private func analyze() {
        generation += 1
        analyzing = true
        status.stringValue = "Analyzing…"
        refreshButtons()
        let listener = Listener(owner: self, generation: generation)
        self.listener = listener
        AppEnvironment.shared.core.syncAnalyze(
            leftConnection: left.conn, leftRoot: left.root,
            rightConnection: right.conn, rightRoot: right.root,
            optionsJson: optionsJSON, listener: listener)
    }

    private func show(_ plan: SyncPlan) {
        analyzing = false
        rows = plan.rows
        hovered = nil
        checked = plan.rows.map(\.checked)
        if let error = plan.error {
            status.stringValue = error
        } else if plan.rows.isEmpty {
            status.stringValue = "The folders are in sync."
        } else {
            status.stringValue = "\(plan.foldersAnalyzed) folder\(plan.foldersAnalyzed == 1 ? "" : "s") analyzed"
        }
        applyFilter()
    }

    private func applyFilter() {
        visible = rows.indices.filter { filter == nil || rows[$0].action == filter }
        table.reloadData()
        refreshButtons()
    }

    private func refreshButtons() {
        let actions: [SyncAction?] = [nil, .add, .update, .delete]
        for (button, action) in zip(tallyButtons, actions) {
            let matching = rows.filter { action == nil || $0.action == action }
            let bytes = matching.reduce(UInt64(0)) { $0 + $1.size }
            let name = ["All", "Added", "Updated", "Deleted"][button.tag]
            button.title = "\(name): \(matching.count) (\(RowPresenter.shared.bytes(bytes)))"
            button.state = filter == action ? .on : .off
        }
        // Nothing to run while analysing, with no plan, or with every row off.
        syncButton.isEnabled = !analyzing && checked.contains(true)
    }

    @objc private func tallyClicked(_ sender: NSButton) {
        filter = [nil, .add, .update, .delete][sender.tag]
        applyFilter()
    }

    // MARK: Rows

    @objc private func rowToggled(_ sender: NSButton) {
        guard checked.indices.contains(sender.tag) else { return }
        checked[sender.tag] = sender.state == .on
        refreshButtons()
    }

    /// Space: if any selected row is on, switch them all off; else all on.
    private func toggleSelectedRows() {
        let picked = table.selectedRowIndexes.compactMap { visible[safe: $0] }
        guard !picked.isEmpty else { return }
        let turnOn = !picked.contains { checked[$0] }
        picked.forEach { checked[$0] = turnOn }
        table.reloadData(forRowIndexes: table.selectedRowIndexes, columnIndexes: [0])
        refreshButtons()
    }

    func numberOfRows(in tableView: NSTableView) -> Int { visible.count }

    private func detail(_ row: SyncRow) -> String {
        func side(_ name: String, _ root: String, _ size: UInt64?, _ modified: Int64?) -> String {
            let path = (root as NSString).appendingPathComponent(row.path)
            guard let size else { return "\(name): \(path)\n    not there" }
            let presenter = RowPresenter.shared
            return "\(name): \(path)\n    \(row.isDir ? "Folder" : presenter.bytes(size))  \(presenter.date(modified))"
        }
        return side("Left", left.root, row.leftSize, row.leftModifiedMs) + "\n"
            + side("Right", right.root, row.rightSize, row.rightModifiedMs)
    }

    private func actionName(_ row: SyncRow) -> String {
        let name: String
        switch row.action {
        case .add: name = "Add"
        case .update: name = "Update"
        case .delete: name = "Delete"
        }
        return row.conflict ? name + " (conflict)" : name
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let index = visible[safe: row], let id = tableColumn?.identifier else { return nil }
        let entry = rows[index]
        if id.rawValue == "on" {
            let box = (tableView.makeView(withIdentifier: id, owner: self) as? NSButton) ?? {
                let box = NSButton(checkboxWithTitle: "", target: self, action: #selector(rowToggled(_:)))
                box.identifier = id
                return box
            }()
            box.tag = index
            box.state = checked[index] ? .on : .off
            box.setAccessibilityLabel("Include \(entry.path)")
            return box
        }
        let cell = (tableView.makeView(withIdentifier: id, owner: self) as? NSTableCellView) ?? {
            let cell = NSTableCellView()
            cell.identifier = id
            let text = NSTextField(labelWithString: "")
            text.lineBreakMode = .byTruncatingMiddle
            text.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(text)
            cell.textField = text
            NSLayoutConstraint.activate([
                text.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 2),
                text.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -2),
                text.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
            return cell
        }()
        let expanded = hovered == index
        cell.textField?.maximumNumberOfLines = expanded && id.rawValue == "path" ? 0 : 1
        switch id.rawValue {
        case "path":
            // Expanded: both sides' full path, date and size under the name.
            let name = entry.path + (entry.isDir ? "/" : "")
            if expanded {
                let text = NSMutableAttributedString(string: name + "\n", attributes: [.foregroundColor: NSColor.labelColor])
                text.append(NSAttributedString(string: detail(entry), attributes: [
                    .foregroundColor: NSColor.secondaryLabelColor,
                    .font: NSFont.systemFont(ofSize: NSFont.smallSystemFontSize),
                ]))
                cell.textField?.attributedStringValue = text
                cell.textField?.lineBreakMode = .byCharWrapping
            } else {
                cell.textField?.stringValue = name
                cell.textField?.textColor = .labelColor
                cell.textField?.lineBreakMode = .byTruncatingMiddle
            }
        case "action":
            cell.textField?.stringValue = actionName(entry)
            cell.textField?.textColor = entry.conflict || entry.action == .delete ? .systemRed : .secondaryLabelColor
        default:
            cell.textField?.stringValue = entry.toRight ? "→" : "←"
            cell.textField?.alignment = .center
            cell.textField?.textColor = .secondaryLabelColor
        }
        // Resting the pointer on a row shows both sides in full.
        cell.toolTip = detail(entry)
        return cell
    }

    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
        visible[safe: row] == hovered && hovered != nil ? Self.expandedHeight : 20
    }

    private static let expandedHeight: CGFloat = 84

    /// Expand the row the pointer rests on (and collapse the previous one).
    private func hover(_ index: Int?) {
        guard index != hovered else { return }
        let before = hovered.flatMap { visible.firstIndex(of: $0) }
        hovered = index
        let after = index.flatMap { visible.firstIndex(of: $0) }
        let changed = IndexSet([before, after].compactMap { $0 })
        table.noteHeightOfRows(withIndexesChanged: changed)
        table.reloadData(forRowIndexes: changed, columnIndexes: IndexSet(integersIn: 0..<table.numberOfColumns))
    }

    // MARK: Buttons

    @objc private func synchronize(_ sender: Any?) {
        guard !analyzing else { return }
        let steps = zip(rows, checked).filter(\.1).map { row, _ in
            SyncStepRecord(path: row.path, action: row.action, toRight: row.toRight, isDir: row.isDir)
        }
        guard !steps.isEmpty else { return }
        ActivityCenter.shared.submit(
            .sync(leftRoot: left.root, rightRoot: right.root, steps: steps),
            window: windowId, src: left.conn, dst: right.conn)
        close()
    }

    @objc private func cancel(_ sender: Any?) {
        close()
    }

    // MARK: Scripted checks

    /// Drive the window as a user would: `subfolders`, `hidden`, `add`,
    /// `skipempty`, `update`, `delete` click a checkbox; `dir=left|right|both`;
    /// `offset=<text>` types into the offset field; `ahead=left|right`;
    /// `filter=all|added|updated|deleted`; `uncheck=all|<path>`, `check=all`;
    /// `exclude=<ext|name|size>:<value>`; `run`.
    func debugSet(_ what: String) {
        let parts = what.split(separator: "=", maxSplits: 1).map(String.init)
        let value = parts.count > 1 ? parts[1] : ""
        let keys = ["skipempty": "skip_empty_folders", "delete": "delete_orphans"]
        if parts.count == 1, let box = checkboxes[keys[what] ?? what] {
            box.performClick(nil)
            return
        }
        switch parts[0] {
        case "dir":
            directionControl.selectedSegment = ["right": 0, "left": 1, "both": 2][value] ?? 0
            directionChanged(nil)
        case "offset":
            offsetField.stringValue = value
            controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: offsetField))
        case "ahead":
            aheadPopup.selectItem(at: value == "right" ? 1 : 0)
            aheadChanged(nil)
        case "filter":
            let index = ["all", "added", "updated", "deleted"].firstIndex(of: value) ?? 0
            tallyButtons[index].performClick(nil)
        case "uncheck", "check":
            for index in rows.indices where value == "all" || rows[index].path == value {
                checked[index] = parts[0] == "check"
            }
            table.reloadData()
            refreshButtons()
        case "exclude":
            let pair = value.split(separator: ":", maxSplits: 1).map(String.init)
            guard pair.count == 2 else { return }
            excludes.append(ExcludeRule(kind: ["name": 1, "ext": 6, "size": 8][pair[0]] ?? 1, value: pair[1]))
            rebuildExcludes()
            analyze()
        case "run":
            syncButton.performClick(nil)
        case "hover":
            hover(rows.firstIndex { $0.path == value })
        case "tag":
            excludes.append(ExcludeRule(kind: value.hasPrefix("!") ? 14 : 13, value: String(value.drop { $0 == "!" })))
            rebuildExcludes()
            analyze()
        default:
            break
        }
    }

    func debugDump() -> String {
        let list = visible.map { index -> String in
            let row = rows[index]
            return "\(row.path):\(actionName(row)):\(row.toRight ? ">" : "<"):\(checked[index] ? "on" : "off")"
        }.joined(separator: ",")
        let tallies = tallyButtons.map(\.title).joined(separator: " | ")
        let delete = flags["delete_orphans"] == true
        var hover = "-"
        if let index = hovered, let row = visible.firstIndex(of: index) {
            let cell = table.view(atColumn: table.column(withIdentifier: .init("path")), row: row, makeIfNecessary: true)
            let text = (cell as? NSTableCellView)?.textField?.stringValue ?? ""
            hover = "\(rows[index].path) height=\(Int(table.rect(ofRow: row).height)) "
                + text.replacingOccurrences(of: "\n", with: " / ")
        }
        return "dir=\(direction) busy=\(analyzing) delete=\(delete) offset=\(offsetMinutes)“\(offsetField.stringValue)” "
            + "rows=[\(list)] \(tallies) sync=\(syncButton.isEnabled) status=\(status.stringValue) hover=\(hover)"
    }
}

/// A table that reports Space, and the row under the pointer, to its owner.
final class KeyTableView: NSTableView {
    var onSpace: (() -> Void)?
    var onHover: ((Int?) -> Void)?
    private var hoverArea: NSTrackingArea?

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let hoverArea { removeTrackingArea(hoverArea) }
        let area = NSTrackingArea(
            rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
            owner: self, userInfo: nil)
        addTrackingArea(area)
        hoverArea = area
    }

    override func mouseMoved(with event: NSEvent) {
        super.mouseMoved(with: event)
        let row = row(at: convert(event.locationInWindow, from: nil))
        onHover?(row >= 0 ? row : nil)
    }

    override func mouseExited(with event: NSEvent) {
        super.mouseExited(with: event)
        onHover?(nil)
    }

    override func keyDown(with event: NSEvent) {
        if event.charactersIgnoringModifiers == " ", let onSpace {
            onSpace()
        } else {
            super.keyDown(with: event)
        }
    }
}

// MARK: - Synclets (a sync's folders and options saved as a favorite)

extension SyncWindowController {
    /// The options as a synclet stores them.
    private var syncletOptions: String {
        var object: [String: Any] = flags
        object["direction"] = direction
        object["offset_minutes"] = offsetMinutes
        object["right_is_ahead"] = rightIsAhead
        object["excludes"] = excludes.map { [$0.kind, $0.value] as [Any] }
        return (try? JSONSerialization.data(withJSONObject: object)).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
    }

    /// Favorites › Add to Favorites while this window is in front: save
    /// both folders and every option as a synclet (local folders only).
    @objc func addToFavorites(_ sender: Any?) {
        guard let window else { return }
        guard left.conn == 0, right.conn == 0 else {
            let alert = NSAlert()
            alert.messageText = "Only syncs between folders on this Mac can be saved as synclets."
            alert.beginSheetModal(for: window)
            return
        }
        let object: [String: Any] = ["left": left.root, "right": right.root, "direction": direction, "options": syncletOptions]
        guard let data = try? JSONSerialization.data(withJSONObject: object), let json = String(data: data, encoding: .utf8) else { return }
        let name = "\(left.root.folderDisplayName) ⇄ \(right.root.folderDisplayName)"
        let draft = FavoriteDraftRecord(name: name, path: left.root, icon: "", kind: .synclet, server: nil, payload: json)
        AddFavoriteSheet(draft: draft, group: "Synclets").begin(on: window) { result in
            guard let result else { return }
            do {
                _ = try AppEnvironment.shared.core.addFavoriteDraft(group: result.group, draft: result.draft)
                AppEnvironment.shared.favoritesChanged()
            } catch {
                let alert = NSAlert()
                alert.messageText = (error as? RustyFinderError)?.message ?? "The synclet could not be saved."
                alert.beginSheetModal(for: window)
            }
        }
    }

    /// Put a synclet's options back. Deleting orphans is asked for again,
    /// as when it is switched on by hand.
    func applySynclet(optionsJSON: String) {
        guard let data = optionsJSON.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return }
        for key in flags.keys where key != "delete_orphans" {
            if let on = object[key] as? Bool { setFlag(key, on) }
        }
        if let saved = object["direction"] as? String, let index = ["LeftToRight", "RightToLeft", "BothWays"].firstIndex(of: saved) {
            directionControl.selectedSegment = index
            direction = saved
            refreshControls()
        }
        offsetMinutes = min(max(object["offset_minutes"] as? Int ?? 0, 0), 1440)
        offsetField.stringValue = String(offsetMinutes)
        rightIsAhead = object["right_is_ahead"] as? Bool ?? false
        aheadPopup.selectItem(at: rightIsAhead ? 1 : 0)
        excludes = ((object["excludes"] as? [[Any]]) ?? []).compactMap { pair in
            guard pair.count == 2, let kind = pair[0] as? Int, let value = pair[1] as? String else { return nil }
            return ExcludeRule(kind: kind, value: value)
        }
        rebuildExcludes()
        analyze()
        if object["delete_orphans"] as? Bool == true, direction != "BothWays", let box = checkboxes["delete_orphans"] {
            box.state = .on
            optionToggled(box)
        }
    }
}
