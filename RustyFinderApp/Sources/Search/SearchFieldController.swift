import AppKit

/// The toolbar search field: its options menu, and the rule for when
/// typing filters the folder live and when Return starts a search.
@MainActor
final class SearchFieldController: NSObject, NSSearchFieldDelegate {
    enum Criterion: String, CaseIterable {
        case name = "Name", fileExtension = "Extension", kind = "Kind", tags = "Tags", content = "Content"
    }

    static let itemId = NSToolbarItem.Identifier("rustyfinder.search")
    let item = NSSearchToolbarItem(itemIdentifier: SearchFieldController.itemId)
    var field: NSSearchField { item.searchField }

    private(set) var criterion = Criterion.name
    private(set) var caseSensitive = false
    private(set) var regex = false
    /// The engine and the subfolder switch are remembered apart for this
    /// Mac and for servers.
    var isRemote = false {
        didSet { if isRemote != oldValue { rebuildMenu() } }
    }

    /// Typing while the folder is filtered live.
    var onFilter: ((String) -> Void)?
    /// Return: the query (as the core reads it) and whether to ask Spotlight.
    var onSearch: ((String, Bool) -> Void)?
    /// Esc or an emptied field.
    var onEnd: (() -> Void)?
    /// Return while filtering: back to the file list.
    var onDone: (() -> Void)?
    /// Whether hidden items are on show (they are then searched too).
    var showsHidden: () -> Bool = { false }

    private var scope: String { isRemote ? "remote" : "local" }
    /// Scripted checks run with their own support folder and must not
    /// read or change the user's remembered choices.
    private static let remembers = ProcessInfo.processInfo.environment["RUSTYFINDER_SUPPORT_DIR"] == nil
    private var scratch: [String: Bool] = [:]

    private func flag(_ name: String) -> Bool {
        let key = "search.\(scope).\(name)"
        return Self.remembers ? UserDefaults.standard.bool(forKey: key) : scratch[key] ?? false
    }

    private func setFlag(_ name: String, _ on: Bool) {
        let key = "search.\(scope).\(name)"
        if Self.remembers { UserDefaults.standard.set(on, forKey: key) } else { scratch[key] = on }
    }

    var spotlight: Bool {
        get { flag("spotlight") }
        set { setFlag("spotlight", newValue) }
    }

    var subfolders: Bool {
        get { flag("subfolders") }
        set { setFlag("subfolders", newValue) }
    }

    /// Plain name matching in this folder only: filter as you type.
    /// Anything else is a search, started with Return.
    var isLive: Bool {
        criterion == .name && !subfolders && !spotlight && !regex && !caseSensitive
    }

    override init() {
        super.init()
        item.label = "Search"
        field.delegate = self
        field.sendsSearchStringImmediately = true
        field.setAccessibilityLabel("Search")
        rebuildMenu()
    }

    func focus() {
        field.window?.makeFirstResponder(field)
    }

    /// Show `text` without treating it as typing.
    func show(_ text: String) {
        guard field.currentEditor() == nil, field.stringValue != text else { return }
        field.stringValue = text
    }

    // MARK: Options menu

    private func rebuildMenu() {
        let menu = NSMenu(title: "Search")
        func add(_ title: String, _ key: String, on: Bool) {
            let entry = NSMenuItem(title: title, action: #selector(pick(_:)), keyEquivalent: "")
            entry.target = self
            entry.representedObject = key
            entry.state = on ? .on : .off
            menu.addItem(entry)
        }
        for value in Criterion.allCases { add(value.rawValue, "criterion:\(value.rawValue)", on: criterion == value) }
        menu.addItem(.separator())
        add("RustyFinder Search", "engine:rustyfinder", on: !spotlight)
        add("Spotlight", "engine:spotlight", on: spotlight)
        menu.addItem(.separator())
        add("Search in Subfolders", "subfolders", on: subfolders)
        add("Case Sensitive", "case", on: caseSensitive)
        add("Regular Expression", "regex", on: regex)
        field.searchMenuTemplate = menu
        field.placeholderString = isLive ? "Filter" : "Search"
    }

    @objc private func pick(_ sender: NSMenuItem) {
        guard let key = sender.representedObject as? String else { return }
        set(option: key)
    }

    /// Change one option by its key (also used by the scripted checks):
    /// `criterion:<Name|Extension|Kind|Tags|Content>`, `engine:<rustyfinder|spotlight>`,
    /// `subfolders`, `case`, `regex`.
    func set(option key: String) {
        let wasLive = isLive
        if key.hasPrefix("criterion:"), let value = Criterion(rawValue: String(key.dropFirst(10))) {
            criterion = value
        } else if key.hasPrefix("engine:") {
            spotlight = key.hasSuffix("spotlight")
        } else if key == "subfolders" {
            subfolders.toggle()
        } else if key == "case" {
            caseSensitive.toggle()
        } else if key == "regex" {
            regex.toggle()
        }
        rebuildMenu()
        // Typed text that was filtering live no longer does.
        if wasLive, !isLive { onFilter?("") }
        if !wasLive, isLive { onFilter?(field.stringValue) }
    }

    // MARK: Typing

    func controlTextDidChange(_ notification: Notification) {
        let text = field.stringValue
        if text.isEmpty {
            onEnd?()
        } else if isLive {
            onFilter?(text)
        }
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        if selector == #selector(NSResponder.insertNewline(_:)) {
            submit()
            return true
        }
        if selector == #selector(NSResponder.cancelOperation(_:)) {
            cancel()
            return true
        }
        return false
    }

    /// Return.
    func submit() {
        let text = field.stringValue
        guard !text.isEmpty else { return }
        if isLive {
            onDone?()
            return
        }
        // A tag can only be found through the index.
        let useSpotlight = spotlight || criterion == .tags
        let query: [String: Any] = [
            "term": text,
            "criterion": criterion.rawValue,
            "case_sensitive": caseSensitive,
            "regex": regex,
            "subfolders": subfolders,
            "hidden": showsHidden(),
            // Reading content on a server downloads it: skip big files there.
            "content_limit": isRemote ? 10_000_000 : 0,
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: query),
              let json = String(data: data, encoding: .utf8)
        else { return }
        onSearch?(json, useSpotlight)
    }

    /// Esc.
    func cancel() {
        field.stringValue = ""
        onEnd?()
        onDone?()
    }

    // MARK: Scripted checks

    /// Replace the text as if typed.
    func debugType(_ text: String) {
        field.stringValue = text
        controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: field))
    }

    func debugDump() -> String {
        "text=\(field.stringValue) criterion=\(criterion.rawValue) subfolders=\(subfolders) "
            + "engine=\(spotlight ? "spotlight" : "rustyfinder") live=\(isLive)"
    }
}
