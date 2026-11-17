import AppKit

/// The Connect panel (Cmd-K): server, user, password and the advanced
/// fields. Errors show inside it and the fields stay editable.
final class ConnectSheetController: NSViewController, NSTextFieldDelegate {
    /// Called with the new connection once it is open.
    var onConnected: ((ConnectionRecord) -> Void)?
    /// A network share (or a VNC address) was asked for: mount it and
    /// report nil, or what went wrong.
    var onOpenShare: ((ServerParams, String, @escaping (String?) -> Void) -> Void)?

    private let protocolPopup = NSPopUpButton()
    private let name = NSTextField()
    private let server = NSTextField()
    private let user = NSTextField()
    private let password = NSSecureTextField()
    private let port = NSTextField()
    private let remotePath = NSTextField()
    private let localPath = NSTextField()
    private let colorPopup = NSPopUpButton()
    private let remember = NSButton(checkboxWithTitle: "Remember password in the keychain", target: nil, action: nil)
    private let addToFavorites = NSButton(checkboxWithTitle: "Add to Favorites", target: nil, action: nil)
    private let errorLabel = NSTextField(wrappingLabelWithString: "")
    private let spinner = NSProgressIndicator()
    private let connectButton = NSButton(title: "Connect", target: nil, action: nil)
    private let cancelButton = NSButton(title: "Cancel", target: nil, action: nil)
    private let advancedToggle = NSButton(title: "Show Advanced", target: nil, action: nil)
    private let passive = NSButton(checkboxWithTitle: "Passive Mode", target: nil, action: nil)
    private let implicitTLS = NSButton(checkboxWithTitle: "Implicit TLS (port 990)", target: nil, action: nil)
    private let encoding = NSPopUpButton()
    private let region = NSComboBox()
    private let addressing = NSPopUpButton()
    private let storageClass = NSPopUpButton()

    /// The kinds of server on offer: title, protocol, URL scheme, port.
    private static let protocols: [(title: String, kind: RemoteProtocol, scheme: String, port: String)] = [
        ("SFTP", .sftp, "sftp", "22"), ("FTP", .ftp, "ftp", "21"), ("FTP with TLS", .ftps, "ftps", "21"),
        ("WebDAV", .webDav, "http", "80"), ("WebDAV HTTPS", .webDavs, "https", "443"),
        ("Amazon S3 and compatible", .s3, "s3", "443"),
        ("SMB (Windows share)", .smb, "smb", "445"), ("AFP", .afp, "afp", "548"), ("NFS", .nfs, "nfs", "2049"),
        ("VNC (Screen Sharing)", .vnc, "vnc", "5900"),
    ]
    /// Name encodings for servers that do not speak UTF-8.
    private static let encodings: [(String, String?)] = [
        ("Automatic", nil), ("UTF-8", "utf-8"), ("Western (Windows-1252)", "windows-1252"),
        ("Cyrillic (Windows-1251)", "windows-1251"), ("Japanese (Shift JIS)", "shift_jis"),
        ("Chinese (GBK)", "gbk"), ("Korean (EUC-KR)", "euc-kr"),
    ]
    /// Tints for the tab and favorite.
    private static let colors: [(String, String)] = [
        ("None", ""), ("Red", "#FF3B30"), ("Orange", "#FF9500"), ("Yellow", "#FFCC00"), ("Green", "#34C759"),
        ("Blue", "#007AFF"), ("Purple", "#AF52DE"), ("Gray", "#8E8E93"),
    ]
    private static let addressings: [(String, Bool?)] = [
        ("Automatic", nil), ("Path style (host/bucket)", true), ("Virtual-hosted (bucket.host)", false),
    ]

    private var chosen: (title: String, kind: RemoteProtocol, scheme: String, port: String) {
        Self.protocols[max(0, protocolPopup.indexOfSelectedItem)]
    }
    private var advancedRows: [NSView] = []
    private var isConnecting = false

