import AppKit
import Security

/// App Deleter (app-deleter spec): when an app goes to the Trash, also
/// offer the files it left in the Library folders. The matching rules live
/// in the Rust core (`app_deleter`); this window shows what they found and
/// sends the checked items to the Trash as one undoable activity.
@MainActor
enum AppDeleter {
    /// Apps waiting their turn: only one window is shown at a time.
    private static var queue: [(path: String, window: UInt64)] = []

    /// True for an application bundle on this Mac.
    static func isApp(_ path: String) -> Bool {
        (path as NSString).pathExtension.lowercased() == "app"
            && NSWorkspace.shared.isFilePackage(atPath: path)
    }

    /// Handle `apps` one after another.
    static func start(_ apps: [String], window: UInt64) {
        queue += apps.map { ($0, window) }
        next()
    }

    /// Show the next app's window, if none is open.
    static func next() {
        guard AppDeleterWindowController.current == nil, !queue.isEmpty else { return }
        let (path, window) = queue.removeFirst()
        guard let identity = identity(of: path) else {
            // Not readable as an app: trash it like any other item.
            ActivityCenter.shared.submit(.trash(paths: [path]), window: window)
            next()
            return
        }
        AppDeleterWindowController.show(app: identity, window: window)
    }

    /// Bundle identifier, names and signing team of the app at `path`.
    static func identity(of path: String) -> AppIdentityRecord? {
        guard let bundle = Bundle(path: path), let id = bundle.bundleIdentifier, !id.isEmpty else { return nil }
        let info = bundle.infoDictionary ?? [:]
        let name = (info["CFBundleDisplayName"] as? String) ?? (info["CFBundleName"] as? String)
            ?? ((path as NSString).lastPathComponent as NSString).deletingPathExtension
        let executable = (info["CFBundleExecutable"] as? String) ?? name
        return AppIdentityRecord(bundleId: id, name: name, executable: executable, teamId: teamId(of: path), bundlePath: path)
    }

    /// The team identifier in the app's code signature, if it has one.
    private static func teamId(of path: String) -> String? {
        var code: SecStaticCode?
        guard SecStaticCodeCreateWithPath(URL(fileURLWithPath: path) as CFURL, [], &code) == errSecSuccess,
              let code else { return nil }
        var info: CFDictionary?
        guard SecCodeCopySigningInformation(code, SecCSFlags(rawValue: kSecCSSigningInformation), &info) == errSecSuccess,
              let dict = info as? [String: Any] else { return nil }
        return dict[kSecCodeInfoTeamIdentifier as String] as? String
    }

    /// The home whose Library is searched. Scripted checks use a fake one
    /// and leave the local domain out, so they never read the real Library.
    static var searchRoots: (home: String, local: String?) {
        if let fake = ProcessInfo.processInfo.environment["RUSTYFINDER_APP_DELETER_HOME"] { return (fake, nil) }
        return (NSHomeDirectory(), "/Library")
    }
}

final class AppDeleterWindowController: NSWindowController, NSWindowDelegate, NSTableViewDataSource, NSTableViewDelegate {
    private(set) static var current: AppDeleterWindowController?

    private struct Item {
        let path: String
        let confident: Bool
        let reason: String
        let size: UInt64
        let isDir: Bool
        let needsAdmin: Bool
        var checked: Bool
        /// The app itself: always first, always checked.
        let isApp: Bool
    }

    private let app: AppIdentityRecord
    private let windowId: UInt64
    private var items: [Item] = []
    private var scanning = true
    private let table = NSTableView()
    private let spinner = NSProgressIndicator()
    private let status = NSTextField(labelWithString: "Looking for related files…")
    private let total = NSTextField(labelWithString: "")
    private let trashButton = NSButton(title: "Move to Trash…", target: nil, action: nil)
    private(set) var confirmation: NSAlert?

    static func show(app: AppIdentityRecord, window: UInt64) {
        let controller = AppDeleterWindowController(app: app, window: window)
        current = controller
        controller.showWindow(nil)
        controller.window?.center()
        controller.window?.makeKeyAndOrderFront(nil)
        controller.scan()
    }

    private init(app: AppIdentityRecord, window windowId: UInt64) {
        self.app = app
        self.windowId = windowId
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 720, height: 460),
            styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.title = "App Deleter"
        window.isReleasedWhenClosed = false
        window.isRestorable = false
        window.minSize = NSSize(width: 560, height: 320)
        super.init(window: window)
        window.delegate = self
        items = [Item(path: app.bundlePath, confident: true, reason: "The application", size: 0, isDir: true,
                      needsAdmin: !FileManager.default.isWritableFile(atPath: (app.bundlePath as NSString).deletingLastPathComponent),
                      checked: true, isApp: true)]
        buildUI()
        refreshTotal()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func windowWillClose(_ notification: Notification) {
        if Self.current === self { Self.current = nil }
        // The next selected app, if several were trashed at once.
        DispatchQueue.main.async { AppDeleter.next() }
    }

    // MARK: Layout

