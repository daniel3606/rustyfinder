import AppKit
import Security

extension Notification.Name {
    /// A remote connection opened or closed.
    static let rustyfinderConnectionsChanged = Notification.Name("RustyFinderConnectionsChanged")
}

/// Passwords live in the macOS keychain, never in RustyFinder's own files.
enum Keychain {
    private static let service = "RustyFinder"

    /// Account name for a connection: "sftp://bob@example.org:2222".
    static func account(host: String, user: String, port: UInt16?, scheme: String = "sftp") -> String {
        var text = "\(scheme)://"
        if !user.isEmpty { text += "\(user)@" }
        text += host
        if let port { text += ":\(port)" }
        return text
    }

    static func password(for account: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
              let data = result as? Data
        else { return nil }
        return String(data: data, encoding: .utf8)
    }

    @discardableResult
    static func store(_ password: String, for account: String) -> Bool {
        let base: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        let data = Data(password.utf8)
        let status = SecItemUpdate(base as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if status == errSecItemNotFound {
            var add = base
            add[kSecValueData as String] = data
            return SecItemAdd(add as CFDictionary, nil) == errSecSuccess
        }
        return status == errSecSuccess
    }
}

/// Opens and closes remote connections, and answers ssh's questions with
/// dialogs.
@MainActor
final class ConnectionCenter {
    static let shared = ConnectionCenter()

    /// Relays one connection attempt's callbacks. `prompt` runs on a Rust
    /// thread and blocks it until the user has answered on the main thread.
    private final class Listener: ConnectListener, @unchecked Sendable {
        let window: NSWindow?
        let done: (Result<ConnectionRecord, ConnectFailure>) -> Void
        /// Scripted checks answer prompts without a dialog.
        let scriptedAnswers: [PromptType: String]

        init(window: NSWindow?, scriptedAnswers: [PromptType: String],
             done: @escaping (Result<ConnectionRecord, ConnectFailure>) -> Void) {
            self.window = window
            self.scriptedAnswers = scriptedAnswers
            self.done = done
        }

        func prompt(text: String, kind: PromptType) -> String? {
            if let scripted = scriptedAnswers[kind] {
                DispatchQueue.main.async { ConnectionCenter.shared.lastPrompt = "\(kind): \(text)" }
                return scripted
            }
            let semaphore = DispatchSemaphore(value: 0)
            var answer: String?
            DispatchQueue.main.async {
                ConnectionCenter.shared.lastPrompt = "\(kind): \(text)"
                PromptDialog.ask(text: text, kind: kind, window: self.window) { reply in
                    answer = reply
                    semaphore.signal()
                }
            }
            semaphore.wait()
            return answer
        }

        func connected(connection: ConnectionRecord) {
            DispatchQueue.main.async { self.done(.success(connection)) }
        }

        func failed(message: String, hostKeyChanged: Bool) {
            DispatchQueue.main.async { self.done(.failure(ConnectFailure(message: message, hostKeyChanged: hostKeyChanged))) }
        }
    }

    struct ConnectFailure: Error {
        let message: String
        let hostKeyChanged: Bool
    }

    /// The last question ssh asked (for scripted checks).
    private(set) var lastPrompt = "-"

    private var core: RustyFinderApp { AppEnvironment.shared.core }

    /// Open servers (archives being browsed are connections too, but not
    /// ones to list as servers).
    var connections: [ConnectionRecord] { core.connections().filter { !$0.archive } }

    /// Connect over SFTP. `completion` runs on the main thread.
    func connectSFTP(
        host: String, user: String, port: UInt16?, password: String?, remotePath: String?,
        encoding: String? = nil, acceptChangedHostKey: String? = nil,
        window: NSWindow?, scriptedAnswers: [PromptType: String] = [:],
        completion: @escaping (Result<ConnectionRecord, ConnectFailure>) -> Void
    ) {
        let params = SftpConnectParams(
            host: host, user: user, port: port, password: password, keyFile: nil,
            remotePath: remotePath,
            // ssh runs this very binary to ask its questions; see main().
            askpassProgram: Bundle.main.executablePath ?? CommandLine.arguments[0],
            encoding: encoding, acceptChangedHostKey: acceptChangedHostKey)
        let listener = Listener(window: window, scriptedAnswers: scriptedAnswers) { [core] result in
            if case .success(let connection) = result {
                // Known by its URL, a server's tabs can reconnect after a
                // relaunch and its Recent Folders are its own.
                core.setConnectionUrl(connection: connection.id, url: Keychain.account(host: host, user: user, port: port))
                NotificationCenter.default.post(name: .rustyfinderConnectionsChanged, object: nil)
            }
            completion(result)
        }
        core.connectSftp(params: params, listener: listener)
    }

