import Foundation

/// Lists one folder through the Rust core and prints it. No UI.
enum SelfTest {
    private final class Listener: WindowListener, @unchecked Sendable {
        let done = DispatchSemaphore(value: 0)
        let lock = NSLock()
        var wanted: String = ""
        var snapshot: SnapshotHandle?
        var info: PaneInfo?
        var failure: String?

        func windowChanged(layout: WindowLayout) {}

        func snapshotReady(tab: UInt64, snapshot: SnapshotHandle, info: PaneInfo) {
            lock.lock()
            defer { lock.unlock() }
            guard info.path == wanted, self.snapshot == nil else { return }
            self.snapshot = snapshot
            self.info = info
            done.signal()
        }

        func loadFailed(tab: UInt64, path: String, error: RustyFinderError) {
            lock.lock()
            defer { lock.unlock() }
            guard failure == nil else { return }
            failure = "\(path): \(error.message)"
            done.signal()
        }
    }

    static func run(path: String) -> Int32 {
        print("rustyfinder-core \(coreVersion())")
        let support = NSTemporaryDirectory() + "rustyfinder-selftest-\(ProcessInfo.processInfo.processIdentifier)"
        defer { try? FileManager.default.removeItem(atPath: support) }
        let app = RustyFinderApp(supportDir: support, home: NSHomeDirectory())
        let listener = Listener()
        let wanted = (path as NSString).standardizingPath
        listener.wanted = wanted
        let window = app.openWindow(listener: listener, restore: nil)
        let tab = window.layout().left.tabs[0].id
        window.navigate(tab: tab, path: wanted)
        guard listener.done.wait(timeout: .now() + 30) == .success else {
            print("selftest: timed out")
            return 2
        }
        if let failure = listener.failure {
            print("selftest: failed: \(failure)")
            return 1
        }
        guard let snapshot = listener.snapshot, let info = listener.info else { return 1 }
        print("path: \(info.path)")
        print("rows: \(snapshot.count()) of \(info.totalCount)")
        for row in snapshot.rows(start: 0, count: 10) {
            print("  \(row.isDir ? "d" : "-") \(row.size)\t\(row.name)")
        }
        return 0
    }
}
