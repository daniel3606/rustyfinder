import AppKit
import UniformTypeIdentifiers

/// How lists and grids draw their items. These apply everywhere (only the
/// layout, sort and grouping are per folder) and live with the app's other
/// small choices.
enum ViewDisplay {
    static var boldFolders: Bool { Prefs.bool("BoldFolderNames") }
    static var listIconPreviews: Bool { Prefs.bool("ListIconPreviews") }
    /// Thumbnails for files on servers (costs a download each).
    static var remoteThumbnails: Bool { Prefs.bool("RemoteThumbnails") }

    /// Files on a server get a thumbnail only when the option is on, and
    /// only small pictures, PDFs and movies: anything else would mean
    /// downloading for nothing.
    static let remoteThumbnailLimit: UInt64 = 2_000_000
    static func remoteThumbnailAllowed(_ row: Row) -> Bool {
        guard remoteThumbnails, !row.isDir, row.size <= remoteThumbnailLimit,
              let type = UTType(filenameExtension: row.extension) else { return false }
        return type.conforms(to: .image) || type.conforms(to: .pdf) || type.conforms(to: .movie)
    }
    /// "" is the system font.
    static var fontFamily: String { Prefs.strings("FontFamily")?.first ?? "" }
    /// Name column takes up any change in the window's width.
    static var resizeNameColumn: Bool { !Prefs.bool("NameColumnFixed") }
    /// Fit every column to its content whenever the content changes.
    static var resizeColumnsToFit: Bool { Prefs.bool("ResizeColumnsToFit") }
    static var iconItemInfo: Bool { Prefs.bool("IconItemInfo") }
    static var iconPreviews: Bool { !Prefs.bool("IconPreviewsOff") }
    static var gridSpacing: CGFloat { CGFloat(Double(Prefs.strings("GridSpacing")?.first ?? "") ?? 8) }

    static func set(_ key: String, _ on: Bool) {
        Prefs.set(key, on)
        changed()
    }

    static func set(_ key: String, text: String) {
        Prefs.setStrings(key, [text])
        changed()
    }

    private static func changed() {
        NotificationCenter.default.post(name: .rustyfinderSettingsChanged, object: nil)
    }

    /// The list font at `size`, in the chosen family; bold for folders when
    /// that option is on.
    static func font(size: CGFloat, bold: Bool = false, monospacedDigits: Bool = false) -> NSFont {
        let weight: NSFont.Weight = bold ? .semibold : .regular
        if !fontFamily.isEmpty, let custom = NSFont(name: fontFamily, size: size) {
            return bold ? NSFontManager.shared.convert(custom, toHaveTrait: .boldFontMask) : custom
        }
        return monospacedDigits ? .monospacedDigitSystemFont(ofSize: size, weight: weight) : .systemFont(ofSize: size, weight: weight)
    }
}

extension Notification.Name {
    /// A tab's folder or view changed (View Options follows the active tab).
    static let rustyfinderTabViewChanged = Notification.Name("RustyFinderTabViewChanged")
}

/// View Options (⌘J): a floating panel for the active tab's folder.
@MainActor
final class ViewOptionsPanel: NSPanel {
    static let shared = ViewOptionsPanel()

    private let titleLabel = NSTextField(labelWithString: "")
    private let scope = NSPopUpButton()
    private let layoutControl = NSSegmentedControl(
        labels: ["Icons", "List", "Columns"], trackingMode: .selectOne, target: nil, action: nil)
    private let groupPopup = NSPopUpButton()
    private let sortPopup = NSPopUpButton()
    private let descending = NSButton(checkboxWithTitle: "Descending", target: nil, action: nil)
    private let foldersFirst = NSButton(checkboxWithTitle: "Show folders on top", target: nil, action: nil)
    private let foldersByName = NSButton(checkboxWithTitle: "Always sort folders alphabetically", target: nil, action: nil)
    private let listBox = NSStackView()
    private let iconBox = NSStackView()
    private let textSize = NSPopUpButton()
    private let iconSize = NSSlider(value: 64, minValue: 16, maxValue: 512, target: nil, action: nil)
    private let iconSizeLabel = NSTextField(labelWithString: "")
    private let spacing = NSSlider(value: 8, minValue: 2, maxValue: 40, target: nil, action: nil)
    private let fontPopup = NSPopUpButton()
    private var toggles: [(NSButton, String, Bool)] = []  // button, pref key, inverted

