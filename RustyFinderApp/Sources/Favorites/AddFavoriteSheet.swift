import AppKit
import UniformTypeIdentifiers

/// The Add to Favorites sheet: a name (prefilled), a favorite group
/// (existing ones offered, a new name creates one) and an icon (the
/// item's own, a built-in symbol, or an image file).
final class AddFavoriteSheet: NSViewController {
    struct Result {
        let draft: FavoriteDraftRecord
        let group: String
    }

    private(set) static var current: AddFavoriteSheet?

    private var draft: FavoriteDraftRecord
    private let initialGroup: String
    private let nameField = NSTextField()
    private let groupBox = NSComboBox()
    private let iconPopup = NSPopUpButton()
    private var customIcon: String?
    private var completion: ((Result?) -> Void)?

    init(draft: FavoriteDraftRecord, group: String) {
        self.draft = draft
        self.initialGroup = group
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    override func loadView() {
        let title = NSTextField(labelWithString: draft.kind == .workspace ? "Save Workspace" : "Add to Favorites")
        title.font = .systemFont(ofSize: NSFont.systemFontSize, weight: .semibold)
        nameField.stringValue = draft.name
        nameField.placeholderString = "Name"
        nameField.setAccessibilityLabel("Name")
        let groups = AppEnvironment.shared.core.favoriteGroups().map(\.name)
        groupBox.addItems(withObjectValues: groups.isEmpty ? ["Favorites"] : groups)
        groupBox.stringValue = initialGroup
        groupBox.completes = true
        groupBox.setAccessibilityLabel("Favorite group")
        fillIcons()
        iconPopup.target = self
        iconPopup.action = #selector(iconChosen(_:))
        iconPopup.setAccessibilityLabel("Icon")

        func row(_ label: String, _ control: NSView) -> NSStackView {
            let text = NSTextField(labelWithString: label)
            text.alignment = .right
            text.widthAnchor.constraint(equalToConstant: 60).isActive = true
            control.widthAnchor.constraint(equalToConstant: 260).isActive = true
            let row = NSStackView(views: [text, control])
            row.alignment = .firstBaseline
            return row
        }
        let detail = NSTextField(labelWithString: draft.path.isEmpty ? "" : draft.path.abbreviatingHome)
        detail.textColor = .secondaryLabelColor
        detail.lineBreakMode = .byTruncatingMiddle
        detail.widthAnchor.constraint(lessThanOrEqualToConstant: 330).isActive = true
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancel(_:)))
        cancel.keyEquivalent = "\u{1b}"
        let add = NSButton(title: draft.kind == .workspace ? "Save" : "Add", target: self, action: #selector(save(_:)))
        add.keyEquivalent = "\r"
        let buttons = NSStackView(views: [cancel, add])
        let stack = NSStackView(views: [title, detail, row("Name:", nameField), row("Group:", groupBox), row("Icon:", iconPopup), buttons])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 10
        stack.edgeInsets = NSEdgeInsets(top: 16, left: 18, bottom: 14, right: 18)
        stack.setCustomSpacing(16, after: stack.arrangedSubviews[4])
        buttons.leadingAnchor.constraint(greaterThanOrEqualTo: stack.leadingAnchor, constant: 240).isActive = true
        view = stack
    }

    private func fillIcons() {
        iconPopup.removeAllItems()
        iconPopup.addItem(withTitle: "Default")
        iconPopup.lastItem?.representedObject = ""
        iconPopup.menu?.addItem(.separator())
        for (title, symbol) in FavoriteIcons.choices {
            iconPopup.addItem(withTitle: title)
            iconPopup.lastItem?.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
            iconPopup.lastItem?.representedObject = symbol
        }
        iconPopup.menu?.addItem(.separator())
        if let custom = customIcon ?? (draft.icon.hasPrefix("/") ? draft.icon : nil) {
            iconPopup.addItem(withTitle: (custom as NSString).lastPathComponent)
            iconPopup.lastItem?.image = FavoriteIcons.image(icon: custom)
            iconPopup.lastItem?.representedObject = custom
        }
        iconPopup.addItem(withTitle: "Choose Image…")
        iconPopup.lastItem?.representedObject = "choose"
        let index = iconPopup.itemArray.firstIndex { ($0.representedObject as? String) == (customIcon ?? draft.icon) } ?? 0
        iconPopup.selectItem(at: index)
    }

    @objc private func iconChosen(_ sender: NSPopUpButton) {
        guard (sender.selectedItem?.representedObject as? String) == "choose", let window = view.window else { return }
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.image]
        panel.beginSheetModal(for: window) { [weak self] response in
            guard let self else { return }
            if response == .OK, let url = panel.url { self.customIcon = url.path }
            self.fillIcons()
        }
    }

    func begin(on window: NSWindow, completion: @escaping (Result?) -> Void) {
        self.completion = completion
        Self.current = self
        let sheet = NSWindow(contentViewController: self)
        sheet.styleMask = [.titled]
        window.beginSheet(sheet)
        sheet.makeFirstResponder(nameField)
    }

    private func finish(_ result: Result?) {
        Self.current = nil
        if let window = view.window { window.sheetParent?.endSheet(window) }
        completion?(result)
        completion = nil
    }

    @objc private func cancel(_ sender: Any?) { finish(nil) }

    @objc private func save(_ sender: Any?) {
        var draft = self.draft
        draft.name = nameField.stringValue.trimmingCharacters(in: .whitespaces)
        let icon = iconPopup.selectedItem?.representedObject as? String ?? ""
        draft.icon = icon == "choose" ? "" : icon
        let group = groupBox.stringValue.trimmingCharacters(in: .whitespaces)
        finish(Result(draft: draft, group: group.isEmpty ? "Favorites" : group))
    }

    // MARK: Scripted checks

    /// `name=<text>`, `group=<text>`, `icon=<symbol>`, `save`, `cancel`.
    func debugSet(_ what: String) {
        loadViewIfNeeded()
        let parts = what.split(separator: "=", maxSplits: 1).map(String.init)
        let value = parts.count > 1 ? parts[1] : ""
        switch parts[0] {
        case "name": nameField.stringValue = value
        case "group": groupBox.stringValue = value
        case "icon":
            if let index = iconPopup.itemArray.firstIndex(where: { ($0.representedObject as? String) == value }) {
                iconPopup.selectItem(at: index)
            }
        case "save": save(nil)
        case "cancel": cancel(nil)
        default: break
        }
    }

    func debugDump() -> String {
        "name=\(nameField.stringValue) group=\(groupBox.stringValue) icon=\(iconPopup.selectedItem?.representedObject as? String ?? "")"
    }
}
