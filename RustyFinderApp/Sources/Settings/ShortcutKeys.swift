import AppKit

/// Converting between the three forms a shortcut takes: the text the core
/// stores ("cmd+shift+n"), a menu item's key equivalent, and a key event.
enum ShortcutKeys {
    /// Named keys: the text name, the character a menu item uses, and the
    /// hardware key code.
    private static let named: [(name: String, menu: String, code: UInt16)] = [
        ("delete", "\u{8}", 51), ("forwarddelete", "\u{7F}", 117), ("return", "\r", 36), ("enter", "\u{3}", 76),
        ("tab", "\t", 48), ("space", " ", 49), ("escape", "\u{1B}", 53),
        ("up", function(NSUpArrowFunctionKey), 126), ("down", function(NSDownArrowFunctionKey), 125),
        ("left", function(NSLeftArrowFunctionKey), 123), ("right", function(NSRightArrowFunctionKey), 124),
        ("home", function(NSHomeFunctionKey), 115), ("end", function(NSEndFunctionKey), 119),
        ("pageup", function(NSPageUpFunctionKey), 116), ("pagedown", function(NSPageDownFunctionKey), 121),
    ]
    /// Key codes of F1…F12.
    private static let functionCodes: [UInt16] = [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111]

    private static func function(_ key: Int) -> String {
        String(UnicodeScalar(UInt16(key))!)
    }

    private static let modifiers: [(String, NSEvent.ModifierFlags)] = [
        ("fn", .function), ("ctrl", .control), ("alt", .option), ("shift", .shift), ("cmd", .command),
    ]

    /// The key equivalent and modifiers for a binding; nil for none.
    static func menuKey(for binding: String) -> (key: String, modifiers: NSEvent.ModifierFlags)? {
        guard !binding.isEmpty else { return nil }
        var words = binding.split(separator: "+").map(String.init)
        guard let key = words.popLast() else { return nil }
        var flags: NSEvent.ModifierFlags = []
        for word in words {
            guard let flag = modifiers.first(where: { $0.0 == word })?.1 else { return nil }
            flags.insert(flag)
        }
        if let entry = named.first(where: { $0.name == key }) { return (entry.menu, flags) }
        if key == "plus" { return ("+", flags) }
        if key.hasPrefix("f"), let number = Int(key.dropFirst()), (1...20).contains(number) {
            return (function(NSF1FunctionKey + number - 1), flags)
        }
        return key.count == 1 ? (key, flags) : nil
    }

    /// The binding a menu item has now ("" for none).
    static func binding(of item: NSMenuItem) -> String {
        binding(key: item.keyEquivalent, modifiers: item.keyEquivalentModifierMask)
    }

    static func binding(key: String, modifiers mask: NSEvent.ModifierFlags) -> String {
        guard !key.isEmpty else { return "" }
        var flags = mask.intersection([.control, .option, .shift, .command])
        var name = key
        // The Globe (fn) key counts for ordinary keys only: arrows and
        // function keys carry that flag by nature.
        let isSpecial = named.contains { $0.menu == key }
            || key.unicodeScalars.first.map { (NSF1FunctionKey...NSF20FunctionKey).contains(Int($0.value)) } == true
        if mask.contains(.function), !isSpecial { flags.insert(.function) }
        if let entry = named.first(where: { $0.menu == key }) {
            name = entry.name
        } else if key == "+" {
            name = "plus"
        } else if let scalar = key.unicodeScalars.first, (NSF1FunctionKey...NSF20FunctionKey).contains(Int(scalar.value)) {
            name = "f\(Int(scalar.value) - NSF1FunctionKey + 1)"
        } else if key != key.lowercased() {
            // An upper-case key equivalent means Shift.
            flags.insert(.shift)
            name = key.lowercased()
        }
        let held = modifiers.filter { flags.contains($0.1) }.map(\.0)
        return (held + [name]).joined(separator: "+")
    }

