import AppKit

@MainActor
protocol TabBarDelegate: AnyObject {
    func tabBar(_ bar: TabBarView, select tab: UInt64)
    func tabBar(_ bar: TabBarView, close tab: UInt64)
    func tabBarNewTab(_ bar: TabBarView)
    func tabBar(_ bar: TabBarView, menuFor tab: UInt64) -> NSMenu?
    /// A tab was dragged to `index` of this bar (from this pane or the other).
    func tabBar(_ bar: TabBarView, move tab: UInt64, to index: Int)
    /// Files were dropped on a tab: they go into its location.
    func tabBar(_ bar: TabBarView, drop paths: [String], on tab: UInt64, modifiers: NSEvent.ModifierFlags) -> Bool
    /// Folders were dropped on "+": one new tab each (PT-08).
    func tabBar(_ bar: TabBarView, openTabsFor folders: [String])
    /// The window a dragged tab must come from (tabs only move within it).
    var tabDragWindowId: UInt64 { get }
}

/// One pane's row of tabs, with a "+" button at the end. Tabs can be
/// dragged to reorder them or to the other pane's bar; files dragged
/// over a tab open it (spring-loading) and can be dropped on it.
final class TabBarView: NSView {
    weak var delegate: TabBarDelegate?
    static let height: CGFloat = 28
    static let tabType = NSPasteboard.PasteboardType("dev.rustyfinder.tab")

    private let stack = NSStackView()
    private let addButton = PlusButton()
    private var buttons: [TabButton] = []
    /// Where a dragged tab would land, drawn as a thin line.
    private var insertionIndex: Int?

    override init(frame: NSRect) {
        super.init(frame: frame)
        stack.orientation = .horizontal
        stack.spacing = 4
        stack.distribution = .fillEqually
        stack.translatesAutoresizingMaskIntoConstraints = false

        addButton.image = NSImage(systemSymbolName: "plus", accessibilityDescription: "New Tab")
        addButton.isBordered = false
        addButton.toolTip = "New Tab (drop folders to open each in a tab)"
        addButton.target = self
        addButton.action = #selector(addTab(_:))
        addButton.translatesAutoresizingMaskIntoConstraints = false
        addButton.onDropFolders = { [weak self] folders in
            guard let self else { return }
            self.delegate?.tabBar(self, openTabsFor: folders)
        }

        addSubview(stack)
        addSubview(addButton)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 6),
            stack.topAnchor.constraint(equalTo: topAnchor, constant: 3),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -3),
            stack.trailingAnchor.constraint(equalTo: addButton.leadingAnchor, constant: -4),
            addButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -6),
            addButton.centerYAnchor.constraint(equalTo: centerYAnchor),
            addButton.widthAnchor.constraint(equalToConstant: 22),
        ])
        setAccessibilityRole(.tabGroup)
        registerForDraggedTypes([Self.tabType])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    /// A title that replaces the folder name (a tab showing search results).
    var titleOverride: ((UInt64) -> String?)?
    /// An icon that replaces the folder icon (search, tag, favorite).
    var iconOverride: ((UInt64) -> (image: NSImage, key: String)?)?

    func update(tabs: [TabRecord], selected: Int) {
        // Rebuild only when the set of tabs changed; otherwise update in place
        // so hover state and layout do not flicker.
        if buttons.map(\.tabId) != tabs.map(\.id) {
            buttons.forEach { $0.removeFromSuperview() }
            buttons = tabs.map { tab in
                let button = TabButton(tabId: tab.id)
                button.onSelect = { [weak self] in self.map { $0.delegate?.tabBar($0, select: tab.id) } }
                button.onClose = { [weak self] in self.map { $0.delegate?.tabBar($0, close: tab.id) } }
                button.menuProvider = { [weak self] in self.flatMap { $0.delegate?.tabBar($0, menuFor: tab.id) } }
                button.onDropFiles = { [weak self] paths, modifiers in
                    guard let self, let delegate = self.delegate else { return false }
                    return delegate.tabBar(self, drop: paths, on: tab.id, modifiers: modifiers)
                }
                button.windowId = { [weak self] in self?.delegate?.tabDragWindowId ?? 0 }
                stack.addArrangedSubview(button)
                return button
            }
        }
        for (i, tab) in tabs.enumerated() {
            // A tab on a server is named after the folder there, or after
            // the server itself at its top.
            let remoteName = (tab.path as NSString).lastPathComponent
            let title: String
            if let override = titleOverride?(tab.id) {
                title = override
            } else if !tab.pending.isEmpty {
                title = remoteName.isEmpty ? tab.pending : remoteName
            } else if tab.conn == 0 {
                title = tab.path.folderDisplayName
            } else {
                title = remoteName.isEmpty ? tab.label : remoteName
            }
            buttons[i].configure(
                title: title, path: tab.path, pinned: tab.pinned, selected: i == selected,
                remoteLabel: tab.conn == 0 && tab.pending.isEmpty ? nil : tab.label, icon: iconOverride?(tab.id))
        }
    }

    /// Titles of the tabs as shown, with their icon's name (scripted checks).
    var debugIcons: String { buttons.map(\.debugIcon).joined(separator: "|") }

    @objc private func addTab(_ sender: Any?) {
        delegate?.tabBarNewTab(self)
    }

    /// Drop a dragged tab at the index the pointer is over.
    func dropIndex(at point: NSPoint) -> Int {
        let local = stack.convert(point, from: self)
        for (i, button) in buttons.enumerated() where local.x < button.frame.midX { return i }
        return buttons.count
    }

    private func draggedTab(_ info: NSDraggingInfo) -> UInt64? {
        guard let text = info.draggingPasteboard.string(forType: Self.tabType) else { return nil }
        let parts = text.split(separator: ":").compactMap { UInt64($0) }
        guard parts.count == 2, parts[0] == delegate?.tabDragWindowId else { return nil }
        return parts[1]
    }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { draggingUpdated(sender) }

    override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation {
        guard draggedTab(sender) != nil else { return [] }
        insertionIndex = dropIndex(at: convert(sender.draggingLocation, from: nil))
        needsDisplay = true
        return .move
    }

    override func draggingExited(_ sender: NSDraggingInfo?) {
        insertionIndex = nil
        needsDisplay = true
    }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        defer {
            insertionIndex = nil
            needsDisplay = true
        }
        guard let tab = draggedTab(sender) else { return false }
        delegate?.tabBar(self, move: tab, to: dropIndex(at: convert(sender.draggingLocation, from: nil)))
        return true
    }

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard let index = insertionIndex else { return }
        let x: CGFloat
        if index < buttons.count {
            x = stack.convert(buttons[index].frame, to: self).minX - 2
        } else {
            x = buttons.last.map { stack.convert($0.frame, to: self).maxX + 2 } ?? 6
        }
        NSColor.controlAccentColor.setFill()
        NSRect(x: x - 1, y: 4, width: 2, height: bounds.height - 8).fill()
    }
}

