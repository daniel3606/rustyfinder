import AppKit

/// Finder tags on local items. The system reads and writes them (so
/// Finder and Spotlight agree); which tags an edit leaves on each item is
/// decided by the Rust core.
@MainActor
enum TagStore {
    /// The seven colour tags under their system names, in menu order:
    /// (name, colour).
    static var colors: [(name: String, color: NSColor)] {
        let names = NSWorkspace.shared.fileLabels
        let colors = NSWorkspace.shared.fileLabelColors
        // Index 0 is "None". Menu order follows Finder: red first.
        let order = [6, 7, 5, 2, 4, 3, 1]
        return order.compactMap { index in
            names.indices.contains(index) && colors.indices.contains(index) ? (names[index], colors[index]) : nil
        }
    }

    /// The colour of a tag: one of the seven colour tags, or a tag the
    /// user gave a colour in Settings → Tags.
    static func color(for tag: String) -> NSColor? {
        if let own = colors.first(where: { $0.name.caseInsensitiveCompare(tag) == .orderedSame })?.color { return own }
        guard let number = list.first(where: { $0.name.caseInsensitiveCompare(tag) == .orderedSame })?.color, number > 0
        else { return nil }
        let all = NSWorkspace.shared.fileLabelColors
        return all.indices.contains(Int(number)) ? all[Int(number)] : nil
    }

    /// Tags of the item at `path`. An item with only an old-style colour
    /// label and no tags reports that colour.
    static func tags(at path: String) -> [String] {
        let url = URL(fileURLWithPath: path)
        guard let values = try? url.resourceValues(forKeys: [.tagNamesKey, .labelNumberKey]) else { return [] }
        if let tags = values.tagNames, !tags.isEmpty { return tags }
        let names = NSWorkspace.shared.fileLabels
        if let label = values.labelNumber, label > 0, names.indices.contains(label) { return [names[label]] }
        return []
    }

    @discardableResult
    static func setTags(_ tags: [String], at path: String) -> Bool {
        do {
            try (URL(fileURLWithPath: path) as NSURL).setResourceValue(tags, forKey: .tagNamesKey)
            remember(tags)
            return true
        } catch {
            return false
        }
    }

    /// Toggle one tag over several items: off if all have it, else on.
    /// Returns what changed, for Undo.
    @discardableResult
    static func toggle(_ tag: String, on paths: [String]) -> [TagChange] {
        let before = paths.map { tags(at: $0) }
        let after = tagsToggle(items: before, tag: tag)
        var changes: [TagChange] = []
        for ((path, old), new) in zip(zip(paths, before), after) where old != new && setTags(new, at: path) {
            changes.append(TagChange(path: path, before: old, after: new))
        }
        return changes
    }

    /// The tags every one of `paths` has.
    static func common(of paths: [String]) -> [String] {
        tagsCommon(items: paths.map { tags(at: $0) })
    }

    /// Apply an edited list of common tags to all of `paths`. Returns
    /// what changed, for Undo.
    @discardableResult
    static func apply(_ edited: [String], to paths: [String]) -> [TagChange] {
        let before = paths.map { tags(at: $0) }
        let after = tagsApplyCommon(items: before, edited: edited)
        var changes: [TagChange] = []
        for ((path, old), new) in zip(zip(paths, before), after) where old != new && setTags(new, at: path) {
            changes.append(TagChange(path: path, before: old, after: new))
        }
        return changes
    }

    // MARK: The tag list (Settings → Tags; completion and the sidebar)

    private static var cached: [TagEntryRecord]?

    /// The user's tags in their order. On first use the list takes in the
    /// colour tags, Finder's own tag list and tags remembered by older
    /// versions; tags the user deleted stay out.
    static var list: [TagEntryRecord] {
        if let cached { return cached }
        let labels = NSWorkspace.shared.fileLabels
        let order = [6, 7, 5, 2, 4, 3, 1]
        var found = order.compactMap { index in
            labels.indices.contains(index) ? TagEntryRecord(name: labels[index], color: UInt8(index), sidebar: true) : nil
        }
        found += finderTags()
        if ProcessInfo.processInfo.environment["RUSTYFINDER_SUPPORT_DIR"] == nil {
            found += (UserDefaults.standard.stringArray(forKey: "tags.custom") ?? []).map { TagEntryRecord(name: $0, color: 0, sidebar: true) }
        }
        let merged = AppEnvironment.shared.core.mergeTags(found: found)
        cached = merged
        return merged
    }

