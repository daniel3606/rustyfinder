import AppKit

/// Tool shortcuts share one key space with the menu commands. A tool shows
/// up in Settings → Shortcuts as the command `tool:<id>`, and every
/// assignment, from there or from the Tools window, is checked against
/// both the menus and the other tools.
@MainActor
enum ToolShortcuts {
    private static let prefix = "tool:"

    static func commandId(_ tool: UInt64) -> String { "\(prefix)\(tool)" }

    static func toolId(_ command: String) -> UInt64? {
        command.hasPrefix(prefix) ? UInt64(command.dropFirst(prefix.count)) : nil
    }

    /// Settings → Shortcuts rows for the saved tools.
    static var commands: [ShortcutCenter.Command] {
        ToolCenter.shared.tools.map { tool in
            ShortcutCenter.Command(
                id: commandId(tool.id), menu: "Tools", title: tool.name,
                item: NSMenuItem(title: tool.name, action: nil, keyEquivalent: ""))
        }
    }

    static func binding(of command: String) -> String {
        guard let id = toolId(command), let tool = ToolCenter.shared.tool(id: id) else { return "" }
        return shortcutNormalize(text: tool.shortcut) ?? tool.shortcut
    }

    static func title(of command: String) -> String? {
        guard let id = toolId(command) else { return nil }
        return "\(ToolCenter.shared.tool(id: id)?.name ?? "Tool") (Tools)"
    }

    /// The tool command that has `binding`, other than `except`.
    static func toolHolder(of binding: String, except: String) -> String? {
        guard let wanted = shortcutNormalize(text: binding), !wanted.isEmpty else { return nil }
        let skip = toolId(except) ?? 0
        return toolShortcutHolder(tools: ToolCenter.shared.tools, binding: wanted, except: skip).map(commandId)
    }

    /// Whoever has `binding` (a menu command or a tool), other than `except`.
    static func holder(of binding: String, except: String) -> String? {
        guard let wanted = shortcutNormalize(text: binding), !wanted.isEmpty else { return nil }
        let center = ShortcutCenter.shared
        if let menu = center.commands.first(where: { $0.id != except && center.binding(of: $0.id) == wanted }) {
            return menu.id
        }
        return toolHolder(of: wanted, except: except)
    }

    /// Give a saved tool a shortcut ("" removes it). If someone else has
    /// it, nothing changes and the holder is returned, unless `force`,
    /// which takes the shortcut away from them.
    @discardableResult
    static func assign(tool id: UInt64, _ binding: String, force: Bool) -> String? {
        let normalized = shortcutNormalize(text: binding) ?? binding
        if let other = holder(of: normalized, except: commandId(id)) {
            guard force else { return other }
            release(other)
        }
        setShortcut(normalized, ofTool: id)
        return nil
    }

    /// Take the shortcut away from a command or tool.
    static func release(_ command: String) {
        if let id = toolId(command) {
            setShortcut("", ofTool: id)
        } else {
            ShortcutCenter.shared.assign(command, "", force: true)
        }
    }

    private static func setShortcut(_ binding: String, ofTool id: UInt64) {
        var tools = ToolCenter.shared.tools
        guard let index = tools.firstIndex(where: { $0.id == id }), tools[index].shortcut != binding else { return }
        tools[index].shortcut = binding
        ToolCenter.shared.save(tools)
    }
}

extension Notification.Name {
    /// Posted after the tool list is saved.
    static let rustyfinderToolsChanged = Notification.Name("RustyFinderToolsChanged")
}

/// A placeholder token that can be dragged into the command (or clicked
/// to insert it at the cursor).
final class PlaceholderChip: NSButton, NSDraggingSource {
    var onInsert: ((String) -> Void)?
    private var downAt: NSPoint?

    init(token: String) {
        super.init(frame: .zero)
        title = token
        bezelStyle = .accessoryBarAction
        controlSize = .small
        font = .monospacedSystemFont(ofSize: NSFont.smallSystemFontSize, weight: .regular)
        toolTip = "Drag into the command, or click to insert"
        setAccessibilityLabel("Placeholder \(token)")
        target = self
        action = #selector(clicked(_:))
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    @objc private func clicked(_ sender: Any?) {
        onInsert?(title)
    }

    override func mouseDown(with event: NSEvent) {
        downAt = event.locationInWindow
        // Wait for either a drag (start one) or the button going up (a click).
        guard let window else { return super.mouseDown(with: event) }
        while let next = window.nextEvent(matching: [.leftMouseDragged, .leftMouseUp]) {
            if next.type == .leftMouseUp {
                clicked(nil)
                return
            }
            let start = downAt ?? next.locationInWindow
            if hypot(next.locationInWindow.x - start.x, next.locationInWindow.y - start.y) > 3 {
                let item = NSDraggingItem(pasteboardWriter: title as NSString)
                item.setDraggingFrame(bounds, contents: snapshot())
                beginDraggingSession(with: [item], event: event, source: self)
                return
            }
        }
    }

    private func snapshot() -> NSImage? {
        guard let rep = bitmapImageRepForCachingDisplay(in: bounds) else { return nil }
        cacheDisplay(in: bounds, to: rep)
        let image = NSImage(size: bounds.size)
        image.addRepresentation(rep)
        return image
    }

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        .copy
    }
}

/// The palette of placeholders under the command field, in rows.
@MainActor
enum PlaceholderPalette {
    static func make(insert: @escaping (String) -> Void) -> NSView {
        let tokens = toolPlaceholders()
        var rows: [NSView] = []
        for start in stride(from: 0, to: tokens.count, by: 2) {
            let chips = tokens[start..<min(start + 2, tokens.count)].map { token -> NSView in
                let chip = PlaceholderChip(token: token)
                chip.onInsert = insert
                return chip
            }
            let row = NSStackView(views: chips)
            row.spacing = 4
            rows.append(row)
        }
        let hint = NSTextField(labelWithString: "Drag placeholders into your command from here:")
        hint.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        hint.textColor = .secondaryLabelColor
        let column = NSStackView(views: [hint] + rows)
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 4
        return column
    }
}