    /// The last question about a failed connection (for scripted checks).
    private(set) var lastDecision = "-"

    /// Connect to a server of any supported kind. SFTP goes through ssh;
    /// the others are spoken by RustyFinder itself. Three failures are
    /// questions rather than ends: a changed host key, a certificate the
    /// system cannot verify, and a password that would go out unencrypted.
    /// The user decides (Cancel is the default) and the attempt is made
    /// once more accordingly.
    func connect(
        _ params: ServerParams, window: NSWindow?, scriptedAnswers: [PromptType: String] = [:],
        completion: @escaping (Result<ConnectionRecord, ConnectFailure>) -> Void
    ) {
        attempt(params, acceptChangedHostKey: nil, window: window, scriptedAnswers: scriptedAnswers) { [weak self] result in
            guard let self, case .failure(let failure) = result,
                  let retry = self.decision(for: failure, params: params) else { return completion(result) }
            ConnectDecisionAlert.ask(retry.question, window: window) { choice in
                self.lastDecision = "\(retry.question.title) -> \(choice ?? "cancel")"
                guard let choice, let next = retry.next(choice) else { return completion(result) }
                self.attempt(next.0, acceptChangedHostKey: next.1, window: window, scriptedAnswers: scriptedAnswers,
                             completion: completion)
            }
        }
    }

    /// What to ask about a failure, and how each answer changes the next try.
    private func decision(for failure: ConnectFailure, params: ServerParams)
        -> (question: ConnectDecisionAlert.Question, next: (String) -> (ServerParams, String?)?)?
    {
        let host = params.host
        if failure.hostKeyChanged, let fingerprint = changedHostKeyFingerprint(message: failure.message) {
            let question = ConnectDecisionAlert.Question(
                title: "The identity of “\(host)” has changed",
                text: "The server presents a different key than the one on record. Someone may be intercepting "
                    + "the connection, or the server was reinstalled.\n\nNew key: \(fingerprint)",
                buttons: ["Cancel", "Connect Anyway"], critical: true)
            return (question, { $0 == "Connect Anyway" ? (params, fingerprint) : nil })
        }
        if let fingerprint = certificateFingerprint(message: failure.message) {
            let question = ConnectDecisionAlert.Question(
                title: "The certificate of “\(host)” can’t be verified",
                text: "It may not be the server it claims to be.\n\n\(failure.message)",
                buttons: ["Cancel", "Continue", "Always Trust"], critical: true)
            return (question, { choice in
                var next = params
                switch choice {
                case "Continue": next.trustedFingerprint = fingerprint
                case "Always Trust":
                    // Kept only if the server still presents what was shown.
                    guard AppEnvironment.shared.core.trustServerCertificate(
                        protocol: params.protocol, implicitTls: params.implicitTls, host: host, port: params.port,
                        fingerprint: fingerprint) else { return nil }
                default: return nil
                }
                return (next, nil)
            })
        }
        if isInsecurePasswordWarning(message: failure.message) {
            let question = ConnectDecisionAlert.Question(
                title: "Send the password unencrypted?",
                text: "“\(host)” asks for the password over plain HTTP. Anyone on the network could read it. "
                    + "Use WebDAV HTTPS if the server offers it.",
                buttons: ["Cancel", "Send Anyway"], critical: false)
            return (question, { choice in
                guard choice == "Send Anyway" else { return nil }
                var next = params
                next.allowInsecureBasic = true
                return (next, nil)
            })
        }
        return nil
    }

