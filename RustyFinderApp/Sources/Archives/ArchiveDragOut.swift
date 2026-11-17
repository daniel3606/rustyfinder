import AppKit
import UniformTypeIdentifiers

/// Dragging an item out of an archive: a file promise. Only the dragged
/// entry is extracted, when and where the drop lands (Finder, another app,
/// a pane). An item already at the drop place is never replaced.
final class ArchiveDragOut: NSObject, NSFilePromiseProviderDelegate {
    private let core: RustyFinderApp
    private let conn: UInt64
    private let path: String
    private let name: String
    private let queue = OperationQueue()

    /// What the last drop wrote, or why it failed (scripted checks).
    nonisolated(unsafe) static var lastResult = "-"

    @MainActor
    static func provider(conn: UInt64, path: String, row: Row) -> NSFilePromiseProvider {
        let type: UTType = row.isDir ? .folder : (UTType(filenameExtension: row.extension) ?? .data)
        let delegate = ArchiveDragOut(core: AppEnvironment.shared.core, conn: conn, path: path, name: row.name)
        let provider = NSFilePromiseProvider(fileType: type.identifier, delegate: delegate)
        // The provider holds its delegate weakly; this keeps it alive.
        provider.userInfo = delegate
        return provider
    }

    private init(core: RustyFinderApp, conn: UInt64, path: String, name: String) {
        self.core = core
        self.conn = conn
        self.path = path
        self.name = name
        queue.qualityOfService = .userInitiated
    }

    func filePromiseProvider(_ filePromiseProvider: NSFilePromiseProvider, fileNameForType fileType: String) -> String {
        name
    }

    func operationQueue(for filePromiseProvider: NSFilePromiseProvider) -> OperationQueue {
        queue
    }

    func filePromiseProvider(
        _ filePromiseProvider: NSFilePromiseProvider, writePromiseTo url: URL,
        completionHandler: @escaping (Error?) -> Void
    ) {
        write(to: url, completion: completionHandler)
    }

    /// Extract the item to `url` (which must not exist yet).
    func write(to url: URL, completion: @escaping (Error?) -> Void) {
        let listener = Listener { error in
            ArchiveDragOut.lastResult = error.map { "failed: \($0)" } ?? "wrote \(url.lastPathComponent)"
            completion(error.map { NSError(domain: "RustyFinder", code: 1, userInfo: [NSLocalizedDescriptionKey: $0]) })
        }
        _ = core.fetchCopy(connection: conn, path: path, dest: url.path, listener: listener)
    }

    private final class Listener: FetchListener, @unchecked Sendable {
        let done: (String?) -> Void
        init(done: @escaping (String?) -> Void) { self.done = done }
        func progress(bytes: UInt64, total: UInt64) {}
        func finished(error: String?) { done(error) }
    }
}
