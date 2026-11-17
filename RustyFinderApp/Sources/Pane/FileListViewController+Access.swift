import AppKit

/// iCloud items, aliases and folders this user can't read (cloud.md §2,
/// edge-cases §3 and §4).
extension FileListViewController {
    // MARK: iCloud Drive

    @objc func downloadNow(_ sender: Any?) {
        let paths = inspectedSelection.filter { CloudItems.isCloudOnly($0.row) }.map(\.path)
        guard !paths.isEmpty, !isRemote else { return }
        CloudItems.downloadNow(paths, window: view.window)
    }

    /// Keep the selected items in iCloud only (several at once).
    @objc func removeDownload(_ sender: Any?) {
        let paths = inspectedSelection.filter { !CloudItems.isCloudOnly($0.row) }.map(\.path)
        guard !paths.isEmpty, !isRemote else { return }
        CloudItems.removeDownload(paths, window: view.window)
    }

    /// Names of the visible rows whose name cell shows the cloud badge
    /// (scripted checks read what is drawn, not what should be).
    var debugCloudBadges: [String] {
        guard let column = tableView.tableColumns.firstIndex(where: { $0.identifier.rawValue == "name" }) else { return [] }
        return (0..<min(content.count, 50)).compactMap { index in
            let tableRow = content.tableRow(ofItem: index)
            guard let cell = tableView.view(atColumn: column, row: tableRow, makeIfNecessary: true) else { return nil }
            let fields = cell.subviews.compactMap { $0 as? NSTextField }.filter { $0 !== (cell as? NSTableCellView)?.textField }
            let badged = fields.contains { field in
                var found = false
                field.attributedStringValue.enumerateAttribute(.attachment, in: NSRange(location: 0, length: field.attributedStringValue.length)) { value, _, _ in
                    if value != nil { found = true }
                }
                return found
            }
            return badged ? content.row(at: index)?.name : nil
        }
    }

    // MARK: Aliases

    /// Open a local file. A Finder alias is resolved first (off the main
    /// thread: resolving may wait for a volume): an alias to a folder shows
    /// that folder here, one to a file opens the file, and one whose
    /// original is gone says it can't be resolved.
    func openLocalFile(_ path: String) {
        let url = URL(fileURLWithPath: path)
        let values = try? url.resourceValues(forKeys: [.isAliasFileKey, .isSymbolicLinkKey])
        guard values?.isAliasFile == true, values?.isSymbolicLink != true else {
            LocalLaunch.openWithDefaultApps([url])
            return
        }
        DispatchQueue.global(qos: .userInitiated).async {
            let target = try? URL(resolvingAliasFileAt: url, options: [.withoutUI, .withoutMounting])
            let isDir = target.map { (try? $0.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true } ?? false
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                guard let target else {
                    self.aliasCannotBeResolved(path)
                    return
                }
                let isPackage = NSWorkspace.shared.isFilePackage(atPath: target.path)
                if isDir, !isPackage || AppEnvironment.shared.settings.treatPackagesAsFolders {
                    self.session.navigate(tab: self.tabId, path: target.path)
                } else {
                    LocalLaunch.openWithDefaultApps([target])
                }
            }
        }
    }

    private func aliasCannotBeResolved(_ path: String) {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "The item “\((path as NSString).lastPathComponent)” can’t be resolved."
        alert.informativeText = "The original item can’t be found. It may have been moved or deleted, "
            + "or be on a volume that isn’t connected."
        pendingConfirmation = alert
        guard let window = view.window else { return }
        alert.beginSheetModal(for: window) { [weak self] _ in self?.pendingConfirmation = nil }
    }

    // MARK: Folders this user can't read

    /// The extra button of the inline error: "Authorize…" when the folder's
    /// own permissions keep this user out, "Open Privacy Settings" when
    /// macOS privacy protection does.
    func configureAccessAction(path: String, error: RustyFinderError) {
        guard !isRemote, case .PermissionDenied = error else {
            banner.setExtra(nil, action: nil)
            return
        }
        switch FolderAccess.problem(at: path) {
        case .privacy:
            banner.setExtra("Open Privacy Settings") { FolderAccess.openPrivacySettings() }
        case .permissions:
            banner.setExtra("Authorize…") { [weak self] in self?.authorize(path) }
        case nil:
            banner.setExtra(nil, action: nil)
        }
    }

    /// Ask once more, then give this user read access through the system's
    /// authorization prompt, and try the folder again.
    func authorize(_ path: String) {
        guard let window = view.window else { return }
        let name = (path as NSString).lastPathComponent
        let alert = NSAlert()
        alert.messageText = "Give yourself access to “\(name)”?"
        alert.informativeText = "macOS will ask for an administrator’s name and password. RustyFinder then adds an "
            + "access entry that lets your account (\(NSUserName())) open and list this folder. Its other permissions "
            + "stay as they are."
        alert.addButton(withTitle: "Authorize")
        let cancel = alert.addButton(withTitle: "Cancel")
        cancel.keyEquivalent = "\u{1b}"
        pendingConfirmation = alert
        alert.beginSheetModal(for: window) { [weak self] response in
            self?.pendingConfirmation = nil
            guard response == .alertFirstButtonReturn else { return }
            FolderAccess.grantRead(path) { [weak self] message in
                guard let self else { return }
                guard let message else {
                    self.session.navigate(tab: self.tabId, path: path)
                    return
                }
                let problem = NSAlert()
                problem.messageText = "Access to “\(name)” wasn’t changed."
                problem.informativeText = message
                self.pendingConfirmation = problem
                if let window = self.view.window {
                    problem.beginSheetModal(for: window) { [weak self] _ in self?.pendingConfirmation = nil }
                }
            }
        }
    }
}

/// Why a local folder can't be read.
@MainActor
enum FolderAccess {
    enum Problem {
        /// Its POSIX permissions or access list keep this user out.
        case permissions
        /// macOS privacy protection (Full Disk Access, Files and Folders).
        case privacy
    }

    /// Asks the system directly: privacy protection answers EPERM, plain
    /// permissions EACCES.
    static func problem(at path: String) -> Problem? {
        guard let dir = opendir(path) else {
            switch errno {
            case EPERM: return .privacy
            case EACCES: return .permissions
            default: return nil
            }
        }
        closedir(dir)
        return nil
    }

    static func openPrivacySettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles") {
            ExternalApps.open(url)
        }
    }

    /// Add an access entry giving this user list and read rights on the
    /// folder (not inherited), as an administrator. `done` gets an error
    /// message, or nil.
    static func grantRead(_ path: String, done: @escaping (String?) -> Void) {
        let argv = ["/bin/chmod", "+a", "user:\(NSUserName()) allow list,search,readattr,readextattr,readsecurity", path]
        let prompt = "RustyFinder wants to let you open “\((path as NSString).lastPathComponent)”."
        DispatchQueue.global(qos: .userInitiated).async {
            let message = AdminAuthorization.run(argv, prompt: prompt)
            DispatchQueue.main.async { done(message) }
        }
    }
}
