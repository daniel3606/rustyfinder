import AppKit

/// The external program that shows two files side by side: FileMerge,
/// Kaleidoscope, Beyond Compare or Araxis Merge, as chosen in Settings.
/// Where each lives is the core's business (`compare_tools`).
struct CompareTool {
    let executable: String

    /// Every tool and whether it is installed.
    static func all() -> [CompareToolInfo] {
        compareTools(home: NSHomeDirectory(), filemergePresent: fileMergePresent())
    }

    /// The tool chosen in Settings (installed or not).
    @MainActor
    static var chosen: CompareToolInfo? {
        let key = AppEnvironment.shared.settings.compareTool
        let tools = all()
        return tools.first { $0.key == key } ?? tools.first
    }

    /// The chosen tool, if it is installed. The scripted checks point
    /// `RUSTYFINDER_COMPARE_TOOL` at a stand-in instead.
    @MainActor
    static func resolve() -> CompareTool? {
        if let override = ProcessInfo.processInfo.environment["RUSTYFINDER_COMPARE_TOOL"] {
            return FileManager.default.isExecutableFile(atPath: override) ? CompareTool(executable: override) : nil
        }
        return chosen?.executable.map(CompareTool.init)
    }

    /// /usr/bin/opendiff is only a stub without Xcode: it needs the
    /// FileMerge that lives inside the selected Xcode.
    private static func fileMergePresent() -> Bool {
        guard let developer = output(of: "/usr/bin/xcode-select", ["-p"]) else { return false }
        let fileMerge = ((developer as NSString).deletingLastPathComponent as NSString)
            .appendingPathComponent("Applications/FileMerge.app")
        return FileManager.default.fileExists(atPath: fileMerge)
    }

    /// Start the tool on two files. Returns a message if it cannot start.
    func run(_ left: String, _ right: String) -> String? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = [left, right]
        do {
            try process.run()
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    private static func output(of tool: String, _ arguments: [String]) -> String? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: tool)
        process.arguments = arguments
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = Pipe()
        guard (try? process.run()) != nil else { return nil }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else { return nil }
        let text = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines)
        return text?.isEmpty == false ? text : nil
    }
}

/// Settings rows for the tools area: the compare tool and whether custom
/// tools get the login shell's `$PATH`. Each control keeps the setting
/// itself, so the Settings window only has to place them.
@MainActor
enum ToolSettingsRows {
    /// Pop-up of compare tools; ones not installed say so.
    static func comparePopup() -> NSPopUpButton {
        let popup = ComparePopup(frame: .zero, pullsDown: false)
        popup.reload()
        popup.setAccessibilityLabel("Compare tool")
        return popup
    }

    static func inheritPathBox() -> NSButton {
        InheritPathBox(checkboxWithTitle: "Inherit $PATH from the login shell", target: nil, action: nil).setUp()
    }
}

private final class ComparePopup: NSPopUpButton {
    func reload() {
        removeAllItems()
        let current = AppEnvironment.shared.settings.compareTool
        for tool in CompareTool.all() {
            addItem(withTitle: tool.executable == nil ? "\(tool.name) (not installed)" : tool.name)
            lastItem?.representedObject = tool.key
            if tool.key == current { select(lastItem) }
        }
        target = self
        action = #selector(chosen(_:))
    }

    @objc private func chosen(_ sender: Any?) {
        guard let key = selectedItem?.representedObject as? String else { return }
        AppEnvironment.shared.update { $0.compareTool = key }
    }
}

private final class InheritPathBox: NSButton {
    func setUp() -> NSButton {
        state = AppEnvironment.shared.settings.toolsInheritPath ? .on : .off
        toolTip = "Custom tools run through your login shell and find commands the way Terminal does."
        target = self
        action = #selector(toggled(_:))
        identifier = .init("tools_inherit_path")
        return self
    }

    @objc private func toggled(_ sender: Any?) {
        AppEnvironment.shared.update { $0.toolsInheritPath = self.state == .on }
    }
}