    /// The list the panel acts on: the active one of the key browser window.
    private var list: FileListViewController? {
        let window = NSApp.windows.first { $0.isMainWindow && $0.windowController is MainWindowController }
            ?? NSApp.windows.first { $0.windowController is MainWindowController && $0.isVisible }
        return (window?.windowController as? MainWindowController)?.activeList
    }

    private init() {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 300, height: 520),
            styleMask: [.titled, .closable, .utilityWindow, .nonactivatingPanel], backing: .buffered, defer: true)
        title = "View Options"
        isFloatingPanel = true
        hidesOnDeactivate = true
        isReleasedWhenClosed = false
        becomesKeyOnlyIfNeeded = true
        build()
        NotificationCenter.default.addObserver(
            forName: .rustyfinderTabViewChanged, object: nil, queue: .main
        ) { [weak self] _ in MainActor.assumeIsolated { self?.refresh() } }
        NotificationCenter.default.addObserver(
            forName: NSWindow.didBecomeMainNotification, object: nil, queue: .main
        ) { [weak self] _ in MainActor.assumeIsolated { self?.refresh() } }
    }

    func toggle() {
        if isVisible {
            orderOut(nil)
        } else {
            refresh()
            if let main = NSApp.mainWindow, frame.origin == .zero {
                setFrameTopLeftPoint(NSPoint(x: main.frame.maxX - frame.width - 20, y: main.frame.maxY - 60))
            }
            orderFront(nil)
        }
    }

    // MARK: Building

    private func label(_ text: String) -> NSTextField {
        let l = NSTextField(labelWithString: text)
        l.font = .boldSystemFont(ofSize: NSFont.smallSystemFontSize)
        l.textColor = .secondaryLabelColor
        return l
    }

    private func row(_ title: String, _ control: NSView) -> NSStackView {
        let t = NSTextField(labelWithString: title)
        t.alignment = .right
        t.widthAnchor.constraint(equalToConstant: 80).isActive = true
        let r = NSStackView(views: [t, control])
        r.spacing = 8
        return r
    }

    private func toggle(_ title: String, key: String, inverted: Bool = false) -> NSButton {
        let b = NSButton(checkboxWithTitle: title, target: self, action: #selector(displayToggled(_:)))
        toggles.append((b, key, inverted))
        return b
    }

    private func build() {
        titleLabel.font = .boldSystemFont(ofSize: NSFont.systemFontSize)
        titleLabel.lineBreakMode = .byTruncatingMiddle
        scope.addItems(withTitles: ["This Folder", "This Folder and Subfolders"])
        scope.target = self
        scope.action = #selector(scopeChanged(_:))
        layoutControl.target = self
        layoutControl.action = #selector(layoutChanged(_:))
        groupPopup.addItems(withTitles: GroupBy.menuOrder.map(\.menuTitle))
        groupPopup.target = self
        groupPopup.action = #selector(groupChanged(_:))
        sortPopup.addItems(withTitles: SortColumn.menuOrder.map(\.menuTitle))
        sortPopup.target = self
        sortPopup.action = #selector(sortChanged(_:))
        for b in [descending, foldersFirst, foldersByName] {
            b.target = self
            b.action = #selector(sortOptionChanged(_:))
        }
        textSize.addItems(withTitles: [11, 12, 13, 14, 16, 18, 20].map { "\($0) pt" })
        textSize.target = self
        textSize.action = #selector(textSizeChanged(_:))
        fontPopup.addItem(withTitle: "System Font")
        fontPopup.menu?.addItem(.separator())
        fontPopup.addItems(withTitles: NSFontManager.shared.availableFontFamilies)
        fontPopup.target = self
        fontPopup.action = #selector(fontChanged(_:))
        fontPopup.toolTip = "Option-click the font to go back to the system font"
        iconSize.target = self
        iconSize.action = #selector(iconSizeChanged(_:))
        iconSize.isContinuous = true
        spacing.target = self
        spacing.action = #selector(spacingChanged(_:))

        listBox.orientation = .vertical
        listBox.alignment = .leading
        listBox.spacing = 6
        for v in [
            label("LIST"),
            row("Text size", textSize),
            row("Font", fontPopup),
            toggle("Bold folder names", key: "BoldFolderNames"),
            toggle("Show icon preview", key: "ListIconPreviews"),
            toggle("Also for files on servers", key: "RemoteThumbnails"),
            toggle("Use relative dates", key: "relativeDates"),
            toggle("Use alternating background", key: "alternatingRows"),
            toggle("Always resize name column", key: "NameColumnFixed", inverted: true),
            toggle("Resize columns to fit", key: "ResizeColumnsToFit"),
        ] as [NSView] { listBox.addArrangedSubview(v) }

        iconBox.orientation = .vertical
        iconBox.alignment = .leading
        iconBox.spacing = 6
        let sizeRow = NSStackView(views: [iconSize, iconSizeLabel])
        iconSize.widthAnchor.constraint(equalToConstant: 150).isActive = true
        for v in [
            label("ICONS"),
            row("Icon size", sizeRow),
            row("Grid spacing", spacing),
            toggle("Show item info", key: "IconItemInfo"),
            toggle("Show icon preview", key: "IconPreviewsOff", inverted: true),
            toggle("Also for files on servers ", key: "RemoteThumbnails"),
        ] as [NSView] { iconBox.addArrangedSubview(v) }

        let useDefaults = NSButton(title: "Use as Defaults", target: self, action: #selector(useAsDefaults(_:)))
        let restore = NSButton(title: "Restore Defaults", target: self, action: #selector(restoreDefaults(_:)))
        let buttons = NSStackView(views: [useDefaults, restore])

        let stack = NSStackView(views: [
            titleLabel,
            row("Apply to", scope),
            row("View", layoutControl),
            row("Group by", groupPopup),
            row("Sort by", sortPopup),
            descending, foldersFirst, foldersByName,
            NSBox.separator(), listBox, iconBox, NSBox.separator(), buttons,
        ])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 8
        stack.edgeInsets = NSEdgeInsets(top: 14, left: 16, bottom: 14, right: 16)
        contentView = stack
    }

    // MARK: Showing the active tab

    func refresh() {
        guard isVisible || CommandLine.arguments.contains("--keys"), let list, let view = list.currentView else { return }
        titleLabel.stringValue = list.content.path.map { ($0 as NSString).lastPathComponent } ?? "—"
        let scopeNow = list.session.viewScope(tab: list.tabId)
        scope.selectItem(at: scopeNow.subfolders ? 1 : 0)
        layoutControl.selectedSegment = [ViewLayout.icons, .list, .columns].firstIndex(of: view.layout) ?? 1
        groupPopup.selectItem(at: GroupBy.menuOrder.firstIndex(of: view.group) ?? 0)
        sortPopup.selectItem(at: SortColumn.menuOrder.firstIndex(of: view.sort) ?? 0)
        descending.state = view.ascending ? .off : .on
        foldersFirst.state = view.foldersFirst ? .on : .off
        foldersByName.state = view.foldersByName ? .on : .off
        foldersByName.isEnabled = view.foldersFirst
        let settings = AppEnvironment.shared.settings
        textSize.selectItem(withTitle: "\(settings.listTextSize) pt")
        iconSize.doubleValue = Double(settings.iconSize)
        iconSizeLabel.stringValue = "\(settings.iconSize) px"
        spacing.doubleValue = Double(ViewDisplay.gridSpacing)
        if ViewDisplay.fontFamily.isEmpty { fontPopup.selectItem(at: 0) } else { fontPopup.selectItem(withTitle: ViewDisplay.fontFamily) }
        for (button, key, inverted) in toggles {
            let on: Bool
            switch key {
            case "relativeDates": on = settings.relativeDates
            case "alternatingRows": on = settings.alternatingRows
            default: on = Prefs.bool(key) != inverted
            }
            button.state = on ? .on : .off
        }
        listBox.isHidden = view.layout == .icons
        iconBox.isHidden = view.layout != .icons
    }

    // MARK: Actions

    private func change(_ edit: (inout ViewOptions) -> Void) {
        guard let list, var view = list.currentView else { return }
        edit(&view)
        list.session.setView(tab: list.tabId, view: view)
        // "This Folder and Subfolders" covers the change too.
        if scope.indexOfSelectedItem == 1 { list.session.applyViewToSubfolders(tab: list.tabId) }
    }

    @objc private func scopeChanged(_ sender: Any?) {
        guard let list else { return }
        if scope.indexOfSelectedItem == 1 {
            list.session.applyViewToSubfolders(tab: list.tabId)
        } else if let view = list.currentView {
            // Back to this folder only: keep the look, drop the subtree flag.
            list.session.restoreDefaultView(tab: list.tabId)
            list.session.setView(tab: list.tabId, view: view)
        }
    }

    @objc private func layoutChanged(_ sender: Any?) {
        let layouts: [ViewLayout] = [.icons, .list, .columns]
        change { $0.layout = layouts[max(0, layoutControl.selectedSegment)] }
    }

    @objc private func groupChanged(_ sender: Any?) {
        change { $0.group = GroupBy.menuOrder[max(0, groupPopup.indexOfSelectedItem)] }
    }

    @objc private func sortChanged(_ sender: Any?) {
        change { $0.sort = SortColumn.menuOrder[max(0, sortPopup.indexOfSelectedItem)] }
    }

    @objc private func sortOptionChanged(_ sender: NSButton) {
        change {
            $0.ascending = descending.state == .off
            $0.foldersFirst = foldersFirst.state == .on
            $0.foldersByName = foldersByName.state == .on
        }
    }

    @objc private func textSizeChanged(_ sender: Any?) {
        guard let size = UInt32(textSize.titleOfSelectedItem?.split(separator: " ").first ?? "") else { return }
        AppEnvironment.shared.update { $0.listTextSize = size }
    }

    @objc private func iconSizeChanged(_ sender: Any?) {
        let size = UInt32(iconSize.doubleValue.rounded())
        iconSizeLabel.stringValue = "\(size) px"
        AppEnvironment.shared.update { $0.iconSize = size }
    }

    @objc private func spacingChanged(_ sender: Any?) {
        ViewDisplay.set("GridSpacing", text: String(Int(spacing.doubleValue)))
    }

    @objc private func fontChanged(_ sender: Any?) {
        // Option-click (or the first item) goes back to the system font.
        let system = fontPopup.indexOfSelectedItem == 0 || NSEvent.modifierFlags.contains(.option)
        ViewDisplay.set("FontFamily", text: system ? "" : (fontPopup.titleOfSelectedItem ?? ""))
        if system { fontPopup.selectItem(at: 0) }
    }

    @objc private func displayToggled(_ sender: NSButton) {
        guard let (_, key, inverted) = toggles.first(where: { $0.0 === sender }) else { return }
        let on = sender.state == .on
        switch key {
        case "relativeDates": AppEnvironment.shared.update { $0.relativeDates = on }
        case "alternatingRows": AppEnvironment.shared.update { $0.alternatingRows = on }
        default: ViewDisplay.set(key, on != inverted)
        }
    }

    @objc private func useAsDefaults(_ sender: Any?) {
        guard let list else { return }
        list.session.useViewAsDefaults(tab: list.tabId)
    }

    @objc private func restoreDefaults(_ sender: Any?) {
        list?.restoreDefaultView(nil)
    }

    /// For scripted checks: "scope=…", "layout=…", "toggle:<title>".
    func debugSet(_ what: String) {
        if what == "subfolders" { scope.selectItem(at: 1); scopeChanged(nil); return }
        if what == "thisfolder" { scope.selectItem(at: 0); scopeChanged(nil); return }
        if what == "defaults" { useAsDefaults(nil); return }
        if what.hasPrefix("toggle:") {
            let title = String(what.dropFirst(7))
            if let (button, _, _) = toggles.first(where: { $0.0.title == title }) {
                button.state = button.state == .on ? .off : .on
                displayToggled(button)
            }
        }
    }

    var debugState: String {
        "\(isVisible ? "shown" : "hidden") folder=\(titleLabel.stringValue) scope=\(scope.indexOfSelectedItem == 1 ? "subfolders" : "folder")"
    }
}

extension NSBox {
    static func separator() -> NSBox {
        let box = NSBox()
        box.boxType = .separator
        return box
    }
}

extension FileListViewController {
    var currentView: ViewOptions? { session.view(tab: tabId) }

    /// ⌘J.
    @objc func showViewOptions(_ sender: Any?) {
        ViewOptionsPanel.shared.toggle()
    }

    /// ⌥⌘R: drop this folder's own view settings.
    @objc func restoreDefaultView(_ sender: Any?) {
        session.restoreDefaultView(tab: tabId)
    }

    /// View › Save View Settings as Defaults.
    @objc func saveViewAsDefaults(_ sender: Any?) {
        session.useViewAsDefaults(tab: tabId)
    }
}

// MARK: - Display helpers used by the list and the grid

/// Second-line facts for Icon view, worked out off the main thread once
/// per item and version.
@MainActor
final class ItemInfoCache {
    static let shared = ItemInfoCache()
    private var cache: [String: String] = [:]
    private var pending: Set<String> = []

    func text(_ row: Row, path: String, ready: @escaping () -> Void) -> String? {
        let key = "\(path)|\(row.modifiedMs ?? 0)|\(row.size)"
        if let hit = cache[key] { return hit }
        // Files show their size at once; images and folders need a look.
        let quick = row.isDir ? nil : RowPresenter.shared.bytes(row.size)
        guard !pending.contains(key) else { return quick }
        pending.insert(key)
        let isDir = row.isDir
        DispatchQueue.global(qos: .utility).async {
            let text = Self.measure(path: path, isDir: isDir)
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self.pending.remove(key)
                    self.cache[key] = text ?? quick ?? ""
                    ready()
                }
            }
        }
        return quick
    }

    private nonisolated static func measure(path: String, isDir: Bool) -> String? {
        if isDir {
            guard let names = try? FileManager.default.contentsOfDirectory(atPath: path) else { return nil }
            let count = names.filter { !$0.hasPrefix(".") }.count
            return count == 1 ? "1 item" : "\(count) items"
        }
        // Pixel size for images, read from the header without decoding.
        guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
              let props = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
              let w = props[kCGImagePropertyPixelWidth] as? Int, let h = props[kCGImagePropertyPixelHeight] as? Int
        else { return nil }
        return "\(w) × \(h)"
    }
}

