import AppKit

extension Notification.Name {
    /// Posted after the theme in use changes or is edited.
    static let rustyfinderThemeChanged = Notification.Name("RustyFinderThemeChanged")
}

/// The theme in use, as colours. A nil colour means the system's own, so
/// the Default theme leaves every view as macOS draws it (appearance and
/// accent colour included). Views read these when they draw and listen
/// for `.rustyfinderThemeChanged`.
@MainActor
final class ThemeCenter {
    static let shared = ThemeCenter()

    private(set) var theme: ThemeRecord
    private(set) var base: NSColor?
    private(set) var background: NSColor?
    private(set) var alternateBackground: NSColor?
    private(set) var text: NSColor?
    private(set) var secondaryText: NSColor?
    private(set) var selection: NSColor?
    private(set) var inactiveSelection: NSColor?
    private(set) var sidebarBackground: NSColor?
    private(set) var sidebarText: NSColor?
    private(set) var sidebarSelection: NSColor?

    private init() {
        theme = AppEnvironment.shared.core.currentTheme()
        load()
    }

    static func color(_ hex: String) -> NSColor? {
        let digits = hex.trimmingCharacters(in: CharacterSet(charactersIn: "#"))
        guard digits.count == 6, let value = UInt32(digits, radix: 16) else { return nil }
        return NSColor(
            srgbRed: CGFloat((value >> 16) & 0xFF) / 255, green: CGFloat((value >> 8) & 0xFF) / 255,
            blue: CGFloat(value & 0xFF) / 255, alpha: 1)
    }

    static func hex(_ color: NSColor) -> String {
        guard let rgb = color.usingColorSpace(.sRGB) else { return "" }
        let r = Int((rgb.redComponent * 255).rounded()), g = Int((rgb.greenComponent * 255).rounded()), b = Int((rgb.blueComponent * 255).rounded())
        return String(format: "#%02X%02X%02X", r, g, b)
    }

    private func load() {
        base = Self.color(theme.base)
        background = Self.color(theme.background)
        alternateBackground = Self.color(theme.alternateBackground)
        text = Self.color(theme.text)
        secondaryText = Self.color(theme.secondaryText)
        selection = Self.color(theme.selection)
        inactiveSelection = Self.color(theme.inactiveSelection)
        sidebarBackground = Self.color(theme.sidebarBackground)
        sidebarText = Self.color(theme.sidebarText)
        sidebarSelection = Self.color(theme.sidebarSelection)
    }

    /// The appearance windows get: forced light or dark, or none (follow
    /// macOS).
    var appearance: NSAppearance? {
        switch theme.appearance {
        case .system: return nil
        case .light: return NSAppearance(named: .aqua)
        case .dark: return NSAppearance(named: .darkAqua)
        }
    }

    /// Re-read the theme in use (after it was chosen, edited or deleted)
    /// and apply it to every window, inactive tabs included.
    func reload() {
        theme = AppEnvironment.shared.core.currentTheme()
        load()
        apply()
    }

    func apply() {
        NSApp.appearance = appearance
        for window in NSApp.windows {
            window.contentView?.needsDisplay = true
            Self.restyle(window.contentView)
        }
        NotificationCenter.default.post(name: .rustyfinderThemeChanged, object: nil)
    }

    /// Push the colours into list and icon views below `view`.
    private static func restyle(_ view: NSView?) {
        guard let view else { return }
        if let table = view as? FileTableView { table.applyTheme() }
        if let grid = view as? NSCollectionView { grid.backgroundColors = [shared.background ?? .clear] }
        view.subviews.forEach(restyle)
    }

    var debugSummary: String {
        let mode = NSApp.appearance?.name.rawValue ?? "system"
        return "\(theme.id) appearance=\(mode) background=\(theme.background.isEmpty ? "-" : theme.background) inactive=\(theme.inactiveSelection.isEmpty ? "-" : theme.inactiveSelection)"
    }
}

/// Row background drawn with the theme's selection colours: the focused
/// pane's selection, or the inactive-selection colour elsewhere.
final class ThemedRowView: NSTableRowView {
    var isSidebar = false

    override func drawSelection(in dirtyRect: NSRect) {
        let center = ThemeCenter.shared
        let color = isSidebar ? center.sidebarSelection : (isEmphasized ? center.selection : center.inactiveSelection)
        guard let color, selectionHighlightStyle != .none else {
            return super.drawSelection(in: dirtyRect)
        }
        color.setFill()
        let rect = isSidebar ? bounds.insetBy(dx: 8, dy: 1) : bounds
        NSBezierPath(roundedRect: rect, xRadius: isSidebar ? 5 : 0, yRadius: isSidebar ? 5 : 0).fill()
    }

    /// The colour the selection is drawn with, for scripted checks.
    var debugSelectionColor: String {
        let center = ThemeCenter.shared
        let color = isEmphasized ? center.selection : center.inactiveSelection
        return color.map(ThemeCenter.hex) ?? "system"
    }
}

extension FileTableView {
    /// Lists draw their rows with the theme's colours.
    override func makeView(withIdentifier identifier: NSUserInterfaceItemIdentifier, owner: Any?) -> NSView? {
        if identifier.rawValue == "NSTableViewRowViewKey" {
            return super.makeView(withIdentifier: identifier, owner: owner) ?? ThemedRowView()
        }
        return super.makeView(withIdentifier: identifier, owner: owner)
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        applyTheme()
    }

    override func didAdd(_ rowView: NSTableRowView, forRow row: Int) {
        super.didAdd(rowView, forRow: row)
        Self.recolor(rowView)
    }

    override func reloadData(forRowIndexes rowIndexes: IndexSet, columnIndexes: IndexSet) {
        super.reloadData(forRowIndexes: rowIndexes, columnIndexes: columnIndexes)
        recolorVisibleRows()
    }

    func applyTheme() {
        let center = ThemeCenter.shared
        backgroundColor = center.background ?? .controlBackgroundColor
        enclosingScrollView?.backgroundColor = center.background ?? .controlBackgroundColor
        recolorVisibleRows()
        needsDisplay = true
    }

    /// Alternating rows in the theme's colours, when it has them.
    override func drawBackground(inClipRect clipRect: NSRect) {
        let center = ThemeCenter.shared
        guard let background = center.background else { return super.drawBackground(inClipRect: clipRect) }
        background.setFill()
        clipRect.fill()
        guard usesAlternatingRowBackgroundColors, let alternate = center.alternateBackground, rowHeight > 0 else { return }
        alternate.setFill()
        let height = rowHeight + intercellSpacing.height
        var index = Int(clipRect.minY / height)
        while CGFloat(index) * height < clipRect.maxY {
            if index % 2 == 1 { NSRect(x: clipRect.minX, y: CGFloat(index) * height, width: clipRect.width, height: height).fill() }
            index += 1
        }
    }

    private func recolorVisibleRows() {
        guard ThemeCenter.shared.text != nil || ThemeCenter.shared.secondaryText != nil else { return }
        enumerateAvailableRowViews { rowView, _ in Self.recolor(rowView) }
    }

    /// Text the list drew in the system's label colours takes the theme's.
    private static func recolor(_ rowView: NSTableRowView) {
        let center = ThemeCenter.shared
        guard center.text != nil || center.secondaryText != nil else { return }
        for case let cell as NSTableCellView in rowView.subviews {
            guard let field = cell.textField else { continue }
            if field.textColor == .labelColor, let text = center.text {
                field.textColor = text
            } else if field.textColor == .secondaryLabelColor, let secondary = center.secondaryText {
                field.textColor = secondary
            }
        }
    }
}
