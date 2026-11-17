import AppKit

/// Text encodings offered in the encoding menu, grouped as in the spec.
enum TextEncodings {
    static let all: [(group: String, name: String, encoding: UInt32?)] = [
        ("Unicode", "Unicode (UTF-8)", nil),
        ("Western", "Western (Mac OS Roman)", 0x0000),
        ("Western", "Western (ISO Latin 1)", 0x0201),
        ("Western", "Western (ISO Latin 3)", cf(.isoLatin3)),
        ("Western", "Western (ISO Latin 9)", cf(.isoLatin9)),
        ("Western", "Western (NextStep)", 0x0B01),
        ("Central European", "Central European (ISO Latin 2)", cf(.isoLatin2)),
        ("Central European", "Central European (ISO Latin 4)", cf(.isoLatin4)),
        ("Central European", "Central European (Mac OS)", cf(.macCentralEurRoman)),
        ("Cyrillic", "Cyrillic (ISO 8859-5)", cf(.isoLatinCyrillic)),
        ("Cyrillic", "Cyrillic (Mac OS)", cf(.macCyrillic)),
        ("Cyrillic", "Cyrillic (Windows)", cf(.windowsCyrillic)),
        ("Greek", "Greek (ISO 8859-7)", cf(.isoLatinGreek)),
        ("Hebrew", "Hebrew (ISO 8859-8)", cf(.isoLatinHebrew)),
        ("Arabic", "Arabic (ISO 8859-6)", cf(.isoLatinArabic)),
        ("Arabic", "Arabic (Mac OS)", cf(.macArabic)),
        ("Baltic", "Baltic (ISO Latin 7)", cf(.isoLatin7)),
        ("Nordic", "Nordic (ISO Latin 6)", cf(.isoLatin6)),
        ("Romanian", "Romanian (ISO Latin 10)", cf(.isoLatin10)),
        ("Turkish", "Turkish (ISO Latin 5)", cf(.isoLatin5)),
        ("Turkish", "Turkish (Mac OS)", cf(.macTurkish)),
        ("Icelandic", "Icelandic (Mac OS)", cf(.macIcelandic)),
        ("Thai", "Thai (ISO 8859-11)", cf(.isoLatinThai)),
        ("Japanese", "Japanese (EUC)", cf(.EUC_JP)),
        ("Japanese", "Japanese (ISO 2022-JP)", 0x0820),
        ("Japanese", "Japanese (Mac OS)", cf(.macJapanese)),
        ("Japanese", "Japanese (Shift JIS)", cf(.shiftJIS)),
        ("Japanese", "Japanese (Windows)", cf(.dosJapanese)),
        ("Korean", "Korean (EUC)", cf(.EUC_KR)),
        ("Korean", "Korean (Mac OS)", cf(.macKorean)),
        ("Korean", "Korean (Windows)", cf(.dosKorean)),
        ("Simplified Chinese", "Simplified Chinese (GB 2312)", cf(.EUC_CN)),
        ("Simplified Chinese", "Simplified Chinese (Mac OS)", cf(.macChineseSimp)),
        ("Simplified Chinese", "Simplified Chinese (Windows)", cf(.dosChineseSimplif)),
        ("Traditional Chinese", "Traditional Chinese (EUC)", cf(.EUC_TW)),
        ("Traditional Chinese", "Traditional Chinese (Windows)", cf(.dosChineseTrad)),
        ("Latin-US", "Latin-US (DOS)", cf(.dosLatinUS)),
        ("ASCII", "Non-lossy ASCII", nil),
    ]

    private static func cf(_ encoding: CFStringEncodings) -> UInt32 {
        UInt32(encoding.rawValue)
    }

    /// Foundation encoding for an entry of `all`.
    static func encoding(at index: Int) -> String.Encoding {
        let entry = all[index]
        if entry.name == "Unicode (UTF-8)" { return .utf8 }
        if entry.name == "Non-lossy ASCII" { return .nonLossyASCII }
        guard let cf = entry.encoding else { return .utf8 }
        return String.Encoding(rawValue: CFStringConvertEncodingToNSStringEncoding(CFStringEncoding(cf)))
    }

