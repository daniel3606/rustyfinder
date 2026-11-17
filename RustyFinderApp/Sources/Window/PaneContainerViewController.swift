import AppKit

@MainActor
protocol PaneContainerDelegate: AnyObject {
    func pane(_ side: PaneSide, select tab: UInt64)
    func pane(_ side: PaneSide, close tab: UInt64)
    func paneNewTab(_ side: PaneSide)
    func pane(_ side: PaneSide, menuFor tab: UInt64) -> NSMenu?
    func pane(_ side: PaneSide, navigate path: String)
    func paneVolumesMenu(_ side: PaneSide) -> NSMenu
    func pane(_ side: PaneSide, pathMenuFor path: String) -> NSMenu?
    /// A tab dragged to `index` of this pane's bar.
    func pane(_ side: PaneSide, move tab: UInt64, to index: Int)
    /// Files dropped on a tab, or on a path-bar segment (`tab` nil, `path`
    /// set): copied or moved there.
    func pane(_ side: PaneSide, drop paths: [String], onTab tab: UInt64?, path: String?, modifiers: NSEvent.ModifierFlags) -> Bool
    /// Folders dropped on "+": one tab each.
    func pane(_ side: PaneSide, openTabsFor folders: [String])
    /// Id of the window, so tabs only move within it.
    var paneWindowId: UInt64 { get }
}

/// One pane of the window: tab bar, path bar, the current tab's list, and
/// the status line.
final class PaneContainerViewController: NSViewController, TabBarDelegate {
    let side: PaneSide
    weak var delegate: PaneContainerDelegate?

    private let tabBar = TabBarView()
    private var tabBarHeight: NSLayoutConstraint!
    private let volumeButton = NSButton()
    private let pathControl = DropPathControl()
    private var pathHeight: NSLayoutConstraint!
    private var pathTop: NSLayoutConstraint!
    private let pathSeparator = NSBox()
    private let pendingView = PendingTabView()
    private let holder = NSView()
    private let statusLabel = NSTextField(labelWithString: "")
    private let spaceLabel = NSTextField(labelWithString: "")
    private let searchProgress = SearchProgressView()
    private let activeLine = NSView()
    private(set) weak var current: FileListViewController?
    private var isActive = false