    /// Finder's own tag list (names and colours), read-only. Scripted
    /// checks read a fixture instead of the user's file, if any.
    static func finderTags() -> [TagEntryRecord] {
        let env = ProcessInfo.processInfo.environment
        let path: String
        if let fixture = env["RUSTYFINDER_FINDER_TAGS_PLIST"] {
            path = fixture
        } else if env["RUSTYFINDER_SUPPORT_DIR"] != nil {
            return []
        } else {
            path = NSHomeDirectory() + "/Library/SyncedPreferences/com.apple.finder.plist"
        }
        guard let data = FileManager.default.contents(atPath: path),
              let plist = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any],
              let values = plist["values"] as? [String: Any],
              let dict = (values["FinderTagDict"] as? [String: Any])?["value"] as? [String: Any],
              let tags = dict["FinderTags"] as? [[String: Any]]
        else { return [] }
        return tags.compactMap { tag in
            guard let name = tag["n"] as? String, !name.isEmpty else { return nil }
            let label = (tag["l"] as? Int) ?? 0
            return TagEntryRecord(name: name, color: UInt8(max(0, min(7, label))), sidebar: true)
        }
    }

    static func setList(_ tags: [TagEntryRecord]) {
        cached = AppEnvironment.shared.core.setTagList(tags: tags)
        NotificationCenter.default.post(name: .rustyfinderTagsChanged, object: nil)
    }

    static func listChanged(_ tags: [TagEntryRecord]) {
        cached = tags
        NotificationCenter.default.post(name: .rustyfinderTagsChanged, object: nil)
    }

    private static func remember(_ tags: [String]) {
        for tag in tags where !list.contains(where: { $0.name.caseInsensitiveCompare(tag) == .orderedSame }) {
            listChanged(AppEnvironment.shared.core.rememberTag(name: tag, color: 0))
        }
    }

    /// Every tag, for completion: the list's order.
    static var known: [String] { list.map(\.name) }

    /// Tags shown in the sidebar.
    static var sidebarTags: [String] { list.filter(\.sidebar).map(\.name) }

    static func setSidebar(_ name: String, shown: Bool) {
        setList(list.map { $0.name == name ? TagEntryRecord(name: $0.name, color: $0.color, sidebar: shown) : $0 })
    }

    /// Add one tag to items (a drop on a sidebar tag).
    static func add(_ tag: String, to paths: [String]) {
        for path in paths {
            let before = tags(at: path)
            let after = tagsAdd(tags: before, tag: tag)
            if after != before { setTags(after, at: path) }
        }
    }

    /// Rename a tag on these items (found by Spotlight and in open
    /// folders). Returns how many items changed.
    @discardableResult
    static func rename(_ old: String, to new: String, on paths: [String]) -> Int {
        var changed = 0
        for path in paths {
            let before = tags(at: path)
            guard let after = tagsRename(tags: before, old: old, new: new), after != before else { continue }
            if setTags(after, at: path) { changed += 1 }
        }
        return changed
    }

    /// Remove a tag from these items. Returns how many items changed.
    @discardableResult
    static func remove(_ tag: String, from paths: [String]) -> Int {
        var changed = 0
        for path in paths {
            let before = tags(at: path)
            let after = before.filter { $0.caseInsensitiveCompare(tag) != .orderedSame }
            if after != before, setTags(after, at: path) { changed += 1 }
        }
        return changed
    }

    /// Up to three dots for an item's colour tags, in tag order.
    static func dots(for tags: [String]) -> NSAttributedString {
        let text = NSMutableAttributedString()
        for color in tags.compactMap(color(for:)).prefix(3) {
            text.append(NSAttributedString(string: "●", attributes: [
                .foregroundColor: color, .font: NSFont.systemFont(ofSize: 9),
            ]))
        }
        return text
    }
}