    /// Foundation encoding for an IANA name from the detector.
    static func encoding(iana: String) -> String.Encoding {
        let cf = CFStringConvertIANACharSetNameToEncoding(iana as CFString)
        guard cf != kCFStringEncodingInvalidId else { return .macOSRoman }
        return String.Encoding(rawValue: CFStringConvertEncodingToNSStringEncoding(cf))
    }

    static func displayName(_ encoding: String.Encoding) -> String {
        if let index = all.indices.first(where: { Self.encoding(at: $0) == encoding }) { return all[index].name }
        return String.localizedName(of: encoding)
    }
}

/// Text view that saves on Cmd-S while it has focus.
final class QuickEditTextView: NSTextView {
    var onSave: (() -> Void)?

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let mods = event.modifierFlags.intersection([.command, .shift, .option, .control])
        if mods == .command, event.charactersIgnoringModifiers == "s", window?.firstResponder === self {
            onSave?()
            return true
        }
        return super.performKeyEquivalent(with: event)
    }
}

/// "Quick edit in place": a text file shown in an editable area with its
/// detected encoding. Saving writes it back in that same encoding.
final class TextPreviewView: NSView, NSTextViewDelegate {
    /// Files larger than this are previewed read-only by Quick Look instead.
    static let maxBytes = 2 * 1024 * 1024

    private let scroll = NSScrollView()
    private let textView = QuickEditTextView()
    private let encodingPopup = NSPopUpButton()
    private let saveButton = NSButton(title: "Save", target: nil, action: nil)
    private let status = NSTextField(labelWithString: "")

    private(set) var url: URL?
    private(set) var encoding: String.Encoding = .utf8
    /// Called after a successful save (a server copy is uploaded then).
    var onSaved: ((URL) -> Void)?
    /// Items inside archives are shown read-only.
    var isEditable: Bool {
        get { textView.isEditable }
        set {
            textView.isEditable = newValue
            saveButton.isHidden = !newValue
        }
    }
    private var bom = Data()
    private var isDirty = false { didSet { saveButton.isEnabled = isDirty } }

