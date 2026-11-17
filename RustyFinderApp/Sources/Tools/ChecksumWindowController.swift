import AppKit

/// Checksums of the selected files: one row per file, an algorithm
/// selector, and a field to compare against a pasted digest.
final class ChecksumWindowController: NSWindowController, NSWindowDelegate, NSTableViewDataSource, NSTableViewDelegate, NSTextFieldDelegate {
    private(set) static var current: ChecksumWindowController?
    private static let algorithms: [ChecksumAlgorithm] = [.md5, .sha1, .sha256, .sha512]

    private struct Row {
        let name: String
        let path: String
        let size: UInt64
        var digest: String?
        var error: String?
        var bytes: UInt64 = 0
    }

    private var rows: [Row]
    private let conn: UInt64
    private var algorithm = ChecksumAlgorithm.sha256
    private var job: ChecksumJob?
    /// Bumped per computation, so a late answer to an old one is dropped.
    private var generation = 0
    private let table = CopyTableView()
    private let popup = NSPopUpButton()
    private let compareField = NSTextField()

    static func show(items: [InspectedItem], conn: UInt64) {
        current?.close()
        let controller = ChecksumWindowController(items: items, conn: conn)
        current = controller
        controller.showWindow(nil)
        controller.compute()
    }

    private init(items: [InspectedItem], conn: UInt64) {
        rows = items.map { Row(name: ($0.path as NSString).lastPathComponent, path: $0.path, size: $0.row.size) }
        self.conn = conn
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 760, height: 320),
            styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.title = "Checksum"
        window.isReleasedWhenClosed = false
        window.isRestorable = false
        window.setFrameAutosaveName("RustyFinderChecksum")
        super.init(window: window)
        window.delegate = self
        buildUI()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func windowWillClose(_ notification: Notification) {
        // Closing cancels whatever is still being read.
        job?.cancel()
        generation += 1
        if Self.current === self { Self.current = nil }
    }