    private func attempt(
        _ params: ServerParams, acceptChangedHostKey: String?, window: NSWindow?, scriptedAnswers: [PromptType: String],
        completion: @escaping (Result<ConnectionRecord, ConnectFailure>) -> Void
    ) {
        if params.protocol == .sftp {
            connectSFTP(
                host: params.host, user: params.user, port: params.port, password: params.password,
                remotePath: params.remotePath, encoding: params.encoding, acceptChangedHostKey: acceptChangedHostKey,
                window: window, scriptedAnswers: scriptedAnswers, completion: completion)
            return
        }
        let listener = Listener(window: window, scriptedAnswers: scriptedAnswers) { [core] result in
            if case .success(let connection) = result {
                let url = Keychain.account(host: params.host, user: params.user, port: params.port, scheme: ServerSpecs.scheme(params.protocol))
                core.setConnectionUrl(connection: connection.id, url: url)
                NotificationCenter.default.post(name: .rustyfinderConnectionsChanged, object: nil)
            }
            completion(result)
        }
        core.connectServer(params: params, listener: listener)
    }

    /// Close a connection: its tabs go back to the starting directory and
    /// the ssh process ends.
    func disconnect(_ connection: UInt64) {
        guard core.disconnect(connection: connection) else { return }
        RemoteFiles.shared.connectionClosed(connection)
        (NSApp.delegate as? AppDelegate)?.windowControllers.forEach {
            $0.session.connectionClosed(conn: connection)
        }
        NotificationCenter.default.post(name: .rustyfinderConnectionsChanged, object: nil)
    }
}

/// The question after a connection failed for a reason the user may
/// choose to accept. Cancel is the first button, so Return cancels.
@MainActor
enum ConnectDecisionAlert {
    struct Question {
        let title: String
        let text: String
        let buttons: [String]
        let critical: Bool
    }

    /// `completion` gets the chosen button's title, or nil for Cancel.
    static func ask(_ question: Question, window: NSWindow?, completion: @escaping (String?) -> Void) {
        let alert = NSAlert()
        alert.alertStyle = question.critical ? .critical : .warning
        alert.messageText = question.title
        alert.informativeText = question.text
        for title in question.buttons { alert.addButton(withTitle: title) }
        alert.buttons.first?.keyEquivalent = "\r"
        alert.buttons.dropFirst().forEach { $0.keyEquivalent = "" }
        let done = { (response: NSApplication.ModalResponse) in
            let index = response.rawValue - NSApplication.ModalResponse.alertFirstButtonReturn.rawValue
            completion(index > 0 && index < question.buttons.count ? question.buttons[index] : nil)
        }
        if let host = window?.attachedSheet ?? window {
            alert.beginSheetModal(for: host, completionHandler: done)
        } else {
            done(alert.runModal())
        }
    }
}

/// The dialogs for ssh's questions.
@MainActor
enum PromptDialog {
    static func ask(text: String, kind: PromptType, window: NSWindow?, completion: @escaping (String?) -> Void) {
        let alert = NSAlert()
        let question = text.trimmingCharacters(in: .whitespacesAndNewlines)
        switch kind {
        case .hostKey:
            // An unknown server: show the fingerprint, let the user decide.
            alert.alertStyle = .warning
            alert.messageText = "This server is not known yet."
            alert.informativeText = question
            alert.addButton(withTitle: "Cancel")
            alert.addButton(withTitle: "Accept and Connect")
            present(alert, on: window) { response in
                completion(response == .alertSecondButtonReturn ? "yes" : "no")
            }
        case .password, .passphrase, .other:
            alert.messageText = kind == .passphrase ? "Enter the passphrase" : (kind == .password ? "Enter the password" : "The server asks")
            alert.informativeText = question
            // Passwords, passphrases and one-time codes are all hidden.
            let field = NSSecureTextField(frame: NSRect(x: 0, y: 0, width: 280, height: 24))
            alert.accessoryView = field
            alert.window.initialFirstResponder = field
            alert.addButton(withTitle: "OK")
            alert.addButton(withTitle: "Cancel")
            present(alert, on: window) { response in
                completion(response == .alertFirstButtonReturn ? field.stringValue : nil)
            }
        }
    }