    override init(frame: NSRect) {
        super.init(frame: frame)
        textView.isRichText = false
        textView.allowsUndo = true
        textView.isAutomaticQuoteSubstitutionEnabled = false
        textView.isAutomaticDashSubstitutionEnabled = false
        textView.isAutomaticTextReplacementEnabled = false
        textView.isAutomaticSpellingCorrectionEnabled = false
        textView.font = .monospacedSystemFont(ofSize: NSFont.smallSystemFontSize, weight: .regular)
        textView.textContainerInset = NSSize(width: 6, height: 6)
        textView.isVerticallyResizable = true
        textView.autoresizingMask = [.width]
        textView.delegate = self
        textView.onSave = { [weak self] in self?.save() }
        textView.setAccessibilityLabel("File contents")

        scroll.documentView = textView
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.translatesAutoresizingMaskIntoConstraints = false

        encodingPopup.controlSize = .small
        encodingPopup.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        var lastGroup = ""
        for (index, entry) in TextEncodings.all.enumerated() {
            if entry.group != lastGroup, index > 0 { encodingPopup.menu?.addItem(.separator()) }
            lastGroup = entry.group
            encodingPopup.addItem(withTitle: entry.name)
            encodingPopup.lastItem?.tag = index
        }
        encodingPopup.target = self
        encodingPopup.action = #selector(encodingChosen(_:))
        encodingPopup.toolTip = "Text Encoding"

        saveButton.target = self
        saveButton.action = #selector(saveClicked(_:))
        saveButton.bezelStyle = .accessoryBarAction
        saveButton.controlSize = .small
        saveButton.isEnabled = false
        saveButton.toolTip = "Save (Cmd-S)"

        status.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        status.textColor = .secondaryLabelColor
        status.lineBreakMode = .byTruncatingTail

        let bar = NSStackView(views: [encodingPopup, status, saveButton])
        bar.orientation = .horizontal
        bar.spacing = 6
        bar.edgeInsets = NSEdgeInsets(top: 4, left: 8, bottom: 4, right: 8)
        bar.translatesAutoresizingMaskIntoConstraints = false
        status.setContentHuggingPriority(.defaultLow, for: .horizontal)
        encodingPopup.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        addSubview(scroll)
        addSubview(bar)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: topAnchor),
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: bar.topAnchor),
            bar.leadingAnchor.constraint(equalTo: leadingAnchor),
            bar.trailingAnchor.constraint(equalTo: trailingAnchor),
            bar.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    /// True if the file should be shown as editable text: small enough,
    /// and its bytes look like text rather than binary data.
    static func canShow(_ url: URL, size: UInt64) -> Bool {
        guard size <= maxBytes, let handle = try? FileHandle(forReadingFrom: url) else { return false }
        defer { try? handle.close() }
        let sample = (try? handle.read(upToCount: 64 * 1024)) ?? Data()
        return detectTextEncoding(bytes: sample).isText
    }

    /// Load the file, guessing its encoding unless one is given. Changing
    /// the encoding re-reads the file; it never modifies it.
    @discardableResult
    func load(_ url: URL, as chosen: String.Encoding? = nil) -> Bool {
        guard let data = try? Data(contentsOf: url) else { return false }
        let guess = detectTextEncoding(bytes: data)
        let skip = min(Int(guess.bomLength), data.count)
        let encoding = chosen ?? TextEncodings.encoding(iana: guess.name)
        // If the chosen encoding cannot decode the bytes, fall back to one
        // in which every byte is a character, so something is always shown.
        let body = data.subdata(in: skip..<data.count)
        let (text, used) = String(data: body, encoding: encoding).map { ($0, encoding) }
            ?? (String(data: body, encoding: .macOSRoman) ?? "", .macOSRoman)

        self.url = url
        self.encoding = used
        self.bom = data.subdata(in: 0..<skip)
        textView.string = text
        textView.undoManager?.removeAllActions()
        textView.scrollToBeginningOfDocument(nil)
        isDirty = false
        if let index = TextEncodings.all.indices.first(where: { TextEncodings.encoding(at: $0) == used }) {
            encodingPopup.selectItem(withTag: index)
        } else {
            // Detected an encoding the menu does not list: show its name.
            encodingPopup.setTitle(TextEncodings.displayName(used))
        }
        status.stringValue = ""
        return true
    }

    func textDidChange(_ notification: Notification) {
        isDirty = true
        status.stringValue = "Edited"
    }

    @objc private func encodingChosen(_ sender: NSPopUpButton) {
        guard let url else { return }
        load(url, as: TextEncodings.encoding(at: sender.selectedTag()))
    }

    @objc private func saveClicked(_ sender: Any?) { save() }

    /// Write the text back in the encoding the file has, keeping its
    /// byte-order mark. The file is replaced in one step so a failure
    /// cannot leave it half-written, and its attributes are kept.
    @discardableResult
    func save() -> Bool {
        guard let url, isDirty else { return false }
        guard let body = textView.string.data(using: encoding) else {
            status.stringValue = "Some characters can’t be saved as \(TextEncodings.displayName(encoding))"
            NSSound.beep()
            return false
        }
        let fm = FileManager.default
        let staging = url.deletingLastPathComponent().appendingPathComponent(".rustyfinder-edit-\(ProcessInfo.processInfo.processIdentifier)")
        do {
            try (bom + body).write(to: staging)
            if let attributes = try? fm.attributesOfItem(atPath: url.path), let mode = attributes[.posixPermissions] {
                try? fm.setAttributes([.posixPermissions: mode], ofItemAtPath: staging.path)
            }
            _ = try fm.replaceItemAt(url, withItemAt: staging)
        } catch {
            try? fm.removeItem(at: staging)
            status.stringValue = "Not saved: \(error.localizedDescription)"
            NSSound.beep()
            return false
        }
        isDirty = false
        status.stringValue = "Saved"
        onSaved?(url)
        return true
    }

    // MARK: Scripted checks

    var debugText: String { textView.string }

    func debugReplaceText(_ text: String) {
        textView.string = text
        textDidChange(Notification(name: NSText.didChangeNotification))
    }
}
