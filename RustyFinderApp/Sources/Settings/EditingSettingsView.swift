import AppKit
import UniformTypeIdentifiers

/// Settings → Editing (settings-defaults §3): the applications Edit (⌘E)
/// uses, the extensions each one edits, and the one used for all others.
final class EditingSettingsView: NSView, NSTableViewDataSource, NSTableViewDelegate, NSTextFieldDelegate {
    private var core: RustyFinderApp { MainActor.assumeIsolated { AppEnvironment.shared.core } }
    private var editors: [EditorRecord] = []
    private let table = NSTableView()
    private let buttons = NSSegmentedControl()

    init() {
        super.init(frame: .zero)
        for (id, title, width) in [("app", "Editor", 200.0), ("extensions", "Extensions", 240.0), ("default", "Default", 60.0)] {
            let column = NSTableColumn(identifier: .init(id))
            column.title = title
            column.width = width
            table.addTableColumn(column)
        }
        table.dataSource = self
        table.delegate = self
        table.rowHeight = 24
        table.setAccessibilityLabel("Editors")
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        scroll.translatesAutoresizingMaskIntoConstraints = false

        buttons.segmentCount = 2
        buttons.setImage(NSImage(systemSymbolName: "plus", accessibilityDescription: "Add Editor"), forSegment: 0)
        buttons.setImage(NSImage(systemSymbolName: "minus", accessibilityDescription: "Remove Editor"), forSegment: 1)
        buttons.trackingMode = .momentary
        buttons.target = self
        buttons.action = #selector(buttonPressed(_:))
        buttons.translatesAutoresizingMaskIntoConstraints = false

        let note = NSTextField(wrappingLabelWithString:
            "Edit (⌘E) opens a file in the editor that lists its extension (comma-separated, without dots), "
            + "otherwise in the default editor, otherwise in the file’s usual application.")
        note.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        note.textColor = .secondaryLabelColor
        note.translatesAutoresizingMaskIntoConstraints = false

        addSubview(scroll)
        addSubview(buttons)
        addSubview(note)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: topAnchor),
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroll.widthAnchor.constraint(equalToConstant: 540),
            scroll.heightAnchor.constraint(equalToConstant: 260),
            buttons.topAnchor.constraint(equalTo: scroll.bottomAnchor, constant: 6),
            buttons.leadingAnchor.constraint(equalTo: leadingAnchor),
            note.topAnchor.constraint(equalTo: buttons.bottomAnchor, constant: 10),
            note.leadingAnchor.constraint(equalTo: leadingAnchor),
            note.widthAnchor.constraint(equalToConstant: 540),
            note.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
        reload()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func reload() {
        editors = core.editors()
        table.reloadData()
    }

    private func save() {
        editors = core.setEditors(editors: editors)
        table.reloadData()
    }

    // MARK: Adding and removing

    @objc private func buttonPressed(_ sender: NSSegmentedControl) {
        if sender.selectedSegment == 0 {
            chooseApp()
        } else {
            removeSelected()
        }
    }

    private func chooseApp() {
        let panel = NSOpenPanel()
        panel.directoryURL = URL(fileURLWithPath: "/Applications")
        panel.allowedContentTypes = [.application]
        panel.allowsMultipleSelection = true
        let done: (NSApplication.ModalResponse) -> Void = { [weak self] response in
            guard response == .OK else { return }
            panel.urls.forEach { self?.add(app: $0.path) }
        }
        if let window { panel.beginSheetModal(for: window, completionHandler: done) } else { done(panel.runModal()) }
    }

    func add(app: String, extensions: [String] = []) {
        guard !editors.contains(where: { $0.app == app }) else { return }
        editors.append(EditorRecord(app: app, extensions: extensions, isDefault: false))
        save()
    }

    /// Removal is permanent (no undo), as the spec says.
    func removeSelected() {
        let row = table.selectedRow
        guard editors.indices.contains(row) else { return }
        editors.remove(at: row)
        save()
    }

    // MARK: Table

    func numberOfRows(in tableView: NSTableView) -> Int { editors.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let id = tableColumn?.identifier.rawValue, let editor = editors[safe: row] else { return nil }
        switch id {
        case "default":
            let box = NSButton(checkboxWithTitle: "", target: self, action: #selector(defaultToggled(_:)))
            box.tag = row
            box.state = editor.isDefault ? .on : .off
            box.setAccessibilityLabel("Default editor")
            return box
        case "extensions":
            let field = NSTextField(string: editor.extensions.joined(separator: ", "))
            field.placeholderString = "txt, md"
            field.isBordered = false
            field.drawsBackground = false
            field.tag = row
            field.delegate = self
            field.setAccessibilityLabel("Extensions")
            return field
        default:
            let cell = NSTableCellView()
            let icon = NSImageView(image: NSWorkspace.shared.icon(forFile: editor.app))
            let name = NSTextField(labelWithString: FileManager.default.displayName(atPath: editor.app))
            name.lineBreakMode = .byTruncatingTail
            name.toolTip = editor.app
            for view in [icon, name] as [NSView] {
                view.translatesAutoresizingMaskIntoConstraints = false
                cell.addSubview(view)
            }
            NSLayoutConstraint.activate([
                icon.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 2),
                icon.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
                icon.widthAnchor.constraint(equalToConstant: 18),
                icon.heightAnchor.constraint(equalToConstant: 18),
                name.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 6),
                name.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -2),
                name.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            ])
            return cell
        }
    }

    @objc private func defaultToggled(_ sender: NSButton) {
        setDefault(row: sender.tag, sender.state == .on)
    }

    /// Only one editor is the default: marking one clears the others.
    func setDefault(row: Int, _ on: Bool) {
        guard editors.indices.contains(row) else { return }
        for i in editors.indices { editors[i].isDefault = on && i == row }
        save()
    }

    func controlTextDidEndEditing(_ notification: Notification) {
        guard let field = notification.object as? NSTextField else { return }
        setExtensions(row: field.tag, field.stringValue)
    }

    func setExtensions(row: Int, _ text: String) {
        guard editors.indices.contains(row) else { return }
        editors[row].extensions = parseEditorExtensions(text: text)
        save()
    }

    // MARK: Scripted checks

    /// `add=<app path>`, `ext=<row>=<text>`, `default=<row>`, `remove=<row>`.
    func debugSet(_ what: String) {
        let parts = what.split(separator: "=", maxSplits: 2).map(String.init)
        let row = parts.count > 1 ? Int(parts[1]) ?? -1 : -1
        switch parts[0] {
        case "add": if parts.count > 1 { add(app: parts[1]) }
        case "ext": setExtensions(row: row, parts.count > 2 ? parts[2] : "")
        case "default": setDefault(row: row, true)
        case "remove":
            table.selectRowIndexes([row], byExtendingSelection: false)
            removeSelected()
        default: break
        }
    }

    var debugSummary: String {
        editors.map { e in
            "\((e.app as NSString).lastPathComponent)[\(e.extensions.joined(separator: ","))]\(e.isDefault ? "*" : "")"
        }.joined(separator: "|")
    }
}