/// The "+" button: a click opens a tab; folders dropped on it open one
/// tab each.
final class PlusButton: NSButton {
    var onDropFolders: (([String]) -> Void)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        registerForDraggedTypes([.fileURL])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    private func folders(_ info: NSDraggingInfo) -> [String] {
        DropRules.paths(from: info).filter { path in
            var isDir: ObjCBool = false
            return FileManager.default.fileExists(atPath: path, isDirectory: &isDir) && isDir.boolValue
        }
    }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        folders(sender).isEmpty ? [] : .link
    }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        let folders = folders(sender)
        guard !folders.isEmpty else { return false }
        onDropFolders?(folders)
        return true
    }
}

/// A single tab. The close button sits on the left and replaces the icon
/// while the pointer is over the tab; pinned tabs show a pin and no close.
final class TabButton: NSView, NSDraggingSource {
    let tabId: UInt64
    var onSelect: (() -> Void)?
    var onClose: (() -> Void)?
    var menuProvider: (() -> NSMenu?)?
    var onDropFiles: (([String], NSEvent.ModifierFlags) -> Bool)?
    var windowId: (() -> UInt64)?

    private let icon = NSImageView()
    private let closeButton = NSButton()
    private let label = NSTextField(labelWithString: "")
    private var isSelected = false
    private var isPinned = false
    private var isHovered = false
    private var isDropTarget = false
    private var tracking: NSTrackingArea?
    private var mouseDownPoint: NSPoint?
    private var springTimer: Timer?
    private(set) var debugIcon = ""

    /// How long files must hover over a tab before it opens.
    static let springDelay: TimeInterval = 0.6

