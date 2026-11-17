import AppKit
import PDFKit
import UniformTypeIdentifiers

/// Edit (⌘E), Print (⌘P) and Share… for the selection (file-ops §1).
extension FileListViewController {
    /// Selected items that are files, not folders: what Edit and Print take.
    var selectedFiles: [InspectedItem] {
        inspectedSelection.filter { !$0.row.isDir }
    }

    // MARK: Edit

    /// Open each selected file in the editor Settings → Editing names for
    /// its extension, else the default editor, else its usual app. Files
    /// on a server are downloaded first and sent back when saved.
    @objc func editSelection(_ sender: Any?) {
        let files = selectedFiles
        guard !files.isEmpty else { return }
        if isRemote {
            files.forEach { RemoteFiles.shared.open(connection: conn, remotePath: $0.path, window: session.id()) }
            return
        }
        let core = AppEnvironment.shared.core
        var byEditor: [String: [URL]] = [:]
        var plain: [URL] = []
        for file in files {
            if let app = core.editorFor(fileName: file.row.name) {
                byEditor[app, default: []].append(file.url)
            } else {
                plain.append(file.url)
            }
        }
        for (app, urls) in byEditor.sorted(by: { $0.key < $1.key }) { ExternalApps.open(urls, withAppAt: app) }
        if !plain.isEmpty { LocalLaunch.openWithDefaultApps(plain) }
    }

    // MARK: Print

    /// Print the selected documents: PDFs, images and plain text through
    /// RustyFinder's own print panel; anything else is handed to its
    /// application with a "print documents" request, as Finder does.
    @objc func printSelection(_ sender: Any?) {
        let files = selectedFiles
        guard !files.isEmpty, !isRemote, let window = view.window else { return }
        Printing.print(files.map(\.url), in: window)
    }

    // MARK: Share

    /// The system share sheet for the selection, beside the first item.
    @objc func shareSelection(_ sender: Any?) {
        let urls = inspectedSelection.map(\.url)
        guard !urls.isEmpty, !isRemote else { return }
        if ExternalApps.isDryRun {
            ExternalApps.record("share \(urls.map(\.lastPathComponent).joined(separator: ", "))")
            return
        }
        let picker = NSSharingServicePicker(items: urls)
        let first = selection.first ?? 0
        let anchor = layout == .icons
            ? (gridView.item(at: IndexPath(item: first, section: 0))?.view.frame ?? gridView.bounds)
            : tableView.rect(ofRow: content.tableRow(ofItem: first))
        picker.show(relativeTo: anchor, of: primaryView, preferredEdge: .maxY)
    }
}

/// Opening files with their usual apps, recorded instead in scripted checks.
@MainActor
enum LocalLaunch {
    static func openWithDefaultApps(_ urls: [URL]) {
        ExternalApps.record("default <- \(urls.map(\.path).joined(separator: ", "))")
        guard !ExternalApps.isDryRun else { return }
        urls.forEach { NSWorkspace.shared.open($0) }
    }
}

/// The print flow for documents.
@MainActor
enum Printing {
    enum Method: Equatable {
        case pdf, image, text
        /// The document's own application prints it.
        case app(String)
    }

    /// How a file is printed: by its content type.
    static func method(for url: URL) -> Method? {
        let type = (try? url.resourceValues(forKeys: [.contentTypeKey]))?.contentType
            ?? UTType(filenameExtension: url.pathExtension)
        if let type {
            if type.conforms(to: .pdf) { return .pdf }
            if type.conforms(to: .image) { return .image }
            if type.conforms(to: .plainText) || type.conforms(to: .sourceCode) { return .text }
        }
        return NSWorkspace.shared.urlForApplication(toOpen: url).map { .app($0.path) }
    }

    /// Queue of in-app print jobs; each print panel waits for the last.
    private static var queue: [URL] = []
    private static var window: NSWindow?

    static func print(_ urls: [URL], in window: NSWindow) {
        var byApp: [String: [URL]] = [:]
        for url in urls {
            switch method(for: url) {
            case .app(let app): byApp[app, default: []].append(url)
            case nil: NSSound.beep()
            default: queue.append(url)
            }
        }
        for (app, docs) in byApp.sorted(by: { $0.key < $1.key }) { printWithApp(app, docs) }
        self.window = window
        runNext()
    }

    private static func runNext() {
        guard !queue.isEmpty, let window else { return }
        let url = queue.removeFirst()
        guard let method = method(for: url) else { return runNext() }
        ExternalApps.record("print \(method) <- \(url.lastPathComponent)")
        guard !ExternalApps.isDryRun, let operation = operation(for: url, method: method) else { return runNext() }
        operation.jobTitle = url.lastPathComponent
        operation.runModal(for: window, delegate: PrintDone.shared, didRun: #selector(PrintDone.done(_:success:context:)), contextInfo: nil)
    }

    private final class PrintDone: NSObject {
        static let shared = PrintDone()
        @objc func done(_ operation: NSPrintOperation, success: Bool, context: UnsafeMutableRawPointer?) {
            MainActor.assumeIsolated { Printing.runNext() }
        }
    }

    private static func operation(for url: URL, method: Method) -> NSPrintOperation? {
        let info = NSPrintInfo.shared.copy() as? NSPrintInfo ?? NSPrintInfo.shared
        switch method {
        case .pdf:
            return PDFDocument(url: url)?.printOperation(for: info, scalingMode: .pageScaleToFit, autoRotate: true)
        case .image:
            guard let image = NSImage(contentsOf: url) else { return nil }
            info.horizontalPagination = .fit
            info.verticalPagination = .fit
            let page = info.imageablePageBounds.size
            let scale = min(1, page.width / max(image.size.width, 1), page.height / max(image.size.height, 1))
            let view = NSImageView(frame: NSRect(x: 0, y: 0, width: image.size.width * scale, height: image.size.height * scale))
            view.image = image
            view.imageScaling = .scaleProportionallyUpOrDown
            return NSPrintOperation(view: view, printInfo: info)
        case .text:
            guard let text = try? String(contentsOf: url, encoding: .utf8)
                ?? String(contentsOf: url, encoding: .isoLatin1) else { return nil }
            info.horizontalPagination = .fit
            let width = info.paperSize.width - info.leftMargin - info.rightMargin
            let view = NSTextView(frame: NSRect(x: 0, y: 0, width: width, height: 100))
            view.string = text
            view.font = .monospacedSystemFont(ofSize: 10, weight: .regular)
            view.sizeToFit()
            return NSPrintOperation(view: view, printInfo: info)
        case .app:
            return nil
        }
    }

    /// Ask the document's application to print it ("print documents"
    /// Apple event), which shows that app's own print panel.
    private static func printWithApp(_ app: String, _ urls: [URL]) {
        ExternalApps.record("print app \((app as NSString).lastPathComponent) <- \(urls.map(\.lastPathComponent).joined(separator: ", "))")
        guard !ExternalApps.isDryRun else { return }
        let list = NSAppleEventDescriptor.list()
        for (i, url) in urls.enumerated() {
            list.insert(NSAppleEventDescriptor(fileURL: url), at: i + 1)
        }
        let event = NSAppleEventDescriptor(
            eventClass: AEEventClass(kCoreEventClass), eventID: AEEventID(kAEPrintDocuments),
            targetDescriptor: nil, returnID: AEReturnID(kAutoGenerateReturnID), transactionID: AETransactionID(kAnyTransactionID))
        event.setParam(list, forKeyword: keyDirectObject)
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.appleEvent = event
        NSWorkspace.shared.open(urls, withApplicationAt: URL(fileURLWithPath: app), configuration: configuration)
    }
}
