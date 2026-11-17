import AppKit

/// SMB and AFP servers announced on the local network (Bonjour), for the
/// sidebar's Shares group, and the share picker shown when one is opened.
@MainActor
final class ShareBrowser: NSObject, NetServiceBrowserDelegate, NetServiceDelegate {
    static let shared = ShareBrowser()

    struct Server: Equatable {
        let name: String
        /// "smb" or "afp".
        let scheme: String
    }

    private(set) var servers: [Server] = []
    private var browsers: [NetServiceBrowser] = []
    private var found: [NetService] = []
    private var resolving: [NetService: (String?) -> Void] = [:]
    /// The picker on screen, for scripted checks.
    private(set) var lastPicker = "-"
    private var pickerAlert: NSAlert?
    private var pickerPopup: NSPopUpButton?

    /// Start looking. Never during scripted checks: they stay off the
    /// network.
    func start() {
        guard browsers.isEmpty, !ExternalApps.isDryRun else { return }
        for type in ["_smb._tcp.", "_afpovertcp._tcp."] {
            let browser = NetServiceBrowser()
            browser.delegate = self
            browser.searchForServices(ofType: type, inDomain: "local.")
            browsers.append(browser)
        }
    }

    nonisolated func netServiceBrowser(_ browser: NetServiceBrowser, didFind service: NetService, moreComing: Bool) {
        MainActor.assumeIsolated {
            found.append(service)
            let server = Server(name: service.name, scheme: service.type.hasPrefix("_afp") ? "afp" : "smb")
            // A server announcing both protocols is listed once, as SMB.
            if !servers.contains(where: { $0.name == server.name }) { servers.append(server) }
            if !moreComing { changed() }
        }
    }

    nonisolated func netServiceBrowser(_ browser: NetServiceBrowser, didRemove service: NetService, moreComing: Bool) {
        MainActor.assumeIsolated {
            found.removeAll { $0 == service }
            if !found.contains(where: { $0.name == service.name }) { servers.removeAll { $0.name == service.name } }
            if !moreComing { changed() }
        }
    }

    private func changed() {
        NotificationCenter.default.post(name: .rustyfinderSidebarChanged, object: nil)
    }

    // MARK: Opening a server

    /// Resolve a discovered server to its host name.
    private func resolve(_ server: Server, completion: @escaping (String?) -> Void) {
        guard let service = found.first(where: { $0.name == server.name }) else { return completion(nil) }
        resolving[service] = completion
        service.delegate = self
        service.resolve(withTimeout: 10)
    }

    nonisolated func netServiceDidResolveAddress(_ sender: NetService) {
        MainActor.assumeIsolated {
            resolving.removeValue(forKey: sender)?(sender.hostName)
        }
    }

    nonisolated func netService(_ sender: NetService, didNotResolve errorDict: [String: NSNumber]) {
        MainActor.assumeIsolated {
            resolving.removeValue(forKey: sender)?(nil)
        }
    }

    /// Open a discovered server: list its shares and let the user pick one.
    func open(_ server: Server, in window: MainWindowController) {
        resolve(server) { [weak self, weak window] host in
            guard let self, let window else { return }
            let name = host.map { $0.hasSuffix(".") ? String($0.dropLast()) : $0 } ?? "\(server.name).local"
            self.pickShare(host: name, scheme: server.scheme, in: window)
        }
    }

    /// The shares of `host` (guest listing, off the main thread, 15 s at
    /// most), without hidden `$` shares. Empty when the server wants a
    /// login first.
    private func listShares(host: String, completion: @escaping ([String]) -> Void) {
        if let fixture = ProcessInfo.processInfo.environment["RUSTYFINDER_SMBUTIL_OUTPUT"] {
            let text = (try? String(contentsOfFile: fixture, encoding: .utf8)) ?? ""
            return completion(smbShares(output: text))
        }
        DispatchQueue.global(qos: .userInitiated).async {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/smbutil")
            process.arguments = ["view", "-N", "-g", "//guest@\(host)"]
            let pipe = Pipe()
            process.standardOutput = pipe
            process.standardError = Pipe()
            var output = ""
            if (try? process.run()) != nil {
                let deadline = DispatchTime.now() + 15
                DispatchQueue.global().asyncAfter(deadline: deadline) { if process.isRunning { process.terminate() } }
                let data = pipe.fileHandleForReading.readDataToEndOfFile()
                process.waitUntilExit()
                output = String(data: data, encoding: .utf8) ?? ""
            }
            let shares = smbShares(output: output)
            DispatchQueue.main.async { completion(shares) }
        }
    }

    /// "Open Share": pick one of the server's shares, then mount it. A
    /// server that lists nothing to a guest gets the system's own login.
    func pickShare(host: String, scheme: String, in window: MainWindowController) {
        listShares(host: scheme == "smb" ? host : "") { [weak self, weak window] shares in
            guard let self, let window, let sheetWindow = window.window else { return }
            let params = ServerParams(
                protocol: ServerSpecs.proto(scheme) ?? .smb, host: host, port: nil, user: "", password: nil,
                remotePath: nil, passive: true, encoding: nil, acceptInvalidCerts: false, region: nil)
            guard !shares.isEmpty else {
                self.lastPicker = "system"
                window.openShare(params, scheme: scheme, side: nil) { message in
                    if let message { window.showProblem(title: "The server could not be opened.", message: message) }
                }
                return
            }
            let alert = NSAlert()
            alert.messageText = "Open a share on “\(host)”"
            alert.informativeText = "Choose the share to connect to."
            let popup = NSPopUpButton(frame: NSRect(x: 0, y: 0, width: 260, height: 26))
            popup.addItems(withTitles: shares)
            alert.accessoryView = popup
            alert.addButton(withTitle: "Open Share")
            alert.addButton(withTitle: "Cancel")
            self.pickerAlert = alert
            self.pickerPopup = popup
            self.lastPicker = shares.joined(separator: ",")
            alert.beginSheetModal(for: sheetWindow) { [weak self] response in
                self?.pickerAlert = nil
                guard response == .alertFirstButtonReturn, let share = popup.titleOfSelectedItem else { return }
                var chosen = params
                chosen.remotePath = share
                window.openShare(chosen, scheme: scheme, side: nil) { message in
                    if let message { window.showProblem(title: "The share could not be opened.", message: message) }
                }
            }
        }
    }
}