    init(tabId: UInt64) {
        self.tabId = tabId
        super.init(frame: .zero)
        wantsLayer = true
        layer?.cornerRadius = 6

        icon.imageScaling = .scaleProportionallyUpOrDown
        closeButton.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Close Tab")
        closeButton.symbolConfiguration = .init(pointSize: 9, weight: .bold)
        closeButton.isBordered = false
        closeButton.target = self
        closeButton.action = #selector(close(_:))
        closeButton.isHidden = true
        label.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        label.lineBreakMode = .byTruncatingMiddle
        label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        for v in [icon, closeButton, label] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        NSLayoutConstraint.activate([
            icon.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 7),
            icon.centerYAnchor.constraint(equalTo: centerYAnchor),
            icon.widthAnchor.constraint(equalToConstant: 14),
            icon.heightAnchor.constraint(equalToConstant: 14),
            closeButton.centerXAnchor.constraint(equalTo: icon.centerXAnchor),
            closeButton.centerYAnchor.constraint(equalTo: icon.centerYAnchor),
            closeButton.widthAnchor.constraint(equalToConstant: 16),
            closeButton.heightAnchor.constraint(equalToConstant: 16),
            label.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 5),
            label.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -7),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            widthAnchor.constraint(greaterThanOrEqualToConstant: 44),
        ])
        setAccessibilityRole(.radioButton)
        registerForDraggedTypes([.fileURL])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func configure(title: String, path: String, pinned: Bool, selected: Bool, remoteLabel: String? = nil, icon custom: (image: NSImage, key: String)? = nil) {
        label.stringValue = title
        toolTip = remoteLabel.map { "\($0):\(path)" } ?? path.abbreviatingHome
        isPinned = pinned
        isSelected = selected
        // Local folders always show a folder icon, never a document icon;
        // remote tabs show the protocol's icon, or their favorite's.
        if pinned {
            icon.image = NSImage(systemSymbolName: "pin.fill", accessibilityDescription: "Pinned")
            debugIcon = "pin"
        } else if let custom {
            icon.image = custom.image
            debugIcon = custom.key
        } else if remoteLabel != nil {
            icon.image = NSImage(systemSymbolName: "network", accessibilityDescription: "Server")
            debugIcon = "network"
        } else {
            icon.image = RowPresenter.shared.folderIcon(atPath: path)
            debugIcon = "folder"
        }
        icon.contentTintColor = pinned || (remoteLabel != nil && custom == nil) ? .secondaryLabelColor : nil
        label.textColor = selected ? .labelColor : .secondaryLabelColor
        setAccessibilityLabel(title)
        setAccessibilityValue(selected)
        refresh()
    }

    private func refresh() {
        let showClose = isHovered && !isPinned
        closeButton.isHidden = !showClose
        icon.isHidden = showClose
        let fill: NSColor = isDropTarget
            ? .controlAccentColor.withAlphaComponent(0.3)
            : isSelected
            ? .labelColor.withAlphaComponent(0.12)
            : (isHovered ? .labelColor.withAlphaComponent(0.06) : .clear)
        layer?.backgroundColor = fill.cgColor
    }

    override func updateLayer() {
        refresh()
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeInKeyWindow], owner: self)
        addTrackingArea(area)
        tracking = area
    }

    override func mouseEntered(with event: NSEvent) {
        isHovered = true
        refresh()
    }

    override func mouseExited(with event: NSEvent) {
        isHovered = false
        refresh()
    }

    override func mouseDown(with event: NSEvent) {
        mouseDownPoint = event.locationInWindow
        onSelect?()
    }

    /// Dragging a tab starts after a few points of movement.
    override func mouseDragged(with event: NSEvent) {
        guard let start = mouseDownPoint, hypot(event.locationInWindow.x - start.x, event.locationInWindow.y - start.y) > 4 else { return }
        mouseDownPoint = nil
        let item = NSPasteboardItem()
        item.setString("\(windowId?() ?? 0):\(tabId)", forType: TabBarView.tabType)
        let dragging = NSDraggingItem(pasteboardWriter: item)
        let image = NSImage(size: bounds.size, flipped: false) { [weak self] _ in
            guard let self, let rep = self.bitmapImageRepForCachingDisplay(in: self.bounds) else { return false }
            self.cacheDisplay(in: self.bounds, to: rep)
            rep.draw(in: self.bounds)
            return true
        }
        dragging.setDraggingFrame(bounds, contents: image)
        beginDraggingSession(with: [dragging], event: event, source: self)
    }

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        context == .withinApplication ? .move : []
    }

    /// Middle-click closes the tab.
    override func otherMouseDown(with event: NSEvent) {
        if event.buttonNumber == 2 { onClose?() } else { super.otherMouseDown(with: event) }
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        menuProvider?()
    }

    override func accessibilityPerformPress() -> Bool {
        onSelect?()
        return true
    }

    @objc private func close(_ sender: Any?) {
        onClose?()
    }

    // MARK: Files dragged over the tab

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        guard !DropRules.paths(from: sender).isEmpty else { return [] }
        isDropTarget = true
        refresh()
        // Spring-loading: hovering opens the tab so the drop can land in
        // its list instead.
        springTimer?.invalidate()
        springTimer = Timer.scheduledTimer(withTimeInterval: Self.springDelay, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.springLoad() }
        }
        return NSEvent.modifierFlags.contains(.command) ? .move : .copy
    }

    override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation {
        guard !DropRules.paths(from: sender).isEmpty else { return [] }
        return NSEvent.modifierFlags.contains(.command) ? .move : .copy
    }

    override func draggingExited(_ sender: NSDraggingInfo?) {
        springTimer?.invalidate()
        isDropTarget = false
        refresh()
    }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        springTimer?.invalidate()
        isDropTarget = false
        refresh()
        return onDropFiles?(DropRules.paths(from: sender), NSEvent.modifierFlags) ?? false
    }

    /// Open this tab because files hover over it.
    func springLoad() {
        springTimer?.invalidate()
        onSelect?()
    }
}