    private static func present(_ alert: NSAlert, on window: NSWindow?, completion: @escaping (NSApplication.ModalResponse) -> Void) {
        // The Connect sheet may be up; a second sheet goes on top of it.
        if let host = window?.attachedSheet ?? window {
            alert.beginSheetModal(for: host, completionHandler: completion)
        } else {
            completion(alert.runModal())
        }
    }
}

/// Remote files opened on this Mac: each is downloaded to a temporary
/// folder, opened, and watched. Saving it uploads it back to its server.
@MainActor
final class RemoteFiles {
    static let shared = RemoteFiles()

    private struct Edited {
        let connection: UInt64
        let remotePath: String
        let localPath: String
        let window: UInt64
        var source: DispatchSourceFileSystemObject?
    }

    private let root = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("RustyFinder-remote-\(ProcessInfo.processInfo.processIdentifier)")
    private var edited: [String: Edited] = [:]
    /// Downloads in flight: activity id -> what to do when it lands, and
    /// the app to open it with (nil: the default one).
    private var pending: [UInt64: (Edited, Bool)] = [:]
    private var openWith: [UInt64: URL] = [:]
    /// Uploads of edited files in flight: activity id -> the file.
    private var uploads: [UInt64: Edited] = [:]
    /// Other downloads waiting for their outcome (drag and drop).
    private var waiters: [UInt64: (FinishRecord) -> Void] = [:]
    /// The last "Remote Editing Error" shown, and what was opened (checks).
    private(set) var lastEditingError = "-"
    var lastOpened = "-"
    var lastCopied = "-"
    var lastDrop = "-"

    /// Download `remotePath` and open it, with `app` or the default app.
    /// With `watch`, saving the local copy uploads it back.
    func open(connection: UInt64, remotePath: String, window: UInt64, watch: Bool = true, app: URL? = nil) {
        let name = (remotePath as NSString).lastPathComponent
        let folder = root.appendingPathComponent(UUID().uuidString)
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        } catch {
            return
        }
        let local = folder.appendingPathComponent(name).path
        let item = Edited(connection: connection, remotePath: remotePath, localPath: local, window: window)
        let op = FileOp.transfer(mode: .copy, sources: [remotePath], destDir: folder.path, renameTo: nil)
        if let id = AppEnvironment.shared.core.submitBetween(window: window, op: op, src: connection, dst: 0) {
            pending[id] = (item, watch)
            if let app { openWith[id] = app }
        }
    }

    /// Download `remotePath` into the folder `into` (as `name`) and report
    /// how it went. Used for remote items dropped on other apps.
    func download(
        connection: UInt64, remotePath: String, into folder: String, name: String, window: UInt64,
        completion: @escaping (FinishRecord?) -> Void
    ) {
        let op = FileOp.transfer(mode: .copy, sources: [remotePath], destDir: folder, renameTo: name)
        guard let id = AppEnvironment.shared.core.submitBetween(window: window, op: op, src: connection, dst: 0) else {
            return completion(nil)
        }
        waiters[id] = completion
    }

    /// Downloads whose only purpose is a local copy: activity id -> the
    /// local path and who wants it.
    private var fetches: [UInt64: (String, (String?) -> Void)] = [:]

