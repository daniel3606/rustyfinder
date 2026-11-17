import AppKit

/// "Compress with Options…": name, format, password, level, metadata.
@MainActor
final class CompressSheet: NSObject {
    struct Choice {
        var name: String
        var format: ArchiveFormat
        var level: CompressionLevel
        var password: String?
        var excludeMetadata: Bool
    }

    private static let formats: [(String, ArchiveFormat)] = [
        ("Zip", .zip), ("Tar + Gzip", .tarGz), ("Tar + Bzip2", .tarBz2), ("Tar + XZ", .tarXz), ("7-Zip", .sevenZip),
    ]
    private static let levels: [(String, CompressionLevel)] = [
        ("Store (no compression)", .store), ("Fast", .fast), ("Normal", .normal), ("Best", .best),
    ]

    private let alert = NSAlert()
    private let nameField = NSTextField(frame: NSRect(x: 0, y: 0, width: 240, height: 22))
    private let formatPopup = NSPopUpButton()
    private let levelPopup = NSPopUpButton()
    private let passwordField = NSSecureTextField(frame: NSRect(x: 0, y: 0, width: 240, height: 22))
    private let metadataBox = NSButton(checkboxWithTitle: "Exclude macOS metadata (.DS_Store, resource forks)", target: nil, action: nil)
    /// The name without extension, so the extension can follow the format.
    private var base: String

    init(baseName: String) {
        base = baseName
        super.init()
        alert.messageText = "Compress"
        alert.addButton(withTitle: "Compress")
        alert.addButton(withTitle: "Cancel")
        formatPopup.addItems(withTitles: Self.formats.map(\.0))
        formatPopup.target = self
        formatPopup.action = #selector(formatChanged(_:))
        levelPopup.addItems(withTitles: Self.levels.map(\.0))
        levelPopup.selectItem(at: 2)
        metadataBox.state = .on
        passwordField.placeholderString = "None"
        nameField.setAccessibilityLabel("Archive name")
        passwordField.setAccessibilityLabel("Password")

        let grid = NSGridView(views: [
            [NSTextField(labelWithString: "Archive name:"), nameField],
            [NSTextField(labelWithString: "Format:"), formatPopup],
            [NSTextField(labelWithString: "Password:"), passwordField],
            [NSTextField(labelWithString: "Compression:"), levelPopup],
            [NSGridCell.emptyContentView, metadataBox],
        ])
        grid.column(at: 0).xPlacement = .trailing
        grid.rowSpacing = 8
        grid.frame = NSRect(origin: .zero, size: grid.fittingSize)
        alert.accessoryView = grid
        alert.window.initialFirstResponder = nameField
        formatChanged(nil)
    }

    private var format: ArchiveFormat { Self.formats[max(0, formatPopup.indexOfSelectedItem)].1 }

    /// The extension follows the format; zip and 7z take a password.
    @objc private func formatChanged(_ sender: Any?) {
        // Keep what the user typed, minus any extension we put there.
        let typed = nameField.stringValue
        for (_, known) in Self.formats {
            let ext = "." + archiveExtension(format: known)
            if typed.hasSuffix(ext) {
                base = String(typed.dropLast(ext.count))
                break
            }
        }
        if !typed.isEmpty, !Self.formats.contains(where: { typed.hasSuffix("." + archiveExtension(format: $0.1)) }) {
            base = typed
        }
        nameField.stringValue = "\(base).\(archiveExtension(format: format))"
        passwordField.isEnabled = format == .zip || format == .sevenZip
        if !passwordField.isEnabled { passwordField.stringValue = "" }
    }

    private var choice: Choice {
        let password = passwordField.stringValue
        return Choice(
            name: nameField.stringValue, format: format,
            level: Self.levels[max(0, levelPopup.indexOfSelectedItem)].1,
            password: password.isEmpty || !passwordField.isEnabled ? nil : password,
            excludeMetadata: metadataBox.state == .on)
    }

    func begin(on window: NSWindow, completion: @escaping (Choice?) -> Void) {
        Self.current = self
        alert.beginSheetModal(for: window) { [self] response in
            Self.current = nil
            let picked = choice
            completion(response == .alertFirstButtonReturn && !picked.name.isEmpty ? picked : nil)
        }
    }

    // MARK: Scripted checks

    private(set) static var current: CompressSheet?

    /// `format=<index>`, `password=<text>`, `name=<text>`.
    func debugSet(_ what: String) {
        let parts = what.split(separator: "=", maxSplits: 1).map(String.init)
        guard parts.count == 2 else { return }
        switch parts[0] {
        case "format":
            formatPopup.selectItem(at: Int(parts[1]) ?? 0)
            formatChanged(nil)
        case "password": passwordField.stringValue = parts[1]
        case "name": nameField.stringValue = parts[1]
        default: break
        }
    }

    func debugDump() -> String {
        "name=\(nameField.stringValue) format=\(formatPopup.titleOfSelectedItem ?? "") password=\(passwordField.isEnabled ? "enabled" : "disabled")"
    }
}
