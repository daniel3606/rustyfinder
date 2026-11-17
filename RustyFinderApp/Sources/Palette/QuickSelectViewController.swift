import AppKit

/// Quick Select (Cmd-S): select items in the current view by pattern.
/// Matches are selected live while typing; Return keeps the result, Esc
/// puts the original selection back.
final class QuickSelectViewController: NSViewController, NSTextFieldDelegate {
    enum Mode: Int {
        case select, add, remove
    }

    private weak var list: FileListViewController?
    private let original: IndexSet
    private let field = NSTextField()
    private let modePopup = NSPopUpButton()
    /// Picks a tag: items carrying it are matched ("tag:<name>").
    private let tagPopup = NSPopUpButton(frame: .zero, pullsDown: true)
    private let countLabel = NSTextField(labelWithString: "")
    private var kept = false

    init(list: FileListViewController) {
        self.list = list
        self.original = list.selection
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    override func loadView() {
        let root = NSView()
        field.placeholderString = "Text, .ext, a pattern with * and ?, or tag:name"
        field.delegate = self
        field.widthAnchor.constraint(equalToConstant: 300).isActive = true
        modePopup.addItems(withTitles: ["Select Matches", "Add to Selection", "Remove from Selection"])
        modePopup.target = self
        modePopup.action = #selector(modeChanged(_:))
        countLabel.textColor = .secondaryLabelColor
        countLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        let title = NSTextField(labelWithString: "Quick Select")
        title.font = .systemFont(ofSize: NSFont.systemFontSize, weight: .semibold)
        tagPopup.addItem(withTitle: "Tag")
        tagPopup.lastItem?.image = NSImage(systemSymbolName: "tag", accessibilityDescription: "Tag")
        for tag in TagStore.known {
            tagPopup.addItem(withTitle: tag)
            if let color = TagStore.color(for: tag) {
                tagPopup.lastItem?.image = NSImage(size: NSSize(width: 10, height: 10), flipped: false) { rect in
                    color.setFill()
                    NSBezierPath(ovalIn: rect).fill()
                    return true
                }
            }
        }
        tagPopup.target = self
        tagPopup.action = #selector(tagChosen(_:))
        tagPopup.toolTip = "Select items with a tag"
        tagPopup.isEnabled = list?.isRemote == false
        let row = NSStackView(views: [field, modePopup, tagPopup])
        row.spacing = 8
        let column = NSStackView(views: [title, row, countLabel])
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 8
        column.edgeInsets = NSEdgeInsets(top: 14, left: 16, bottom: 12, right: 16)
        column.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(column)
        NSLayoutConstraint.activate([
            column.topAnchor.constraint(equalTo: root.topAnchor),
            column.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            column.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            column.trailingAnchor.constraint(equalTo: root.trailingAnchor),
        ])
        view = root
        apply()
    }

    override func viewDidAppear() {
        super.viewDidAppear()
        view.window?.makeFirstResponder(field)
    }

    func controlTextDidChange(_ notification: Notification) {
        apply()
    }

    @objc private func modeChanged(_ sender: Any?) {
        apply()
    }

    @objc private func tagChosen(_ sender: NSPopUpButton) {
        guard let tag = sender.titleOfSelectedItem, sender.indexOfSelectedItem > 0 else { return }
        field.stringValue = "tag:\(tag)"
        apply()
    }

    /// Items carrying `tag` (local folders only; tags are read per item).
    private func tagMatches(_ tag: String, in snapshot: SnapshotHandle) -> IndexSet {
        guard let list, !list.isRemote else { return [] }
        var matches = IndexSet()
        for index in 0..<Int(snapshot.count()) {
            guard let path = snapshot.pathOf(index: UInt32(index)) else { continue }
            if TagStore.tags(at: path).contains(where: { $0.caseInsensitiveCompare(tag) == .orderedSame }) {
                matches.insert(list.content.shownIndex(ofMain: index))
            }
        }
        return matches
    }

    /// Show the selection this pattern and mode would give.
    private func apply() {
        guard let list, let snapshot = list.content.snapshot else { return }
        let text = field.stringValue
        let matches = text.lowercased().hasPrefix("tag:")
            ? tagMatches(String(text.dropFirst(4)).trimmingCharacters(in: .whitespaces), in: snapshot)
            : IndexSet(snapshot.matchPattern(pattern: text).map { list.content.shownIndex(ofMain: Int($0)) })
        let mode = Mode(rawValue: modePopup.indexOfSelectedItem) ?? .select
        let result: IndexSet
        switch mode {
        case .select: result = matches
        case .add: result = original.union(matches)
        case .remove: result = original.subtracting(matches)
        }
        list.setSelection(result)
        countLabel.stringValue = matches.count == 1 ? "1 match" : "\(matches.count.formatted()) matches"
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.insertNewline(_:)):
            kept = true
            close()
        case #selector(NSResponder.cancelOperation(_:)):
            close()
        default: return false
        }
        return true
    }

    private func close() {
        // Cancelled: the selection goes back to what it was.
        if !kept { list?.setSelection(original) }
        guard let window = view.window else { return }
        window.sheetParent?.endSheet(window)
    }

    // MARK: Scripted checks

    func debugType(_ text: String) {
        loadViewIfNeeded()
        field.stringValue = text
        apply()
    }

    func debugMode(_ index: Int) {
        loadViewIfNeeded()
        modePopup.selectItem(at: index)
        apply()
    }

    func debugKey(_ key: String) {
        if key == "return" { kept = true }
        close()
    }

    var debugCount: String { countLabel.stringValue }
}
