import AppKit

/// The menu commands and their shortcuts. The defaults are whatever the
/// menus were built with; the user's changes are kept by the core and
/// applied back onto the menu items, so the menus always show the
/// binding in force.
@MainActor
final class ShortcutCenter {
    static let shared = ShortcutCenter()
    private var core: RustyFinderApp { AppEnvironment.shared.core }

    /// One command: its id (menu path), where it lives, and its menu item.
    struct Command {
        let id: String
        let menu: String
        let title: String
        let item: NSMenuItem
    }

    private(set) var commands: [Command] = []
    private(set) var bindings: [String: String] = [:]
    private var defaults: [String: String] = [:]

    /// Commands bound to a key with no modifier at all (scripted checks).
    var bareSummary: String {
        commands.filter { !$0.item.keyEquivalent.isEmpty && $0.item.keyEquivalentModifierMask.intersection([.command, .control, .option, .shift, .function]).isEmpty }
            .map { "\($0.id)=\(bindings[$0.id] ?? "?")" }.joined(separator: ", ")
    }

    /// The shortcuts that differ from the defaults, for scripted checks.
    var customSummary: String {
        commands.filter { bindings[$0.id] != defaults[$0.id] }
            .map { "\($0.id)=\(bindings[$0.id] ?? "")" }.joined(separator: ", ")
    }

    /// Menus whose items come and go (their entries are not commands).
    private static let dynamicMenus: Set<String> = ["Tools", "Recent Folders", "Connections", "Workspaces", "Services", "Open With"]

    /// Read the commands and their default shortcuts from the main menu,
    /// then lay the saved changes over them. Call once the menu is built.
    func install(_ mainMenu: NSMenu) {
        commands = []
        func walk(_ menu: NSMenu, path: [String]) {
            for item in menu.items where !item.isSeparatorItem && !item.isHidden {
                if let submenu = item.submenu {
                    if !Self.dynamicMenus.contains(item.title) { walk(submenu, path: path + [item.title]) }
                } else if item.action != nil, !path.isEmpty {
                    let id = (path + [item.title]).joined(separator: "/")
                    // The same title twice in one menu keeps the first.
                    if !commands.contains(where: { $0.id == id }) {
                        commands.append(Command(id: id, menu: path.joined(separator: " ▸ "), title: item.title, item: item))
                    }
                }
            }
        }
        // The application menu has no title of its own.
        for item in mainMenu.items where item.title.isEmpty { item.title = "RustyFinder" }
        walk(mainMenu, path: [])
        let entries = commands.map { ShortcutEntry(command: $0.id, binding: ShortcutKeys.binding(of: $0.item)) }
        defaults = Dictionary(entries.map { ($0.command, $0.binding) }, uniquingKeysWith: { a, _ in a })
        apply(core.shortcutsLoad(defaults: entries))
    }

    private func apply(_ entries: [ShortcutEntry]) {
        bindings = Dictionary(entries.map { ($0.command, $0.binding) }, uniquingKeysWith: { a, _ in a })
        for command in commands {
            let key = ShortcutKeys.menuKey(for: bindings[command.id] ?? "")
            command.item.keyEquivalent = key?.key ?? ""
            command.item.keyEquivalentModifierMask = key?.modifiers ?? []
        }
    }

    func binding(of command: String) -> String {
        if ToolShortcuts.toolId(command) != nil { return ToolShortcuts.binding(of: command) }
        return bindings[command] ?? ""
    }

    /// Give a command a shortcut ("" removes it). Returns the command that
    /// already has it, in which case nothing changed: ask, then `force`.
    @discardableResult
    func assign(_ command: String, _ binding: String, force: Bool = false) -> String? {
        if let tool = ToolShortcuts.toolId(command) {
            return ToolShortcuts.assign(tool: tool, binding, force: force)
        }
        // Tools share the key space with the menus.
        if let tool = ToolShortcuts.toolHolder(of: binding, except: command) {
            guard force else { return tool }
            ToolShortcuts.release(tool)
        }
        let result = core.shortcutAssign(command: command, binding: binding, force: force)
        apply(result.entries)
        return result.conflict
    }

    func restoreDefaults() {
        apply(core.shortcutsRestore())
    }

    func title(of command: String) -> String {
        if let tool = ToolShortcuts.title(of: command) { return tool }
        return commands.first { $0.id == command }.map { "\($0.title) (\($0.menu))" } ?? command
    }
}