    init(side: PaneSide) {
        self.side = side
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    override func loadView() {
        let root = NSView()
        tabBar.delegate = self
        tabBar.translatesAutoresizingMaskIntoConstraints = false

        // A thin accent line marks the pane that has keyboard focus.
        activeLine.wantsLayer = true
        activeLine.translatesAutoresizingMaskIntoConstraints = false

        volumeButton.image = NSImage(systemSymbolName: "internaldrive", accessibilityDescription: "Volumes")
        volumeButton.isBordered = false
        volumeButton.toolTip = "Volumes"
        volumeButton.target = self
        volumeButton.action = #selector(showVolumes(_:))
        volumeButton.translatesAutoresizingMaskIntoConstraints = false

        pathControl.pathStyle = .standard
        pathControl.controlSize = .small
        pathControl.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        pathControl.target = self
        pathControl.action = #selector(pathClicked(_:))
        let pathMenu = NSMenu()
        pathMenu.delegate = self
        pathControl.menu = pathMenu
        pathControl.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        pathControl.translatesAutoresizingMaskIntoConstraints = false

        holder.translatesAutoresizingMaskIntoConstraints = false

        for label in [statusLabel, spaceLabel] {
            label.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
            label.textColor = .secondaryLabelColor
            label.lineBreakMode = .byTruncatingTail
            label.translatesAutoresizingMaskIntoConstraints = false
        }
        spaceLabel.alignment = .right
        statusLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        spaceLabel.setContentCompressionResistancePriority(.defaultLow - 1, for: .horizontal)

        let statusSeparator = NSBox()
        for box in [pathSeparator, statusSeparator] {
            box.boxType = .separator
            box.translatesAutoresizingMaskIntoConstraints = false
        }

        searchProgress.onStop = { [weak self] in self?.current?.stopSearch() }
        for v in [activeLine, tabBar, volumeButton, pathControl, pathSeparator, holder, statusSeparator, statusLabel, spaceLabel, searchProgress] {
            root.addSubview(v)
        }
        tabBarHeight = tabBar.heightAnchor.constraint(equalToConstant: 0)
        pathTop = pathControl.topAnchor.constraint(equalTo: tabBar.bottomAnchor, constant: 3)
        pathHeight = pathControl.heightAnchor.constraint(equalToConstant: 22)
        pendingView.translatesAutoresizingMaskIntoConstraints = false
        pendingView.isHidden = true
        root.addSubview(pendingView)
        pathControl.onDrop = { [weak self] index, paths, modifiers in
            guard let self, let path = self.segmentPath(index) else { return false }
            return self.delegate?.pane(self.side, drop: paths, onTab: nil, path: path, modifiers: modifiers) ?? false
        }
        pathControl.canDrop = { [weak self] index in self?.segmentPath(index) != nil }
        NSLayoutConstraint.activate([
            pendingView.topAnchor.constraint(equalTo: pathSeparator.bottomAnchor),
            pendingView.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            pendingView.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            pendingView.bottomAnchor.constraint(equalTo: statusSeparator.topAnchor),
            activeLine.topAnchor.constraint(equalTo: root.safeAreaLayoutGuide.topAnchor),
            activeLine.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            activeLine.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            activeLine.heightAnchor.constraint(equalToConstant: 2),

            tabBar.topAnchor.constraint(equalTo: activeLine.bottomAnchor),
            tabBar.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            tabBar.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            tabBarHeight,

            volumeButton.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 8),
            volumeButton.centerYAnchor.constraint(equalTo: pathControl.centerYAnchor),
            volumeButton.widthAnchor.constraint(equalToConstant: 20),
            pathTop,
            pathControl.leadingAnchor.constraint(equalTo: volumeButton.trailingAnchor, constant: 4),
            pathControl.trailingAnchor.constraint(lessThanOrEqualTo: root.trailingAnchor, constant: -8),
            pathHeight,

            pathSeparator.topAnchor.constraint(equalTo: pathControl.bottomAnchor, constant: 3),
            pathSeparator.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            pathSeparator.trailingAnchor.constraint(equalTo: root.trailingAnchor),

            holder.topAnchor.constraint(equalTo: pathSeparator.bottomAnchor),
            holder.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            holder.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            holder.bottomAnchor.constraint(equalTo: statusSeparator.topAnchor),

            statusSeparator.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            statusSeparator.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            statusSeparator.bottomAnchor.constraint(equalTo: statusLabel.topAnchor, constant: -4),

            statusLabel.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 10),
            statusLabel.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -5),
            searchProgress.leadingAnchor.constraint(equalTo: statusLabel.trailingAnchor, constant: 6),
            searchProgress.centerYAnchor.constraint(equalTo: statusLabel.centerYAnchor),
            spaceLabel.leadingAnchor.constraint(greaterThanOrEqualTo: searchProgress.trailingAnchor, constant: 8),
            spaceLabel.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -10),
            spaceLabel.firstBaselineAnchor.constraint(equalTo: statusLabel.firstBaselineAnchor),

