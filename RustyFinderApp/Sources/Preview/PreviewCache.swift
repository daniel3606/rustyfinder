import AppKit

/// Local copies of items on servers and inside archives, for the preview
/// pane, Info, Quick Look, Icon view thumbnails and drags out of archives.
/// Copies live in a temporary folder that is removed at quit; an item that
/// changes (another size or date) gets a fresh copy.
@MainActor
final class PreviewCache {
    static let shared = PreviewCache()

    /// Files on a server larger than this are not downloaded for a preview
    /// until the user asks (archives are on this Mac: no limit).
    static let autoDownloadLimit: UInt64 = 10 * 1_000_000

    struct Key: Hashable {
        let conn: UInt64
        let path: String
        let size: UInt64
        let modified: Int64?
    }

    private let root = URL(fileURLWithPath: NSTemporaryDirectory())
        .appendingPathComponent("RustyFinder-preview-\(ProcessInfo.processInfo.processIdentifier)")
    private var ready: [Key: String] = [:]
    private var running: [Key: (job: FetchJob, waiting: [(String?) -> Void])] = [:]
    /// Fraction done of each download in flight.
    private(set) var progress: [Key: Double] = [:]
    /// What happened last (scripted checks).
    private(set) var lastEvent = "-"

    static func key(_ item: InspectedItem) -> Key {
        Key(conn: item.conn, path: item.path, size: item.row.size, modified: item.row.modifiedMs)
    }

    /// The local file to preview: the item itself on this Mac, else its
    /// copy if one has been made.
    func local(for item: InspectedItem) -> String? {
        item.conn == 0 ? item.path : ready[Self.key(item)]
    }

    func isFetching(_ item: InspectedItem) -> Bool { running[Self.key(item)] != nil }

    /// Too big to download without asking (servers only).
    static func needsAsking(_ item: InspectedItem) -> Bool {
        item.conn != 0 && !ArchiveCenter.shared.isArchive(item.conn) && !item.row.isDir
            && item.row.size > autoDownloadLimit
    }

    /// Make a local copy (a whole folder for a folder) and hand back its
    /// path, or nil if it could not be made. Asking twice for the same
    /// item while it downloads waits for the one copy.
    func fetch(_ item: InspectedItem, completion: @escaping (String?) -> Void) {
        let key = Self.key(item)
        if let local = local(for: item), FileManager.default.fileExists(atPath: local) { return completion(local) }
        if running[key] != nil {
            running[key]?.waiting.append(completion)
            return
        }
        let folder = root.appendingPathComponent(UUID().uuidString)
        guard (try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)) != nil else {
            return completion(nil)
        }
        let name = (item.path as NSString).lastPathComponent
        let dest = folder.appendingPathComponent(name.isEmpty ? "item" : name).path
        let listener = Listener { [weak self] done, total, finished in
            guard let self else { return }
            if let finished {
                self.finish(key, local: finished.error == nil ? dest : nil, error: finished.error)
            } else if total > 0 {
                self.progress[key] = Double(done) / Double(total)
                NotificationCenter.default.post(name: .rustyfinderPreviewFetchProgress, object: nil)
            }
        }
        lastEvent = "fetching \(name)"
        progress[key] = 0
        let job = AppEnvironment.shared.core.fetchCopy(connection: item.conn, path: item.path, dest: dest, listener: listener)
        running[key] = (job, [completion])
    }

    private func finish(_ key: Key, local: String?, error: String?) {
        let waiting = running.removeValue(forKey: key)?.waiting ?? []
        progress[key] = nil
        if let local { ready[key] = local }
        lastEvent = local != nil ? "fetched \((key.path as NSString).lastPathComponent)" : "failed \(error ?? "")"
        waiting.forEach { $0(local) }
        NotificationCenter.default.post(name: .rustyfinderPreviewFetchProgress, object: nil)
    }

    /// Drop the copy of an item (Refresh): the next preview fetches anew.
    func forget(_ item: InspectedItem) {
        let key = Self.key(item)
        running.removeValue(forKey: key)?.job.cancel()
        progress[key] = nil
        if let local = ready.removeValue(forKey: key) {
            try? FileManager.default.removeItem(atPath: (local as NSString).deletingLastPathComponent)
        }
        lastEvent = "forgot \((item.path as NSString).lastPathComponent)"
    }

    /// Copies are deleted when the app quits.
    func cleanUp() {
        running.values.forEach { $0.job.cancel() }
        running.removeAll()
        ready.removeAll()
        try? FileManager.default.removeItem(at: root)
    }

    private final class Listener: FetchListener, @unchecked Sendable {
        struct Finished { let error: String? }
        let update: @MainActor (UInt64, UInt64, Finished?) -> Void

        init(update: @escaping @MainActor (UInt64, UInt64, Finished?) -> Void) {
            self.update = update
        }

        func progress(bytes: UInt64, total: UInt64) {
            DispatchQueue.main.async { MainActor.assumeIsolated { self.update(bytes, total, nil) } }
        }

        func finished(error: String?) {
            DispatchQueue.main.async { MainActor.assumeIsolated { self.update(0, 0, Finished(error: error)) } }
        }
    }
}

extension Notification.Name {
    /// A preview download moved on or ended.
    static let rustyfinderPreviewFetchProgress = Notification.Name("RustyFinderPreviewFetchProgress")
}