    override func loadView() {
        let root = NSView()
        protocolPopup.addItems(withTitles: Self.protocols.map(\.title))
        protocolPopup.target = self
        protocolPopup.action = #selector(protocolChanged(_:))
        passive.state = .on
        encoding.addItems(withTitles: Self.encodings.map(\.0))
        for (title, hex) in Self.colors {
            colorPopup.addItem(withTitle: title)
            colorPopup.lastItem?.image = hex.isEmpty ? nil : Self.swatch(hex)
        }
        colorPopup.setAccessibilityLabel("Color")
        region.addItems(withObjectValues: s3Regions().map(\.code))
        region.completes = true
        region.placeholderString = "us-east-1"
        region.widthAnchor.constraint(equalToConstant: 260).isActive = true
        addressing.addItems(withTitles: Self.addressings.map(\.0))
        storageClass.addItems(withTitles: s3StorageClasses())
        implicitTLS.target = self
        implicitTLS.action = #selector(implicitChanged(_:))
        for field in [name, server, user, password, port, remotePath, localPath] {
            field.delegate = self
            field.widthAnchor.constraint(equalToConstant: 260).isActive = true
        }
        name.placeholderString = "For favorites (optional)"
        server.placeholderString = "example.org or sftp://user@host:22/path"
        user.placeholderString = "From ~/.ssh/config if empty"
        port.placeholderString = "22"
        remotePath.placeholderString = "Login folder if empty"
        localPath.placeholderString = "A folder on this Mac (optional)"
        errorLabel.textColor = .systemRed
        errorLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        errorLabel.isHidden = true
        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isDisplayedWhenStopped = false

        connectButton.target = self
        connectButton.action = #selector(connect(_:))
        connectButton.keyEquivalent = "\r"
        connectButton.isEnabled = false
        cancelButton.target = self
        cancelButton.action = #selector(cancel(_:))
        cancelButton.keyEquivalent = "\u{1b}"
        advancedToggle.target = self
        advancedToggle.action = #selector(toggleAdvanced(_:))
        advancedToggle.bezelStyle = .accessoryBarAction
        advancedToggle.controlSize = .small

        func row(_ name: String, _ controls: NSView...) -> NSView {
            let label = NSTextField(labelWithString: name)
            label.alignment = .right
            label.widthAnchor.constraint(equalToConstant: 96).isActive = true
            let line = NSStackView(views: [label] + controls)
            line.orientation = .horizontal
            line.alignment = .firstBaseline
            line.spacing = 8
            return line
        }
        let chooseLocal = NSButton(title: "Choose…", target: self, action: #selector(chooseLocalPath(_:)))
        chooseLocal.controlSize = .small
        let portRow = row("Port", port)
        let pathRow = row("Remote Path", remotePath)
        let localRow = row("Local Path", localPath, chooseLocal)
        let colorRow = row("Color", colorPopup)
        let passiveRow = row("", passive)
        let implicitRow = row("", implicitTLS)
        let encodingRow = row("Text Encoding", encoding)
        let regionRow = row("Region", region)
        let addressingRow = row("Addressing", addressing)
        let storageRow = row("Storage Class", storageClass)
        advancedRows = [
            portRow, pathRow, localRow, colorRow, passiveRow, implicitRow, encodingRow, regionRow, addressingRow,
            storageRow,
        ]
        advancedRows.forEach { $0.isHidden = true }
        protocolChanged(nil)

        let buttons = NSStackView(views: [advancedToggle, NSView(), spinner, cancelButton, connectButton])
        buttons.orientation = .horizontal
        buttons.spacing = 8
        let title = NSTextField(labelWithString: "Connect to Server")
        title.font = .systemFont(ofSize: NSFont.systemFontSize, weight: .semibold)

        let column = NSStackView(views: [
            title, row("Protocol", protocolPopup), row("Name", name), row("Server", server), row("Username", user),
            row("Password", password),
        ] + advancedRows + [row("", remember), row("", addToFavorites), errorLabel, buttons])
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 10
        column.edgeInsets = NSEdgeInsets(top: 18, left: 18, bottom: 16, right: 18)
        column.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(column)
        NSLayoutConstraint.activate([
            column.topAnchor.constraint(equalTo: root.topAnchor),
            column.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            column.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            column.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            buttons.widthAnchor.constraint(equalTo: column.widthAnchor, constant: -36),
            errorLabel.widthAnchor.constraint(equalTo: column.widthAnchor, constant: -36),
        ])
        view = root
    }

    private static func swatch(_ hex: String) -> NSImage {
        let color = connectionColor(hex) ?? .clear
        return NSImage(size: NSSize(width: 12, height: 12), flipped: false) { rect in
            color.setFill()
            NSBezierPath(ovalIn: rect.insetBy(dx: 1, dy: 1)).fill()
            return true
        }
    }

    // MARK: Fields

    func controlTextDidChange(_ notification: Notification) {
        guard let field = notification.object as? NSTextField else { return }
        if field === server { absorbURL() }
        if field === password {
            // A pasted password may carry a line break; it never belongs.
            let cleaned = password.stringValue.components(separatedBy: .newlines).joined()
            if cleaned != password.stringValue { password.stringValue = cleaned }
        }
        // Editing after a failure clears the message.
        errorLabel.isHidden = true
        validate()
    }

    /// A URL pasted into Server fills in the other fields and leaves only
    /// the host behind.
    private func absorbURL() {
        guard let parsed = parseConnectionUrl(text: server.stringValue),
              let index = Self.protocols.firstIndex(where: { $0.kind == parsed.protocol })
        else { return }
        // An S3-compatible service is often given as an http(s) address:
        // that is its endpoint, not a WebDAV URL.
        if chosen.kind == .s3, parsed.protocol != .s3 { return }
        protocolPopup.selectItem(at: index)
        protocolChanged(nil)
        server.stringValue = parsed.host
        if !parsed.user.isEmpty { user.stringValue = parsed.user }
        if let secret = parsed.password { password.stringValue = secret }
        if let number = parsed.port { port.stringValue = String(number) }
        if !parsed.path.isEmpty { remotePath.stringValue = parsed.path }
        if parsed.port != nil || !parsed.path.isEmpty { setAdvanced(shown: true) }
    }

    private var portValue: UInt16? { UInt16(port.stringValue.trimmingCharacters(in: .whitespaces)) }

    /// Connect is available once the required fields are filled: a server,
    /// and a port that is a number if one was typed.
    private func validate() {
        // Amazon S3 needs no server, only an access key.
        let hasServer = chosen.kind == .s3
            ? !user.stringValue.trimmingCharacters(in: .whitespaces).isEmpty
            : !server.stringValue.trimmingCharacters(in: .whitespaces).isEmpty
        let portOK = port.stringValue.isEmpty || portValue != nil
        connectButton.isEnabled = hasServer && portOK && !isConnecting
    }

    /// The placeholders and the protocol-only options follow the protocol.
    @objc private func protocolChanged(_ sender: Any?) {
        let kind = chosen.kind
        port.placeholderString = kind == .ftps && implicitTLS.state == .on ? "990" : chosen.port
        // S3 has keys instead of a login, a bucket instead of a path, and
        // its server may stay empty (Amazon itself).
        let s3 = kind == .s3
        password.placeholderString = s3 ? "Secret Key" : nil
        let share = [.smb, .afp, .nfs].contains(kind)
        remotePath.placeholderString = s3 ? "Bucket (all buckets if empty)" : (share ? "Share" : "Login folder if empty")
        if share { setAdvanced(shown: true) }
        for control in [region, addressing, storageClass] as [NSControl] { control.isEnabled = s3 }
        switch kind {
        case .s3: user.placeholderString = "Access Key"
        case .sftp: user.placeholderString = "From ~/.ssh/config if empty"
        case .webDav, .webDavs: user.placeholderString = "Optional"
        default: user.placeholderString = "anonymous if empty"
        }
        server.placeholderString = kind == .s3
            ? "Amazon if empty, or the host of a compatible service"
            : "example.org or \(chosen.scheme)://user@host/path"
        validate()
        let ftp = kind == .ftp || kind == .ftps
        passive.isEnabled = ftp
        implicitTLS.isEnabled = kind == .ftps
        // Name encodings matter where the server stores bytes: FTP and SFTP.
        encoding.isEnabled = ftp || kind == .sftp
        errorLabel.isHidden = true
    }

    @objc private func implicitChanged(_ sender: Any?) {
        protocolChanged(nil)
    }

    @objc private func toggleAdvanced(_ sender: Any?) {
        setAdvanced(shown: advancedRows.first?.isHidden ?? true)
    }

    private func setAdvanced(shown: Bool) {
        advancedRows.forEach { $0.isHidden = !shown }
        advancedToggle.title = shown ? "Hide Advanced" : "Show Advanced"
    }

    @objc private func chooseLocalPath(_ sender: Any?) {
        guard let window = view.window else { return }
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.prompt = "Choose"
        panel.beginSheetModal(for: window) { [weak self] response in
            if response == .OK, let path = panel.url?.path { self?.localPath.stringValue = path }
        }
    }

    // MARK: Connecting

    private var params: ServerParams {
        let path = remotePath.stringValue.trimmingCharacters(in: .whitespaces)
        let regionText = region.stringValue.trimmingCharacters(in: .whitespaces)
        let storage = storageClass.titleOfSelectedItem ?? "STANDARD"
        return ServerParams(
            protocol: chosen.kind, host: server.stringValue.trimmingCharacters(in: .whitespaces), port: portValue,
            user: user.stringValue.trimmingCharacters(in: .whitespaces), password: nil,
            remotePath: path.isEmpty ? nil : path, passive: passive.state == .on,
            encoding: Self.encodings[max(0, encoding.indexOfSelectedItem)].1,
            acceptInvalidCerts: false, region: regionText.isEmpty ? nil : regionText,
            implicitTls: chosen.kind == .ftps && implicitTLS.state == .on,
            pathStyle: Self.addressings[max(0, addressing.indexOfSelectedItem)].1,
            storageClass: storage == "STANDARD" ? nil : storage)
    }

    @objc private func connect(_ sender: Any?) {
        var params = self.params
        let account = Keychain.account(host: params.host, user: params.user, port: params.port, scheme: chosen.scheme)
        // An empty password field falls back to one saved earlier.
        let typed = password.stringValue
        params.password = typed.isEmpty ? Keychain.password(for: account) : typed
        let saveFavorite = addToFavorites.state == .on
        let keep = { [weak self] in
            guard let self else { return }
            if (self.remember.state == .on || saveFavorite), !typed.isEmpty { Keychain.store(typed, for: account) }
            if saveFavorite { self.saveConnection() }
        }

        setConnecting(true)
        // Shares are mounted by the system and then browsed as folders.
        if [.smb, .afp, .nfs, .vnc].contains(chosen.kind) {
            onOpenShare?(params, chosen.scheme) { [weak self] message in
                guard let self else { return }
                self.setConnecting(false)
                if let message {
                    self.errorLabel.stringValue = message
                    self.errorLabel.isHidden = false
                } else {
                    keep()
                    self.dismissSheet()
                }
            }
            return
        }
        ConnectionCenter.shared.connect(params, window: view.window?.sheetParent ?? view.window) { [weak self] result in
            guard let self else { return }
            self.setConnecting(false)
            switch result {
            case .success(let connection):
                keep()
                self.dismissSheet()
                self.onConnected?(connection)
            case .failure(let failure):
                // The panel stays open with the reason; fields stay editable.
                self.errorLabel.stringValue = failure.message
                self.errorLabel.isHidden = false
            }
        }
    }

    /// Remember this connection (never its password) for favorites.
    @discardableResult
    private func saveConnection() -> SavedConnectionRecord {
        let p = params
        return AppEnvironment.shared.core.saveConnection(connection: SavedConnectionRecord(
            id: 0, name: name.stringValue.trimmingCharacters(in: .whitespaces), protocol: p.protocol, host: p.host,
            port: p.port, user: p.user, remotePath: p.remotePath ?? "",
            localPath: localPath.stringValue.trimmingCharacters(in: .whitespaces),
            color: Self.colors[max(0, colorPopup.indexOfSelectedItem)].1, encoding: p.encoding ?? "",
            passive: p.passive, implicitTls: p.implicitTls, region: p.region ?? "", pathStyle: p.pathStyle,
            storageClass: p.storageClass ?? ""))
    }

    private func setConnecting(_ connecting: Bool) {
        isConnecting = connecting
        connecting ? spinner.startAnimation(nil) : spinner.stopAnimation(nil)
        validate()
    }

    @objc private func cancel(_ sender: Any?) {
        dismissSheet()
    }

    private func dismissSheet() {
        guard let window = view.window else { return }
        window.sheetParent?.endSheet(window)
    }

    // MARK: Scripted checks

    func debugFill(_ text: String) {
        loadViewIfNeeded()
        server.stringValue = text
        absorbURL()
        validate()
    }

    /// `field=value` for name, local, color (index), fav (on/off),
    /// implicit (on/off), region, addressing (index), storage (title),
    /// encoding (index), user, password, port, path; "go" presses Connect.
    func debugSet(_ text: String) {
        loadViewIfNeeded()
        if text == "go" {
            if connectButton.isEnabled { connect(nil) }
            return
        }
        let parts = text.split(separator: "=", maxSplits: 1).map(String.init)
        let value = parts.count > 1 ? parts[1] : ""
        switch parts[0] {
        case "name": name.stringValue = value
        case "local": localPath.stringValue = value
        case "color": colorPopup.selectItem(at: Int(value) ?? 0)
        case "fav": addToFavorites.state = value == "on" ? .on : .off
        case "implicit":
            implicitTLS.state = value == "on" ? .on : .off
            implicitChanged(nil)
        case "region": region.stringValue = value
        case "addressing": addressing.selectItem(at: Int(value) ?? 0)
        case "storage": storageClass.selectItem(withTitle: value)
        case "encoding": encoding.selectItem(at: Int(value) ?? 0)
        case "user": user.stringValue = value
        case "password": password.stringValue = value
        case "port": port.stringValue = value
        case "path": remotePath.stringValue = value
        default: break
        }
        validate()
    }

    func debugDump() -> String {
        "protocol=\(chosen.title) server=\(server.stringValue) user=\(user.stringValue) port=\(port.stringValue) path=\(remotePath.stringValue) password=\(password.stringValue.isEmpty ? "-" : "set") connect=\(connectButton.isEnabled) error=\(errorLabel.isHidden ? "-" : errorLabel.stringValue)"
            + " name=\(name.stringValue) local=\(localPath.stringValue) color=\(colorPopup.titleOfSelectedItem ?? "-") regions=\(region.numberOfItems) encoding=\(encoding.isEnabled ? "on" : "off") implicit=\(implicitTLS.isEnabled ? (implicitTLS.state == .on ? "on" : "off") : "-") storage=\(storageClass.isEnabled ? (storageClass.titleOfSelectedItem ?? "-") : "-")"
    }
}

/// A connection's tint, `#RRGGBB`.
func connectionColor(_ hex: String) -> NSColor? {
    let digits = hex.trimmingCharacters(in: CharacterSet(charactersIn: "#"))
    guard digits.count == 6, let value = UInt32(digits, radix: 16) else { return nil }
    return NSColor(
        srgbRed: CGFloat((value >> 16) & 0xFF) / 255, green: CGFloat((value >> 8) & 0xFF) / 255,
        blue: CGFloat(value & 0xFF) / 255, alpha: 1)
}