            root.widthAnchor.constraint(greaterThanOrEqualToConstant: 220),
        ])
        root.setAccessibilityRole(.group)
        root.setAccessibilityLabel(side == .left ? "Left pane" : "Right pane")
        view = root
    }

    /// Show this pane's tabs and put `list` (the selected tab) in the content area.
    /// Title for a tab that is not showing its folder (search results).
    var tabTitleOverride: ((UInt64) -> String?)? {
        get { tabBar.titleOverride }
        set { tabBar.titleOverride = newValue }
    }

    /// Icon for a tab that is not a plain folder (search, tag, favorite).
    var tabIconOverride: ((UInt64) -> (image: NSImage, key: String)?)? {
        get { tabBar.iconOverride }
        set { tabBar.iconOverride = newValue }
    }

    /// View › Hide Path Bar: the path bar row goes away in this pane.
    func setPathBarHidden(_ hidden: Bool) {
        _ = view
        pathControl.isHidden = hidden
        volumeButton.isHidden = hidden
        pathSeparator.isHidden = hidden
        pathHeight.constant = hidden ? 0 : 22
        pathTop.constant = hidden ? 0 : 3
    }

    var isPathBarVisible: Bool { !pathControl.isHidden }

    /// A restored server tab waiting for its connection: a message (and
    /// Retry after a failure) over the list. Nil hides it.
    func showPending(_ message: String?, failed: Bool, retry: @escaping () -> Void) {
        _ = view
        pendingView.isHidden = message == nil
        pendingView.show(message ?? "", failed: failed, retry: retry)
    }

    var pendingText: String { pendingView.isHidden ? "-" : pendingView.text }
    var tabIcons: String { tabBar.debugIcons }

    /// Folder of path-bar segment `index`, local or on the server.
    private func segmentPath(_ index: Int) -> String? {
        if current?.isRemote == true { return remoteSegments[safe: index] }
        return pathControl.pathItems[safe: index]?.url?.path
    }

    /// Drop files on path-bar segment `index` (scripted checks).
    /// A negative index counts from the last segment (-1).
    func debugDropOnSegment(_ index: Int, paths: [String]) -> Bool {
        let count = current?.isRemote == true ? remoteSegments.count : pathControl.pathItems.count
        return pathControl.onDrop?(index < 0 ? count + index : index, paths, []) ?? false
    }

    func update(pane: PaneTabsRecord, list: FileListViewController, active: Bool) {
        _ = view
        let showBar = pane.tabs.count > 1 || AppEnvironment.shared.settings.alwaysShowTabBar
        tabBar.isHidden = !showBar
        tabBarHeight.constant = showBar ? TabBarView.height : 0
        tabBar.update(tabs: pane.tabs, selected: Int(pane.selected))

        if current !== list {
            current?.view.removeFromSuperview()
            current?.removeFromParent()
            addChild(list)
            list.view.translatesAutoresizingMaskIntoConstraints = false
            holder.addSubview(list.view)
            NSLayoutConstraint.activate([
                list.view.topAnchor.constraint(equalTo: holder.topAnchor),
                list.view.bottomAnchor.constraint(equalTo: holder.bottomAnchor),
                list.view.leadingAnchor.constraint(equalTo: holder.leadingAnchor),
                list.view.trailingAnchor.constraint(equalTo: holder.trailingAnchor),
            ])
            current = list
        }
        isActive = active
        activeLine.layer?.backgroundColor = (active ? (ThemeCenter.shared.base ?? NSColor.controlAccentColor) : NSColor.clear).cgColor
        pathControl.alphaValue = active ? 1 : 0.6
        refresh(path: pane.tabs[Int(pane.selected)].path)
    }

    /// Update the path bar and status line from the current tab.
    func refresh(path fallback: String? = nil) {
        guard let list = current else { return }
        let path = list.content.path ?? fallback
        if let path {
            if list.isRemote {
                showRemotePath(path, label: remoteLabel(for: list))
            } else {
                let url = URL(fileURLWithPath: path)
                if pathControl.url != url { pathControl.url = url }
            }
            if let space = AppEnvironment.shared.core.spaceOn(connection: list.conn, path: path) {
                spaceLabel.stringValue = "\(RowPresenter.shared.bytes(space.free)) available"
            } else {
                spaceLabel.stringValue = ""
            }
        }
        statusLabel.stringValue = list.statusText
        searchProgress.update(running: list.search?.running == true)
    }

    private func remoteLabel(for list: FileListViewController) -> String {
        AppEnvironment.shared.core.connections().first { $0.id == list.conn }?.label ?? "Server"
    }

    /// Path bar for a server: its name first, then each folder. Segments
    /// are plain items carrying the remote path they stand for.
    private func showRemotePath(_ path: String, label: String) {
        var items: [NSPathControlItem] = []
        for (index, component) in pathComponents(path: path).enumerated() {
            let item = NSPathControlItem()
            item.title = index == 0 ? label : component.label
            item.image = NSImage(systemSymbolName: index == 0 ? "network" : "folder", accessibilityDescription: nil)
            items.append(item)
        }
        remoteSegments = pathComponents(path: path).map(\.path)
        pathControl.pathItems = items
    }

    var statusText: String { statusLabel.stringValue }
    var debugSearchProgress: SearchProgressView { searchProgress }
    var isTabBarVisible: Bool { !tabBar.isHidden }

    /// Remote path of each path bar segment while a server is shown.
    private var remoteSegments: [String] = []

    @objc private func pathClicked(_ sender: NSPathControl) {
        guard let clicked = sender.clickedPathItem else { return }
        if current?.isRemote == true {
            // Remote segments have no URL; look the path up by position.
            guard let index = sender.pathItems.firstIndex(where: { $0 === clicked }),
                  let path = remoteSegments[safe: index]
            else { return }
            delegate?.pane(side, navigate: path)
        } else if let url = clicked.url {
            delegate?.pane(side, navigate: url.path)
        }
    }

    @objc private func showVolumes(_ sender: NSButton) {
        guard let menu = delegate?.paneVolumesMenu(side) else { return }
        menu.popUp(positioning: nil, at: NSPoint(x: 0, y: sender.bounds.height + 4), in: sender)
    }

    // MARK: TabBarDelegate

    func tabBar(_ bar: TabBarView, select tab: UInt64) { delegate?.pane(side, select: tab) }
    func tabBar(_ bar: TabBarView, close tab: UInt64) { delegate?.pane(side, close: tab) }
    func tabBarNewTab(_ bar: TabBarView) { delegate?.paneNewTab(side) }
    func tabBar(_ bar: TabBarView, menuFor tab: UInt64) -> NSMenu? { delegate?.pane(side, menuFor: tab) }
    func tabBar(_ bar: TabBarView, move tab: UInt64, to index: Int) { delegate?.pane(side, move: tab, to: index) }

    func tabBar(_ bar: TabBarView, drop paths: [String], on tab: UInt64, modifiers: NSEvent.ModifierFlags) -> Bool {
        delegate?.pane(side, drop: paths, onTab: tab, path: nil, modifiers: modifiers) ?? false
    }

    func tabBar(_ bar: TabBarView, openTabsFor folders: [String]) { delegate?.pane(side, openTabsFor: folders) }
    var tabDragWindowId: UInt64 { delegate?.paneWindowId ?? 0 }

    /// The tab bar, for scripted checks.
    var debugTabBar: TabBarView { tabBar }
}

