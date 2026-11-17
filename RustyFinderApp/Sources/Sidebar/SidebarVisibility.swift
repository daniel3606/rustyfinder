import AppKit

extension Notification.Name {
    /// Posted when sidebar groups or items are hidden, shown or reordered.
    static let rustyfinderSidebarChanged = Notification.Name("RustyFinderSidebarChanged")
}

/// What the user hid from the sidebar, and the order of devices. Keys:
/// `group:devices|connections|shares|recents|tags`, `favgroup:<id>`,
/// `device:<path>`, `share:<path or server name>`.
@MainActor
enum SidebarVisibility {
    private static let hiddenKey = "sidebar.hidden"
    private static let orderKey = "sidebar.deviceOrder"

    static var hidden: Set<String> { Set(Prefs.strings(hiddenKey) ?? []) }

    static func isHidden(_ key: String) -> Bool { hidden.contains(key) }

    static func set(_ key: String, hidden on: Bool) {
        var all = hidden
        if on { all.insert(key) } else { all.remove(key) }
        Prefs.setStrings(hiddenKey, all.sorted())
        NotificationCenter.default.post(name: .rustyfinderSidebarChanged, object: nil)
    }

    static func setAll(_ keys: Set<String>) {
        Prefs.setStrings(hiddenKey, keys.sorted())
        NotificationCenter.default.post(name: .rustyfinderSidebarChanged, object: nil)
    }

    /// Volume paths in the order the user dragged them into.
    static var deviceOrder: [String] { Prefs.strings(orderKey) ?? [] }

    static func setDeviceOrder(_ paths: [String]) {
        Prefs.setStrings(orderKey, paths)
        NotificationCenter.default.post(name: .rustyfinderSidebarChanged, object: nil)
    }

    /// `items` sorted by the saved device order; unknown ones keep their
    /// place after the known ones.
    static func ordered<T>(_ items: [T], path: (T) -> String) -> [T] {
        let order = deviceOrder
        return items.enumerated().sorted { a, b in
            let ia = order.firstIndex(of: path(a.element)) ?? (order.count + a.offset)
            let ib = order.firstIndex(of: path(b.element)) ?? (order.count + b.offset)
            return ia < ib
        }.map(\.element)
    }
}

/// The Sidebar Editor: a checkbox for every group and every device,
/// share and favorite group.
final class SidebarEditorController: NSViewController {
    private(set) static var current: SidebarEditorController?
    private let entries: [(key: String, title: String, indent: Bool)]
    private var boxes: [NSButton] = []

    init(entries: [(key: String, title: String, indent: Bool)]) {
        self.entries = entries
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    override func loadView() {
        let title = NSTextField(labelWithString: "Show in Sidebar")
        title.font = .systemFont(ofSize: NSFont.systemFontSize, weight: .semibold)
        let stack = NSStackView(views: [title])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 5
        let hidden = SidebarVisibility.hidden
        for (index, entry) in entries.enumerated() {
            let box = NSButton(checkboxWithTitle: entry.title, target: self, action: #selector(toggled(_:)))
            box.state = hidden.contains(entry.key) ? .off : .on
            box.tag = index
            boxes.append(box)
            let row = NSStackView(views: [box])
            row.edgeInsets = NSEdgeInsets(top: 0, left: entry.indent ? 18 : 0, bottom: 0, right: 0)
            stack.addArrangedSubview(row)
        }
        let done = NSButton(title: "Done", target: self, action: #selector(done(_:)))
        done.keyEquivalent = "\r"
        stack.addArrangedSubview(done)
        stack.edgeInsets = NSEdgeInsets(top: 14, left: 16, bottom: 14, right: 16)
        view = stack
    }

    @objc private func toggled(_ sender: NSButton) {
        SidebarVisibility.set(entries[sender.tag].key, hidden: sender.state == .off)
    }

    @objc private func done(_ sender: Any?) {
        Self.current = nil
        guard let window = view.window else { return }
        window.sheetParent?.endSheet(window)
    }

    func begin(on window: NSWindow) {
        Self.current = self
        let sheet = NSWindow(contentViewController: self)
        sheet.styleMask = [.titled]
        window.beginSheet(sheet)
    }

    /// Click the box with this title (scripted checks).
    func debugToggle(_ title: String) {
        loadViewIfNeeded()
        boxes.first { $0.title == title }?.performClick(nil)
    }

    func debugDump() -> String {
        boxes.map { "\($0.state == .on ? "+" : "-")\($0.title)" }.joined(separator: ",")
    }
}