    /// The binding a key press stands for; nil for a press of modifiers only.
    static func binding(for event: NSEvent) -> String? {
        var flags = event.modifierFlags.intersection([.control, .option, .shift, .command])
        let special = named.contains { $0.code == event.keyCode } || functionCodes.contains(event.keyCode)
        if event.modifierFlags.contains(.function), !special { flags.insert(.function) }
        var name: String
        if let entry = named.first(where: { $0.code == event.keyCode }) {
            name = entry.name
        } else if let index = functionCodes.firstIndex(of: event.keyCode) {
            name = "f\(index + 1)"
        } else if let chars = event.charactersIgnoringModifiers?.lowercased(), chars.count == 1 {
            name = chars == "+" ? "plus" : chars
        } else {
            return nil
        }
        // With Option held the character is the composed one ("å"); use
        // the plain key instead.
        if flags.contains(.option), let plain = event.characters(byApplyingModifiers: [])?.lowercased(), plain.count == 1,
           named.first(where: { $0.code == event.keyCode }) == nil, functionCodes.firstIndex(of: event.keyCode) == nil
        {
            name = plain == "+" ? "plus" : plain
        }
        let held = modifiers.filter { flags.contains($0.1) }.map(\.0)
        return (held + [name]).joined(separator: "+")
    }
}

/// A field that records the next key combination pressed while it has
/// the keyboard. Esc cancels; the X clears the binding.
final class ShortcutRecorderField: NSView {
    var binding = "" { didSet { refresh() } }
    /// Called with the new binding ("" when cleared).
    var onChange: ((String) -> Void)?
    private let label = NSTextField(labelWithString: "")
    private let clear = NSButton(title: "✕", target: nil, action: nil)
    private var recording = false { didSet { refresh() } }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 5
        layer?.borderWidth = 1
        label.alignment = .center
        label.translatesAutoresizingMaskIntoConstraints = false
        clear.isBordered = false
        clear.font = .systemFont(ofSize: 10)
        clear.target = self
        clear.action = #selector(clearBinding(_:))
        clear.translatesAutoresizingMaskIntoConstraints = false
        clear.setAccessibilityLabel("Clear shortcut")
        addSubview(label)
        addSubview(clear)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 6),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            clear.leadingAnchor.constraint(equalTo: label.trailingAnchor, constant: 2),
            clear.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -4),
            clear.centerYAnchor.constraint(equalTo: centerYAnchor),
            clear.widthAnchor.constraint(equalToConstant: 16),
            heightAnchor.constraint(equalToConstant: 22),
            widthAnchor.constraint(greaterThanOrEqualToConstant: 110),
        ])
        setAccessibilityRole(.button)
        refresh()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    override var acceptsFirstResponder: Bool { true }

    private func refresh() {
        label.stringValue = recording ? "Type shortcut…" : (binding.isEmpty ? "None" : shortcutDisplay(binding: binding))
        label.textColor = recording ? .secondaryLabelColor : (binding.isEmpty ? .tertiaryLabelColor : .labelColor)
        clear.isHidden = binding.isEmpty || recording
        layer?.borderColor = (recording ? NSColor.controlAccentColor : NSColor.separatorColor).cgColor
        setAccessibilityLabel("Shortcut: \(label.stringValue)")
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        recording = true
    }

    override func resignFirstResponder() -> Bool {
        recording = false
        return true
    }

    override func keyDown(with event: NSEvent) {
        guard recording else { return super.keyDown(with: event) }
        // Esc alone leaves recording without changing anything.
        if event.keyCode == 53, event.modifierFlags.intersection([.control, .option, .shift, .command]).isEmpty {
            recording = false
            return
        }
        guard let pressed = ShortcutKeys.binding(for: event) else { return }
        recording = false
        onChange?(pressed)
    }

    /// Menu shortcuts must not fire while a combination is being recorded.
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard recording else { return false }
        keyDown(with: event)
        return true
    }

    @objc private func clearBinding(_ sender: Any?) {
        onChange?("")
    }
}
