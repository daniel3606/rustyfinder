import AppKit

extension Notification.Name {
    /// An item's iCloud download state changed (downloaded or evicted).
    static let rustyfinderCloudStateChanged = Notification.Name("RustyFinderCloudStateChanged")
}

/// iCloud Drive items on this Mac (cloud.md §2). RustyFinder never signs
/// in or starts syncing; it only asks the system, through FileManager, to
/// download an item now or to remove its local copy. A cloud-only item is
/// recognised from the listing itself (the `SF_DATALESS` flag), so badges
/// cost no extra file system calls.
@MainActor
enum CloudItems {
    /// Scripted checks have no iCloud account: they name items to treat as
    /// cloud-only (`RUSTYFINDER_FAKE_CLOUD_ONLY`) or busy syncing
    /// (`RUSTYFINDER_FAKE_ICLOUD_BUSY`), comma-separated file names.
    private static func fakes(_ key: String) -> Set<String> {
        Set((ProcessInfo.processInfo.environment[key] ?? "").split(separator: ",").map(String.init))
    }
    private static let fakeCloudOnly = fakes("RUSTYFINDER_FAKE_CLOUD_ONLY")
    private static let fakeBusy = fakes("RUSTYFINDER_FAKE_ICLOUD_BUSY")

    /// Downloads asked for and not yet finished, by path.
    private(set) static var downloading: Set<String> = []
    /// The last request or problem (scripted checks).
    private(set) static var lastEvent = "-"

    /// The content lives only in the cloud: reading it would download it.
    static func isCloudOnly(_ row: Row) -> Bool {
        row.cloudOnly || fakeCloudOnly.contains(row.name)
    }

    /// True for an item iCloud keeps (cloud-only or downloaded).
    static func isInCloud(_ path: String) -> Bool {
        if fakeCloudOnly.contains((path as NSString).lastPathComponent) { return true }
        return (try? URL(fileURLWithPath: path).resourceValues(forKeys: [.isUbiquitousItemKey]))?.isUbiquitousItem == true
    }

    /// iCloud is uploading or downloading it right now (edge-cases §4).
    static func isBusy(_ path: String) -> Bool {
        if fakeBusy.contains((path as NSString).lastPathComponent) { return true }
        let keys: Set<URLResourceKey> = [.isUbiquitousItemKey, .ubiquitousItemIsUploadingKey, .ubiquitousItemIsDownloadingKey]
        guard let values = try? URL(fileURLWithPath: path).resourceValues(forKeys: keys), values.isUbiquitousItem == true
        else { return false }
        return values.ubiquitousItemIsUploading == true || values.ubiquitousItemIsDownloading == true
    }

    /// A small cloud after the name in List view, where tag dots go.
    static func badge(after dots: NSAttributedString, cloudOnly: Bool) -> NSAttributedString {
        guard cloudOnly, let symbol = NSImage(systemSymbolName: "icloud.and.arrow.down", accessibilityDescription: "In iCloud only")
        else { return dots }
        let result = NSMutableAttributedString(attributedString: dots)
        if result.length > 0 { result.append(NSAttributedString(string: " ")) }
        let attachment = NSTextAttachment()
        attachment.image = symbol.withSymbolConfiguration(.init(pointSize: 11, weight: .regular))
        result.append(NSAttributedString(attachment: attachment))
        result.addAttribute(.foregroundColor, value: NSColor.secondaryLabelColor, range: NSRange(location: 0, length: result.length))
        return result
    }

    /// An icon with a small cloud in its corner (Icon view).
    static func badged(_ icon: NSImage) -> NSImage {
        guard let cloud = NSImage(systemSymbolName: "icloud.and.arrow.down.fill", accessibilityDescription: "In iCloud only")
        else { return icon }
        let size = NSSize(width: 128, height: 128)
        return NSImage(size: size, flipped: false) { rect in
            icon.draw(in: rect)
            let side = rect.width * 0.38
            let corner = NSRect(x: rect.maxX - side, y: rect.minY, width: side, height: side * 0.8)
            NSColor.windowBackgroundColor.withAlphaComponent(0.85).setFill()
            NSBezierPath(roundedRect: corner, xRadius: side * 0.2, yRadius: side * 0.2).fill()
            let tinted = cloud.withSymbolConfiguration(.init(paletteColors: [.secondaryLabelColor])) ?? cloud
            tinted.draw(in: corner.insetBy(dx: side * 0.12, dy: side * 0.12))
            return true
        }
    }

    // MARK: Download Now / Remove Download

