import AppKit
import UniformTypeIdentifiers

/// Display strings and icons for rows. Everything is cached per extension so
/// scrolling a huge folder never asks the system the same question twice.
@MainActor
final class RowPresenter {
    static let shared = RowPresenter()

    private let dateFormatter: DateFormatter = {
        let f = DateFormatter()
        f.dateStyle = .medium
        f.timeStyle = .short
        f.doesRelativeDateFormatting = true
        return f
    }()

    private let sizeFormatter: ByteCountFormatter = {
        let f = ByteCountFormatter()
        f.countStyle = .file
        return f
    }()

    private var kindByExt: [String: String] = [:]
    private var iconByExt: [String: NSImage] = [:]
    private let iconByPath = NSCache<NSString, NSImage>()
    private lazy var folderIcon = NSWorkspace.shared.icon(for: .folder)
    private lazy var fileIcon = NSWorkspace.shared.icon(for: .data)

    /// Items with no date (some servers) show an empty cell.
    func date(_ ms: Int64?) -> String {
        guard let ms else { return "" }
        dateFormatter.doesRelativeDateFormatting = AppEnvironment.shared.settings.relativeDates
        return dateFormatter.string(from: Date(timeIntervalSince1970: Double(ms) / 1000))
    }

    /// Icon for a folder path (sidebar, tabs, path bar), cached.
    func folderIcon(atPath path: String) -> NSImage {
        if let cached = iconByPath.object(forKey: path as NSString) { return cached }
        let icon = NSWorkspace.shared.icon(forFile: path)
        iconByPath.setObject(icon, forKey: path as NSString)
        return icon
    }

    func size(_ row: Row) -> String {
        row.isDir ? "--" : bytes(row.size)
    }

    func bytes(_ count: UInt64) -> String {
        sizeFormatter.string(fromByteCount: Int64(clamping: count))
    }

    /// Name as shown. On disk a ":" stands for the "/" the user typed
    /// (the macOS convention), so it is shown as "/".
    func displayName(_ row: Row) -> String {
        // A search result's name is a path below the folder searched.
        var name = row.name.contains("/") ? (row.name as NSString).lastPathComponent : row.name
        // Bytes of a server name that are not text travel as U+10FF00 + byte
        // (vendor/russh-sftp); they show as the replacement character.
        let raw: ClosedRange<UInt32> = 0x10FF00...0x10FFFF
        if name.unicodeScalars.contains(where: { raw.contains($0.value) }) {
            name = String(String.UnicodeScalarView(name.unicodeScalars.map { raw.contains($0.value) ? "\u{FFFD}" : $0 }))
        }
        return name.contains(":") ? name.replacingOccurrences(of: ":", with: "/") : name
    }

    /// The name as shown, without the extension when the item's "hide
    /// extension" flag is set (local items only; `path` nil on servers).
    func displayName(_ row: Row, path: String?) -> String {
        let name = displayName(row)
        guard let ext = hiddenExtension(row, path: path) else { return name }
        return String(name.dropLast(ext.count + 1))
    }

    /// The extension the item hides, if its flag says so.
    func hiddenExtension(_ row: Row, path: String?) -> String? {
        guard let path else { return nil }
        let ext = (row.name as NSString).pathExtension
        guard !ext.isEmpty, row.name.count > ext.count + 1,
              (try? URL(fileURLWithPath: path).resourceValues(forKeys: [.hasHiddenExtensionKey]))?.hasHiddenExtension == true
        else { return nil }
        return ext
    }

    /// Where a search result lives, relative to the folder searched
    /// ("" for a result directly in it, and for ordinary listings).
    func location(_ row: Row) -> String {
        row.name.contains("/") ? (row.name as NSString).deletingLastPathComponent : ""
    }

    /// Kind name for files with this extension ("Document" for none).
    func kindName(extension ext: String) -> String {
        if ext.isEmpty { return "Document" }
        if let cached = kindByExt[ext] { return cached }
        let kind = UTType(filenameExtension: ext)?.localizedDescription ?? "\(ext.uppercased()) File"
        kindByExt[ext] = kind
        return kind
    }

    func kind(_ row: Row) -> String {
        if row.isBrokenLink { return "Broken Alias" }
        if row.kind == .symlink { return "Alias" }
        let ext = nameExtension(row)
        if row.isDir {
            guard !ext.isEmpty, let type = UTType(filenameExtension: ext),
                  type.conforms(to: .package) || type.conforms(to: .bundle)
            else { return "Folder" }
            return type.localizedDescription ?? "Package"
        }
        if ext.isEmpty { return "Document" }
        if let cached = kindByExt[ext] { return cached }
        let kind = UTType(filenameExtension: ext)?.localizedDescription ?? "\(ext.uppercased()) File"
        kindByExt[ext] = kind
        return kind
    }

    /// `path` is only evaluated for items whose icon depends on the item
    /// itself (bundles such as apps); plain files and folders use a shared
    /// per-type icon.
    func icon(_ row: Row, path: @autoclosure () -> String?) -> NSImage {
        let ext = nameExtension(row)
        // Aliases, locked items and folders (custom icons and colours) get
        // the system's own icon for that very item, badges included.
        if row.kind == .symlink || row.locked || row.isDir, let path = path() {
            if let cached = iconByPath.object(forKey: path as NSString) { return cached }
            let icon = NSWorkspace.shared.icon(forFile: path)
            iconByPath.setObject(icon, forKey: path as NSString)
            return icon
        }
        if row.isDir {
            guard !ext.isEmpty, let path = path() else { return folderIcon }
            if let cached = iconByPath.object(forKey: path as NSString) { return cached }
            let icon = NSWorkspace.shared.icon(forFile: path)
            iconByPath.setObject(icon, forKey: path as NSString)
            return icon
        }
        if ext.isEmpty { return fileIcon }
        if let cached = iconByExt[ext] { return cached }
        let icon = UTType(filenameExtension: ext).map { NSWorkspace.shared.icon(for: $0) } ?? fileIcon
        iconByExt[ext] = icon
        return icon
    }

    private func nameExtension(_ row: Row) -> String {
        // Rust reports "" for folders, but bundles need their extension here.
        row.isDir ? (row.name as NSString).pathExtension.lowercased() : row.extension
    }
}

extension String {
    /// "/Users/me/Docs" -> "~/Docs" for display.
    var abbreviatingHome: String {
        (self as NSString).abbreviatingWithTildeInPath
    }

    /// Folder name as the system shows it ("Macintosh HD" for "/").
    var folderDisplayName: String {
        FileManager.default.displayName(atPath: self)
    }
}