    /// Download `remotePath` to a temporary copy and hand back its local
    /// path, or nil if the download failed.
    func fetch(connection: UInt64, remotePath: String, window: UInt64, completion: @escaping (String?) -> Void) {
        let folder = root.appendingPathComponent(UUID().uuidString)
        guard (try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)) != nil else {
            return completion(nil)
        }
        let local = folder.appendingPathComponent((remotePath as NSString).lastPathComponent).path
        let op = FileOp.transfer(mode: .copy, sources: [remotePath], destDir: folder.path, renameTo: nil)
        guard let id = AppEnvironment.shared.core.submitBetween(window: window, op: op, src: connection, dst: 0) else {
            return completion(nil)
        }
        fetches[id] = (local, completion)
    }

    /// Called for every finished activity; picks out our downloads.
    func activityFinished(_ report: FinishRecord) {
        if let (local, completion) = fetches.removeValue(forKey: report.id) {
            completion(report.result == .success ? local : nil)
            return
        }
        if let waiter = waiters.removeValue(forKey: report.id) {
            waiter(report)
            return
        }
        if let item = uploads.removeValue(forKey: report.id) {
            if report.result != .success { uploadFailed(item, message: report.message ?? report.title) }
            return
        }
        let app = openWith.removeValue(forKey: report.id)
        guard let (item, watch) = pending.removeValue(forKey: report.id), report.result == .success else { return }
        let file = URL(fileURLWithPath: item.localPath)
        // Scripted checks record what would open instead of launching apps.
        if DebugSnapshot.keys == nil {
            if let app {
                NSWorkspace.shared.open([file], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration())
            } else {
                NSWorkspace.shared.open(file)
            }
        }
        lastOpened = "\((item.remotePath as NSString).lastPathComponent) with \(app.map { ($0.path as NSString).lastPathComponent } ?? "default app")"
        if watch { startWatching(item) }
    }

    /// The edited file could not go back to its server: say so, and offer
    /// to try again (file-ops §7).
    private func uploadFailed(_ item: Edited, message: String) {
        let name = (item.remotePath as NSString).lastPathComponent
        lastEditingError = "\(name): \(message)"
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "Remote Editing Error"
        alert.informativeText = "Your changes to “\(name)” could not be uploaded to the server. \(message)"
        alert.addButton(withTitle: "Retry")
        alert.addButton(withTitle: "Cancel")
        let window = (NSApp.delegate as? AppDelegate)?.windowControllers.first { $0.session.id() == item.window }?.window
        let retry = { [weak self] (response: NSApplication.ModalResponse) in
            if response == .alertFirstButtonReturn { self?.upload(item) }
        }
        if let window {
            alert.beginSheetModal(for: window, completionHandler: retry)
        } else {
            retry(alert.runModal())
        }
    }

    func debugDump() -> [String] {
        [
            "remote.opened: \(lastOpened)", "remote.editerror: \(lastEditingError)",
            "remote.copied: \(lastCopied.replacingOccurrences(of: "\n", with: " | "))", "remote.drop: \(lastDrop)",
        ]
    }

    private func startWatching(_ item: Edited) {
        var item = item
        let fd = Darwin.open(item.localPath, O_EVTONLY)
        guard fd >= 0 else { return }
        // Editors save either in place (write) or by replacing the file
        // (delete/rename); both mean "saved".
        let source = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd, eventMask: [.write, .delete, .rename], queue: .main)
        source.setEventHandler { [weak self] in
            guard let self, let current = self.edited[item.localPath] else { return }
            let replaced = !source.data.intersection([.delete, .rename]).isEmpty
            // Wait a moment: a save is often several writes in a row.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                self.upload(current)
                if replaced {
                    // The watched file was swapped for a new one: watch that.
                    current.source?.cancel()
                    self.edited[item.localPath] = nil
                    self.startWatching(current)
                }
            }
        }
        source.setCancelHandler { close(fd) }
        source.resume()
        item.source = source
        edited[item.localPath] = item
    }

    private func upload(_ item: Edited) {
        guard FileManager.default.fileExists(atPath: item.localPath) else { return }
        if let id = AppEnvironment.shared.core.submitBetween(
            window: item.window, op: .put(source: item.localPath, target: item.remotePath),
            src: 0, dst: item.connection)
        {
            uploads[id] = item
        }
    }

    /// Save into a watched copy, as an editor would (scripted checks).
    func debugEdit(name: String, text: String) {
        guard let item = edited.values.first(where: { ($0.remotePath as NSString).lastPathComponent == name }) else { return }
        try? text.write(toFile: item.localPath, atomically: false, encoding: .utf8)
    }

    /// Stop watching files of a closed connection.
    func connectionClosed(_ connection: UInt64) {
        for (path, item) in edited where item.connection == connection {
            item.source?.cancel()
            edited[path] = nil
        }
    }

    /// Temporary copies are deleted when the app quits.
    func cleanUp() {
        edited.values.forEach { $0.source?.cancel() }
        edited.removeAll()
        try? FileManager.default.removeItem(at: root)
    }

    var debugWatched: [String] { edited.values.map { ($0.remotePath as NSString).lastPathComponent }.sorted() }
}