/// The row of colour swatches in the context menu. A click toggles that
/// colour tag on the selection and closes the menu.
final class TagSwatchesView: NSView {
    var onPick: ((String) -> Void)?
    private var names: [String] = []

    init(selected: Set<String>) {
        super.init(frame: NSRect(x: 0, y: 0, width: 220, height: 28))
        let entries = MainActor.assumeIsolated { TagStore.colors }
        names = entries.map(\.name)
        for (index, entry) in entries.enumerated() {
            let button = NSButton(frame: NSRect(x: 18 + index * 26, y: 5, width: 18, height: 18))
            button.title = ""
            button.isBordered = false
            button.wantsLayer = true
            button.layer?.cornerRadius = 9
            button.layer?.backgroundColor = entry.color.cgColor
            // A ring marks a tag that every selected item already has.
            if selected.contains(entry.name) {
                button.layer?.borderWidth = 2
                button.layer?.borderColor = NSColor.labelColor.cgColor
            }
            button.tag = index
            button.target = self
            button.action = #selector(picked(_:))
            button.toolTip = entry.name
            button.setAccessibilityLabel(entry.name)
            addSubview(button)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    @objc private func picked(_ sender: NSButton) {
        enclosingMenuItem?.menu?.cancelTracking()
        onPick?(names[sender.tag])
    }
}

/// "Tags…": a token field in a pop-over. It shows the tags every selected
/// item has; adding a token adds the tag to all, removing one removes it
/// from all. Changes apply at once.
final class TagsPopoverController: NSViewController, NSTokenFieldDelegate {
    private let paths: [String]
    private let field = NSTokenField()
    var onChange: (() -> Void)?
    /// Each item's tags when the pop-over opened.
    private let original: [String: [String]]
    /// Called once as the pop-over goes away, with every item whose tags
    /// changed while it was open: one undo step for the whole edit.
    var onFinish: (([TagChange]) -> Void)?

    init(paths: [String]) {
        self.paths = paths
        self.original = Dictionary(paths.map { ($0, TagStore.tags(at: $0)) }, uniquingKeysWith: { a, _ in a })
        super.init(nibName: nil, bundle: nil)
    }

    override func viewDidDisappear() {
        super.viewDidDisappear()
        finish()
    }

    /// Report the net change once.
    func finish() {
        guard let report = onFinish else { return }
        onFinish = nil
        report(paths.compactMap { path in
            let now = TagStore.tags(at: path)
            let then = original[path] ?? []
            return now == then ? nil : TagChange(path: path, before: then, after: now)
        })
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    override func loadView() {
        let root = NSView(frame: NSRect(x: 0, y: 0, width: 300, height: 64))
        let label = NSTextField(labelWithString: paths.count == 1 ? "Tags" : "Tags of \(paths.count) items")
        label.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        label.textColor = .secondaryLabelColor
        field.delegate = self
        field.objectValue = TagStore.common(of: paths)
        field.placeholderString = "Add tags"
        field.setAccessibilityLabel("Tags")
        let stack = NSStackView(views: [label, field])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 6
        stack.edgeInsets = NSEdgeInsets(top: 10, left: 12, bottom: 12, right: 12)
        stack.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: root.topAnchor),
            stack.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            field.widthAnchor.constraint(equalToConstant: 276),
        ])
        view = root
    }

    func tokenField(
        _ tokenField: NSTokenField, completionsForSubstring substring: String, indexOfToken tokenIndex: Int,
        indexOfSelectedItem selectedIndex: UnsafeMutablePointer<Int>?
    ) -> [Any]? {
        tagsComplete(known: TagStore.known, typed: substring)
    }

    /// Every edit (a token confirmed with Enter or Tab, or removed) is
    /// applied straight away.
    func controlTextDidChange(_ notification: Notification) { commit() }
    func controlTextDidEndEditing(_ notification: Notification) { commit() }

    private func commit() {
        let edited = (field.objectValue as? [String]) ?? []
        apply(edited)
    }

    func apply(_ edited: [String]) {
        TagStore.apply(edited, to: paths)
        onChange?()
    }

    /// What the field would offer for typed text (scripted checks).
    func debugCompletions(_ typed: String) -> [String] {
        tagsComplete(known: TagStore.known, typed: typed)
    }
}