extension FileListViewController {
    func itemInfo(_ row: Row, path: String) -> String? {
        ItemInfoCache.shared.text(row, path: path) { [weak self] in
            guard let self, let index = self.content.snapshot?.indexOf(name: row.name) else { return }
            self.gridView.reloadItems(at: [self.content.gridPath(ofItem: Int(index))])
        }
    }

    /// The icon in a list row: a thumbnail when "Show icon preview" is on
    /// (filled in when it arrives), else the item's kind icon.
    func listIcon(_ row: Row, at index: Int, in cell: NSTableCellView) -> NSImage {
        let path = content.path(at: index)
        let generic = RowPresenter.shared.icon(row, path: isRemote ? nil : path)
        guard ViewDisplay.listIconPreviews, let path, !row.isDir,
              !isRemote || isArchive || ViewDisplay.remoteThumbnailAllowed(row) else { return generic }
        let side: CGFloat = 32
        if let thumbnail = ThumbnailCache.shared.cached(path, row: row, side: side) { return thumbnail }
        let scale = view.window?.backingScaleFactor ?? 2
        if isRemote {
            ThumbnailCache.shared.requestInArchive(InspectedItem(path: path, row: row, conn: conn), side: side, scale: scale) {
                [weak cell, weak self] image in
                guard let self, let cell, self.content.path(at: index) == path else { return }
                cell.imageView?.image = image
            }
            return generic
        }
        ThumbnailCache.shared.request(path, row: row, side: side, scale: scale) { [weak cell, weak self] image in
            // Only if the cell still shows this item.
            guard let self, let cell, let current = self.content.path(at: index), current == path else { return }
            cell.imageView?.image = image
        }
        return generic
    }

    /// "Resize columns to fit": every visible column to its content.
    func fitAllColumns() {
        for (i, column) in tableView.tableColumns.enumerated() where !column.isHidden {
            column.width = tableView(tableView, sizeToFitWidthOfColumn: i)
        }
    }
}