    private func buildUI() {
        guard let content = window?.contentView else { return }
        popup.addItems(withTitles: Self.algorithms.map { checksumName(algorithm: $0) })
        popup.selectItem(at: 2)
        popup.target = self
        popup.action = #selector(algorithmChanged(_:))
        compareField.placeholderString = "Paste a checksum to compare"
        compareField.delegate = self
        compareField.setAccessibilityLabel("Compare with")
        let copy = NSButton(title: "Copy", target: self, action: #selector(copyRows(_:)))
        let top = NSStackView(views: [NSTextField(labelWithString: "Algorithm:"), popup, compareField, copy])
        top.spacing = 8

        for (id, title, width) in [("name", "Name", 200.0), ("algorithm", "Algorithm", 80.0), ("digest", "Checksum", 420.0)] {
            let column = NSTableColumn(identifier: .init(id))
            column.title = title
            column.width = width
            table.addTableColumn(column)
        }
        table.dataSource = self
        table.delegate = self
        table.allowsMultipleSelection = true
        table.rowHeight = 20
        table.usesAlternatingRowBackgroundColors = true
        table.setAccessibilityLabel("Checksums")
        table.onCopy = { [weak self] in self?.copyRows(nil) }
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder

        let root = NSStackView(views: [top, scroll])
        root.orientation = .vertical
        root.spacing = 10
        root.edgeInsets = NSEdgeInsets(top: 12, left: 12, bottom: 12, right: 12)
        root.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(root)
        NSLayoutConstraint.activate([
            root.topAnchor.constraint(equalTo: content.topAnchor),
            root.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            root.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            root.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            top.widthAnchor.constraint(equalTo: root.widthAnchor, constant: -24),
        ])
    }

    // MARK: Computing

    private final class Listener: ChecksumListener, @unchecked Sendable {
        weak var owner: ChecksumWindowController?
        let generation: Int

        init(owner: ChecksumWindowController, generation: Int) {
            self.owner = owner
            self.generation = generation
        }

        func progress(index: UInt32, bytes: UInt64) {
            DispatchQueue.main.async { [self] in
                guard let owner, owner.generation == generation, owner.rows.indices.contains(Int(index)) else { return }
                owner.rows[Int(index)].bytes = bytes
                owner.table.reloadData(forRowIndexes: [Int(index)], columnIndexes: [2])
            }
        }

        func finished(index: UInt32, digest: String?, error: String?) {
            DispatchQueue.main.async { [self] in
                guard let owner, owner.generation == generation, owner.rows.indices.contains(Int(index)) else { return }
                owner.rows[Int(index)].digest = digest
                owner.rows[Int(index)].error = error
                owner.table.reloadData(forRowIndexes: [Int(index)], columnIndexes: IndexSet(0..<3))
            }
        }
    }

    private func compute() {
        job?.cancel()
        generation += 1
        for index in rows.indices {
            rows[index].digest = nil
            rows[index].error = nil
            rows[index].bytes = 0
        }
        table.reloadData()
        job = AppEnvironment.shared.core.checksum(
            connection: conn, paths: rows.map(\.path), algorithm: algorithm,
            listener: Listener(owner: self, generation: generation))
    }

    @objc private func algorithmChanged(_ sender: Any?) {
        algorithm = Self.algorithms[max(0, popup.indexOfSelectedItem)]
        compute()
    }

    func controlTextDidChange(_ notification: Notification) {
        table.reloadData()
    }

    private func matches(_ row: Row) -> Bool {
        guard let digest = row.digest else { return false }
        return checksumMatches(digest: digest, pasted: compareField.stringValue)
    }

    /// The selected rows (all, if none is selected) as CSV.
    @objc private func copyRows(_ sender: Any?) {
        let picked = table.selectedRowIndexes.isEmpty ? IndexSet(rows.indices) : table.selectedRowIndexes
        let done = picked.compactMap { index -> ChecksumRow? in
            guard let digest = rows[index].digest else { return nil }
            return ChecksumRow(name: rows[index].name, algorithm: algorithm, digest: digest)
        }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(checksumCsv(rows: done), forType: .string)
    }

    // MARK: Table

    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let id = tableColumn?.identifier else { return nil }
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
        let entry = rows[row]
        let matched = matches(entry)
        cell.textField?.font = id.rawValue == "digest"
            ? .monospacedSystemFont(ofSize: 11, weight: matched ? .bold : .regular)
            : .systemFont(ofSize: NSFont.systemFontSize)
        cell.textField?.textColor = matched ? .systemGreen : .labelColor
        switch id.rawValue {
        case "name": cell.textField?.stringValue = entry.name
        case "algorithm": cell.textField?.stringValue = checksumName(algorithm: algorithm)
        default:
            if let digest = entry.digest {
                cell.textField?.stringValue = digest
            } else if let error = entry.error {
                cell.textField?.stringValue = error
                cell.textField?.textColor = .systemRed
            } else if entry.size > 0, entry.bytes > 0 {
                cell.textField?.stringValue = "\(Int(Double(entry.bytes) / Double(entry.size) * 100)) %"
            } else {
                cell.textField?.stringValue = "…"
            }
        }
        return cell
    }

    // MARK: Scripted checks

    /// `algo=<md5|sha1|sha256|sha512>`, `compare=<text>`, `copy`.
    func debugSet(_ what: String) {
        let parts = what.split(separator: "=", maxSplits: 1).map(String.init)
        let value = parts.count > 1 ? parts[1] : ""
        switch parts[0] {
        case "algo":
            popup.selectItem(at: ["md5", "sha1", "sha256", "sha512"].firstIndex(of: value) ?? 2)
            algorithmChanged(nil)
        case "compare":
            compareField.stringValue = value
            table.reloadData()
        case "copy":
            table.selectAll(nil)
            copyRows(nil)
        default: break
        }
    }

    func debugDump() -> String {
        rows.map { "\($0.name)=\($0.digest ?? $0.error ?? "…")\(matches($0) ? "(match)" : "")" }.joined(separator: "|")
            + " algo=\(checksumName(algorithm: algorithm))"
    }
}

/// A table whose rows can be copied with Cmd-C.
final class CopyTableView: NSTableView {
    var onCopy: (() -> Void)?

    @objc func copy(_ sender: Any?) {
        onCopy?()
    }
}
