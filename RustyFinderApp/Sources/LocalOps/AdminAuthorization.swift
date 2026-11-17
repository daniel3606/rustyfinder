import Foundation

/// Running one command as an administrator, through the system's own
/// authorization prompt (`do shell script … with administrator
/// privileges`). RustyFinder installs no privileged helper and never sees
/// the password. Every argument is quoted by the Rust core, so no path
/// can change what runs.
enum AdminAuthorization {
    /// Scripted checks never type a password: the request is recorded and
    /// treated as declined, which must count as a failure.
    private static var isScripted: Bool {
        ProcessInfo.processInfo.environment["RUSTYFINDER_SUPPORT_DIR"] != nil
            && CommandLine.arguments.contains("--keys")
    }

    /// Run `argv` as an administrator. Blocks until the user has answered
    /// and the command has run, so call it off the main thread. Returns an
    /// error message, or nil on success.
    static func run(_ argv: [String], prompt: String) -> String? {
        if isScripted {
            DispatchQueue.main.async { ExternalApps.record("admin: \(privilegedShellLine(argv: argv))") }
            return "User canceled."
        }
        let script = privilegedAppleScript(argv: argv, prompt: prompt)
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
        process.arguments = ["-e", script]
        let errors = Pipe()
        process.standardError = errors
        process.standardOutput = FileHandle.nullDevice
        do {
            try process.run()
        } catch {
            return error.localizedDescription
        }
        process.waitUntilExit()
        guard process.terminationStatus != 0 else { return nil }
        let text = String(data: errors.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        // "execution error: User canceled. (-128)" -> "User canceled."
        let message = text.components(separatedBy: "execution error: ").last?
            .replacingOccurrences(of: #"\s*\(-?\d+\)\s*$"#, with: "", options: .regularExpression)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return (message?.isEmpty == false ? message : nil) ?? "The command failed."
    }

    /// The prompt text the system shows above the password field.
    static func prompt(for step: PrivilegedStep) -> String {
        let name = (step.path as NSString).lastPathComponent
        switch step.kind {
        case "remove": return "RustyFinder wants to delete “\(name)”."
        case "move": return "RustyFinder wants to move “\(name)”."
        case "copy": return "RustyFinder wants to copy “\(name)”."
        case "mkdir": return "RustyFinder wants to create the folder “\(name)”."
        case "trash": return "RustyFinder wants to move “\(name)” to the Trash."
        default: return "RustyFinder wants to change “\(name)”."
        }
    }
}