/// Shown over the list of a restored server tab until it has reconnected:
/// what is happening, and Retry after a failure. The tab is never
/// silently replaced.
final class PendingTabView: NSView {
    private let label = NSTextField(wrappingLabelWithString: "")
    private let spinner = NSProgressIndicator()
    private let retryButton = NSButton(title: "Retry", target: nil, action: nil)
    private var retry: (() -> Void)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        label.alignment = .center
        label.textColor = .secondaryLabelColor
        spinner.style = .spinning
        spinner.controlSize = .small
        retryButton.target = self
        retryButton.action = #selector(retryPressed(_:))
        let stack = NSStackView(views: [spinner, label, retryButton])
        stack.orientation = .vertical
        stack.spacing = 10
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.centerXAnchor.constraint(equalTo: centerXAnchor),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
            label.widthAnchor.constraint(lessThanOrEqualToConstant: 320),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    override var isOpaque: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        (ThemeCenter.shared.background ?? NSColor.controlBackgroundColor).setFill()
        dirtyRect.fill()
    }

    func show(_ text: String, failed: Bool, retry: @escaping () -> Void) {
        label.stringValue = text
        self.retry = retry
        retryButton.isHidden = !failed
        spinner.isHidden = failed
        failed ? spinner.stopAnimation(nil) : spinner.startAnimation(nil)
        setAccessibilityLabel(text)
    }

    var text: String { label.stringValue + (retryButton.isHidden ? "" : " [Retry]") }

    @objc private func retryPressed(_ sender: Any?) { retry?() }
}

/// The path bar, accepting items dropped on a segment. The segment under
/// a drag expands as on hover, so a shortened name can be read.
final class DropPathControl: NSPathControl {
    var onDrop: ((Int, [String], NSEvent.ModifierFlags) -> Bool)?
    var canDrop: ((Int) -> Bool)?
    private var hovered: Int?

    override init(frame: NSRect) {
        super.init(frame: frame)
        registerForDraggedTypes([.fileURL])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    /// Index of the segment at a point in window coordinates.
    private func segment(at windowPoint: NSPoint) -> Int? {
        guard let cell = cell as? NSPathCell else { return nil }
        let point = convert(windowPoint, from: nil)
        guard let hit = cell.pathComponentCell(at: point, withFrame: bounds, in: self) else { return nil }
        return cell.pathComponentCells.firstIndex { $0 === hit }
    }

    private func expand(_ index: Int?, location: NSPoint) {
        guard index != hovered, let cell = cell as? NSPathCell else { return }
        let event = NSEvent.mouseEvent(
            with: .mouseMoved, location: location, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: window?.windowNumber ?? 0, context: nil, eventNumber: 0, clickCount: 0, pressure: 0)
        if hovered != nil, let event { cell.mouseExited(with: event, frame: bounds, in: self) }
        hovered = index
        if index != nil, let event { cell.mouseEntered(with: event, frame: bounds, in: self) }
    }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { draggingUpdated(sender) }

    override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation {
        let index = segment(at: sender.draggingLocation)
        expand(index, location: sender.draggingLocation)
        guard let index, canDrop?(index) == true, !DropRules.paths(from: sender).isEmpty else { return [] }
        return NSEvent.modifierFlags.contains(.command) ? .move : .copy
    }

    override func draggingExited(_ sender: NSDraggingInfo?) {
        expand(nil, location: sender?.draggingLocation ?? .zero)
    }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        defer { expand(nil, location: sender.draggingLocation) }
        guard let index = segment(at: sender.draggingLocation) else { return false }
        return onDrop?(index, DropRules.paths(from: sender), NSEvent.modifierFlags) ?? false
    }
}

extension PaneContainerViewController: NSMenuDelegate {
    /// Right-click on the path bar: Copy Path, Open in New Tab, Add to
    /// Favorites, for the clicked segment (or the current folder).
    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        let path = pathControl.clickedPathItem?.url?.path ?? current?.content.path
        guard let path, let custom = delegate?.pane(side, pathMenuFor: path) else { return }
        for item in custom.items {
            custom.removeItem(item)
            menu.addItem(item)
        }
    }
}