    private func buildUI() {
        guard let content = window?.contentView else { return }
        let icon = NSImageView(image: NSWorkspace.shared.icon(forFile: app.bundlePath))
        icon.widthAnchor.constraint(equalToConstant: 48).isActive = true
        icon.heightAnchor.constraint(equalToConstant: 48).isActive = true
        let version = (Bundle(path: app.bundlePath)?.infoDictionary?["CFBundleShortVersionString"] as? String).map { " \($0)" } ?? ""
        let title = NSTextField(labelWithString: "\(app.name)\(version)")
        title.font = .systemFont(ofSize: 15, weight: .semibold)
        let subtitle = NSTextField(labelWithString: app.bundleId)
        subtitle.textColor = .secondaryLabelColor
        subtitle.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        let warning = NSTextField(wrappingLabelWithString:
            "Review the list before moving anything. Items named after the app’s identifier are checked. "
            + "Possible matches are unchecked: check one only if you are sure it belongs to this app.")
        warning.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        warning.textColor = .secondaryLabelColor
        let names = NSStackView(views: [title, subtitle, warning])
        names.orientation = .vertical
        names.alignment = .leading
        names.spacing = 2
        let header = NSStackView(views: [icon, names])
        header.alignment = .top
        header.spacing = 12

        for (id, title, width) in [("check", "", 24.0), ("name", "Name", 220.0), ("match", "Match", 110.0),
                                   ("where", "Where", 220.0), ("size", "Size", 80.0)] {
            let column = NSTableColumn(identifier: .init(id))
            column.title = title
            column.width = width
            if id == "size" { column.headerCell.alignment = .right }
            table.addTableColumn(column)
        }
        table.dataSource = self
        table.delegate = self
        table.rowHeight = 22
        table.usesAlternatingRowBackgroundColors = true
        table.style = .inset
        table.setAccessibilityLabel("Items to move to the Trash")
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder

        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isDisplayedWhenStopped = false
        spinner.startAnimation(nil)
        status.textColor = .secondaryLabelColor
        total.alignment = .right
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancel(_:)))
        cancel.keyEquivalent = "\u{1b}"
        trashButton.target = self
        trashButton.action = #selector(moveToTrash(_:))
        trashButton.hasDestructiveAction = true
        let footer = NSStackView(views: [spinner, status, NSView(), total, cancel, trashButton])
        footer.spacing = 8

        let stack = NSStackView(views: [header, scroll, footer])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 16, left: 16, bottom: 16, right: 16)
        stack.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: content.topAnchor),
            stack.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            scroll.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -32),
            footer.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -32),
            header.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -32),
            scroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 160),
        ])
    }

    // MARK: Finding items

    /// The core walks the Library folders on a worker thread.
    private func scan() {
        let app = self.app
        let roots = AppDeleter.searchRoots
        DispatchQueue.global(qos: .userInitiated).async {
            let found = findAppLeftovers(app: app, home: roots.home, localLibrary: roots.local)
            let appSize = Self.allocatedSize(app.bundlePath)
            DispatchQueue.main.async { [weak self] in self?.show(found, appSize: appSize) }
        }
    }

    private func show(_ found: [LeftoverRecord], appSize: UInt64) {
        scanning = false
        let first = items[0]
        items[0] = Item(path: first.path, confident: true, reason: first.reason, size: appSize, isDir: true,
                        needsAdmin: first.needsAdmin, checked: true, isApp: true)
        items += found.map {
            Item(path: $0.path, confident: $0.confident, reason: $0.reason, size: $0.size, isDir: $0.isDir,
                 needsAdmin: $0.needsAdmin, checked: $0.confident, isApp: false)
        }
        spinner.stopAnimation(nil)
        let possible = found.filter { !$0.confident }.count
        status.stringValue = found.isEmpty
            ? "No related files found."
            : "\(found.count) related \(found.count == 1 ? "item" : "items") found"
                + (possible > 0 ? ", \(possible) possible \(possible == 1 ? "match" : "matches") left unchecked." : ".")
        table.reloadData()
        refreshTotal()
    }

    nonisolated private static func allocatedSize(_ path: String) -> UInt64 {
        let url = URL(fileURLWithPath: path)
        let keys: [URLResourceKey] = [.totalFileAllocatedSizeKey, .isSymbolicLinkKey]
        var bytes: UInt64 = 0
        let walker = FileManager.default.enumerator(at: url, includingPropertiesForKeys: keys, options: [], errorHandler: nil)
        while let child = walker?.nextObject() as? URL {
            bytes += UInt64((try? child.resourceValues(forKeys: Set(keys)))?.totalFileAllocatedSize ?? 0)
        }
        return bytes
    }

    private func refreshTotal() {
        let checked = items.filter(\.checked)
        let bytes = checked.reduce(UInt64(0)) { $0 + $1.size }
        total.stringValue = "\(checked.count) of \(items.count) checked, \(RowPresenter.shared.bytes(bytes))"
        trashButton.isEnabled = !scanning && !checked.isEmpty
    }

    // MARK: Table

    func numberOfRows(in tableView: NSTableView) -> Int { items.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let id = tableColumn?.identifier.rawValue, let item = items[safe: row] else { return nil }
        if id == "check" {
            let box = NSButton(checkboxWithTitle: "", target: self, action: #selector(toggled(_:)))
            box.tag = row
            box.state = item.checked ? .on : .off
            // The app itself is what the user asked to trash.
            box.isEnabled = !item.isApp
            box.setAccessibilityLabel("Move \((item.path as NSString).lastPathComponent) to the Trash")
            return box
        }
        let text: String
        var color = NSColor.labelColor
        var image: NSImage?
        switch id {
        case "name":
            text = FileManager.default.displayName(atPath: item.path)
            image = NSWorkspace.shared.icon(forFile: item.path)
        case "match":
            text = item.isApp ? "Application" : (item.confident ? "Confident" : "Possible match")
            color = item.confident ? .labelColor : .systemOrange
        case "where":
            text = ((item.path as NSString).deletingLastPathComponent as NSString).abbreviatingWithTildeInPath
            color = .secondaryLabelColor
        default:
            text = RowPresenter.shared.bytes(item.size)
            color = .secondaryLabelColor
        }
        let cell = NSTableCellView()
        let field = NSTextField(labelWithString: text)
        field.textColor = color
        field.lineBreakMode = id == "where" ? .byTruncatingHead : .byTruncatingTail
        field.alignment = id == "size" ? .right : .left
        field.toolTip = id == "match" || id == "name"
            ? item.reason + (item.needsAdmin ? ". Needs an administrator password to move." : "")
            : item.path
        field.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(field)
        cell.textField = field
        var leading = field.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 2)
        if let image {
            let icon = NSImageView(image: image)
            icon.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(icon)
            cell.imageView = icon
            NSLayoutConstraint.activate([
                icon.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 2),
                icon.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
                icon.widthAnchor.constraint(equalToConstant: 16),
                icon.heightAnchor.constraint(equalToConstant: 16),
            ])
            leading = field.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 4)
        }
        if item.needsAdmin, id == "match" {
            field.stringValue += " · admin"
        }
        NSLayoutConstraint.activate([
            leading,
            field.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -2),
            field.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        return cell
    }

    @objc private func toggled(_ sender: NSButton) {
        guard items.indices.contains(sender.tag), !items[sender.tag].isApp else { return }
        items[sender.tag].checked = sender.state == .on
        refreshTotal()
    }

    // MARK: Buttons

    @objc private func cancel(_ sender: Any?) {
        window?.close()
    }

    /// Asks once more, then trashes the checked items as one activity, so
    /// one Undo puts them all back.
    @objc func moveToTrash(_ sender: Any?) {
        guard let window, !scanning else { return }
        let paths = items.filter(\.checked).map(\.path)
        guard !paths.isEmpty else { return }
        let alert = NSAlert()
        alert.alertStyle = .warning
        let others = paths.count - 1
        alert.messageText = others == 0
            ? "Move “\(app.name)” to the Trash?"
            : "Move “\(app.name)” and \(others) related \(others == 1 ? "item" : "items") to the Trash?"
        var info = "Undo (⌘Z) puts everything back where it was."
        if items.contains(where: { $0.checked && $0.needsAdmin }) {
            info += " Some items need an administrator password; you will be asked for it."
        }
        alert.informativeText = info
        alert.addButton(withTitle: "Move to Trash")
        let keep = alert.addButton(withTitle: "Cancel")
        keep.keyEquivalent = "\r"
        alert.buttons[0].keyEquivalent = ""
        confirmation = alert
        alert.beginSheetModal(for: window) { [weak self] response in
            guard let self else { return }
            self.confirmation = nil
            guard response == .alertFirstButtonReturn else { return }
            ActivityCenter.shared.submit(.trash(paths: paths), window: self.windowId)
            self.window?.close()
        }
    }

    // MARK: Scripted checks

    /// `uncheck=<name>`, `check=<name>`, `trash`, `cancel`.
    func debugSet(_ what: String) {
        let parts = what.split(separator: "=", maxSplits: 1).map(String.init)
        let name = parts.count > 1 ? parts[1] : ""
        switch parts[0] {
        case "check", "uncheck":
            if let i = items.firstIndex(where: { ($0.path as NSString).lastPathComponent == name }), !items[i].isApp {
                items[i].checked = parts[0] == "check"
                table.reloadData()
                refreshTotal()
            }
        case "trash": moveToTrash(nil)
        case "cancel": cancel(nil)
        default: break
        }
    }

    func debugDump() -> String {
        let rows = items.map { item in
            let kind = item.isApp ? "app" : (item.confident ? "confident" : "possible")
            return "\(item.checked ? "[x]" : "[ ]") \((item.path as NSString).lastPathComponent) \(kind)"
        }
        return "\(app.name) scanning=\(scanning) items=\(rows.joined(separator: "|")) total=\(total.stringValue)"
    }
}