    /// Ask the system to download the items, then watch until they are here.
    static func downloadNow(_ paths: [String], window: NSWindow?) {
        var failed: [(String, String)] = []
        for path in paths {
            do {
                try FileManager.default.startDownloadingUbiquitousItem(at: URL(fileURLWithPath: path))
                downloading.insert(path)
            } catch {
                failed.append((path, error.localizedDescription))
            }
        }
        lastEvent = "download \(paths.map { ($0 as NSString).lastPathComponent }.joined(separator: ","))"
            + (failed.isEmpty ? "" : " failed")
        if !failed.isEmpty { report("Unable to Download", failed, window: window) }
        if !downloading.isEmpty { watch() }
        NotificationCenter.default.post(name: .rustyfinderCloudStateChanged, object: nil)
    }

    /// Remove the local copies, keeping the items in iCloud. Several at
    /// once; whatever fails is listed under "Unable to Remove Download".
    static func removeDownload(_ paths: [String], window: NSWindow?) {
        var failed: [(String, String)] = []
        for path in paths {
            do {
                try FileManager.default.evictUbiquitousItem(at: URL(fileURLWithPath: path))
            } catch {
                failed.append((path, error.localizedDescription))
            }
        }
        lastEvent = "evict \(paths.count)" + (failed.isEmpty ? "" : " failed \(failed.count)")
        if !failed.isEmpty { report("Unable to Remove Download", failed, window: window) }
        NotificationCenter.default.post(name: .rustyfinderCloudStateChanged, object: nil)
    }

    private static func report(_ title: String, _ failed: [(String, String)], window: NSWindow?) {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = title
        alert.informativeText = failed.prefix(5).map { "“\(($0.0 as NSString).lastPathComponent)”: \($0.1)" }
            .joined(separator: "\n") + (failed.count > 5 ? "\n…and \(failed.count - 5) more" : "")
        lastProblem = title
        if let window { alert.beginSheetModal(for: window) } else { alert.runModal() }
    }

    private(set) static var lastProblem = "-"

    /// Poll the downloads until each one is current (or has stopped).
    private static var timer: Timer?

    private static func watch() {
        guard timer == nil else { return }
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { _ in
            MainActor.assumeIsolated { poll() }
        }
    }

    private static func poll() {
        let keys: Set<URLResourceKey> = [.ubiquitousItemDownloadingStatusKey, .ubiquitousItemIsDownloadingKey]
        let done = downloading.filter { path in
            guard let values = try? URL(fileURLWithPath: path).resourceValues(forKeys: keys) else { return true }
            return values.ubiquitousItemDownloadingStatus == .current || values.ubiquitousItemIsDownloading == false
        }
        guard !done.isEmpty else { return }
        downloading.subtract(done)
        if downloading.isEmpty {
            timer?.invalidate()
            timer = nil
        }
        NotificationCenter.default.post(name: .rustyfinderCloudStateChanged, object: nil)
    }

    // MARK: Preview

    /// What the preview shows for a cloud-only item instead of its content:
    /// opening it there would start a download nobody asked for (CL-07).
    static func placeholder(for item: InspectedItem, window: @escaping () -> NSWindow?) -> NSView {
        let icon = NSImageView(image: NSWorkspace.shared.icon(forFile: item.path))
        icon.widthAnchor.constraint(equalToConstant: 96).isActive = true
        icon.heightAnchor.constraint(equalToConstant: 96).isActive = true
        let cloud = NSImageView(image: NSImage(systemSymbolName: "icloud", accessibilityDescription: "In iCloud")!)
        cloud.contentTintColor = .secondaryLabelColor
        let busy = downloading.contains(item.path)
        let text = NSTextField(wrappingLabelWithString: busy
            ? "Downloading from iCloud…"
            : "This item is stored in iCloud. Download it to preview it.")
        text.alignment = .center
        text.textColor = .secondaryLabelColor
        let button = DownloadButton(title: "Download Now", target: nil, action: nil)
        button.target = button
        button.action = #selector(DownloadButton.pressed(_:))
        button.onPress = { downloadNow([item.path], window: window()) }
        button.isEnabled = !busy
        let stack = NSStackView(views: [icon, cloud, text, button])
        stack.orientation = .vertical
        stack.spacing = 10
        stack.edgeInsets = NSEdgeInsets(top: 20, left: 20, bottom: 20, right: 20)
        return stack
    }

    /// A button that runs a closure.
    private final class DownloadButton: NSButton {
        var onPress: (() -> Void)?

        @objc func pressed(_ sender: Any?) {
            MainActor.assumeIsolated { onPress?() }
        }
    }
}
