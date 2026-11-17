import AppKit

/// Archives opened as folders. Opening one registers it in the core as a
/// connection, so panes browse it and the copy engine reads from it.
@MainActor
final class ArchiveCenter {
    static let shared = ArchiveCenter()
    private var core: RustyFinderApp { AppEnvironment.shared.core }

    /// Entries each open archive left out as unsafe, for the extract log.
    private var blocked: [UInt64: [String]] = [:]
    /// Extractions in flight: activity id -> the archive they read.
    private var extracting: [UInt64: UInt64] = [:]
    /// Passwords the scripted checks answer with instead of the dialog.
    var scriptedPasswords: [String] = []
    /// What the last password question said (scripted checks).
    private(set) var lastPrompt = "-"

    /// Connections that are archives (ids are never reused).
    private var archiveIds: Set<UInt64> = []

    func isArchive(_ connection: UInt64) -> Bool {
        connection != 0 && archiveIds.contains(connection)
    }

    private final class Listener: ArchiveListener, @unchecked Sendable {
        let done: (Result<(ConnectionRecord, [String]), Failure>) -> Void
        struct Failure: Error {
            let reason: ArchiveFailure
            let message: String
        }

        init(done: @escaping (Result<(ConnectionRecord, [String]), Failure>) -> Void) {
            self.done = done
        }

        func opened(connection: ConnectionRecord, blocked: [String]) {
            DispatchQueue.main.async { self.done(.success((connection, blocked))) }
        }

        func failed(reason: ArchiveFailure, message: String) {
            DispatchQueue.main.async { self.done(.failure(Failure(reason: reason, message: message))) }
        }
    }

    /// Open the archive at `path` on `connection` as a folder. A password
    /// is asked for when needed, again after a wrong one; Cancel gives up.
    /// An archive on a server is downloaded to a temporary copy first.
    /// `completion` gets the archive's connection, or nil.
    func open(
        connection: UInt64, path: String, window: NSWindow?, windowId: UInt64,
        completion: @escaping (ConnectionRecord?) -> Void
    ) {
        if connection == 0 {
            open(local: path, connection: 0, path: path, password: nil, window: window, completion: completion)
            return
        }
        RemoteFiles.shared.fetch(connection: connection, remotePath: path, window: windowId) { [weak self] local in
            guard let local else { return completion(nil) }
            self?.open(local: local, connection: connection, path: path, password: nil, window: window, completion: completion)
        }
    }

    private func open(
        local: String, connection: UInt64, path: String, password: String?, window: NSWindow?,
        completion: @escaping (ConnectionRecord?) -> Void
    ) {
        let name = (path as NSString).lastPathComponent
        let listener = Listener { [weak self] result in
            guard let self else { return }
            switch result {
            case .success(let (record, blocked)):
                self.archiveIds.insert(record.id)
                if !blocked.isEmpty { self.blocked[record.id] = blocked }
                completion(record)
            case .failure(let failure):
                switch failure.reason {
                case .passwordRequired, .wrongPassword:
                    let again = failure.reason == .wrongPassword ? "The password is not correct. " : ""
                    let question = "\(again)Enter password for the archive “\(name)”."
                    self.lastPrompt = question
                    let retry: (String?) -> Void = { answer in
                        guard let answer, !answer.isEmpty else { return completion(nil) }
                        self.open(local: local, connection: connection, path: path, password: answer, window: window, completion: completion)
                    }
                    if !self.scriptedPasswords.isEmpty {
                        retry(self.scriptedPasswords.removeFirst())
                    } else if ProcessInfo.processInfo.environment["RUSTYFINDER_SUPPORT_DIR"] != nil, DebugSnapshot.keys != nil {
                        // A scripted run with no answer prepared: Cancel.
                        retry(nil)
                    } else {
                        PromptDialog.ask(text: question, kind: .password, window: window, completion: retry)
                    }
                case .unrecognized, .other:
                    let alert = NSAlert()
                    alert.alertStyle = .warning
                    alert.messageText = "“\(name)” can’t be opened."
                    alert.informativeText = failure.message
                    self.lastProblem = "\(alert.messageText) \(alert.informativeText)"
                    if let window, window.attachedSheet == nil {
                        alert.beginSheetModal(for: window)
                    }
                    completion(nil)
                }
            }
        }
        core.openArchive(
            localFile: local, originConnection: connection, originPath: path, password: password, listener: listener)
    }

    /// The last "can't be opened" message (scripted checks).
    private(set) var lastProblem = "-"

    /// Extract the archive at `path` on `connection` into `destDir` on
    /// `destConnection`, as an activity.
    func extract(
        connection: UInt64, path: String, destConnection: UInt64, destDir: String,
        window: NSWindow?, windowId: UInt64
    ) {
        open(connection: connection, path: path, window: window, windowId: windowId) { [weak self] record in
            guard let self, let record else { return }
            let id = self.core.extract(
                window: windowId, archiveConnection: record.id, destConnection: destConnection,
                destDir: destDir, blocked: self.blocked[record.id] ?? [])
            if let id { self.extracting[id] = record.id }
        }
    }

    /// Called for every finished activity: an archive that was opened only
    /// to extract it is closed again, unless a tab is browsing it.
    func activityFinished(_ report: FinishRecord) {
        guard let archive = extracting.removeValue(forKey: report.id), !extracting.values.contains(archive) else { return }
        let browsing = NSApp.windows.compactMap { $0.windowController as? MainWindowController }
            .contains { $0.shows(connection: archive) }
        if !browsing { close(archive) }
    }

    func close(_ connection: UInt64) {
        blocked[connection] = nil
        _ = core.disconnect(connection: connection)
    }

    /// Unpacked copies are deleted when the app quits.
    func cleanUp() {
        core.cleanUpArchives()
    }
}
