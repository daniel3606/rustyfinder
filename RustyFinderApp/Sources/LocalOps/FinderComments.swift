import Foundation

/// Finder comments. Finder keeps them in its own store, so only Finder can
/// set one that then shows in its Get Info; this asks it to.
enum FinderComments {
    /// Comments set during a scripted check, which never drives Finder.
    nonisolated(unsafe) private static var scripted: [String: String] = [:]
    private static let lock = NSLock()

    private static var isScripted: Bool {
        ProcessInfo.processInfo.environment["RUSTYFINDER_SUPPORT_DIR"] != nil
            && CommandLine.arguments.contains("--keys")
    }

    /// Set the comment of the item at `path`. Blocks while Finder works, so
    /// call it off the main thread. Returns an error message, or nil.
    static func set(_ comment: String, at path: String) -> String? {
        if isScripted {
            lock.lock()
            scripted[path] = comment
            lock.unlock()
            return nil
        }
        let quote = { (text: String) in
            text.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
        }
        let source = "tell application \"Finder\" to set comment of (POSIX file \"\(quote(path))\" as alias) to \"\(quote(comment))\""
        var error: NSDictionary?
        NSAppleScript(source: source)?.executeAndReturnError(&error)
        return error?[NSAppleScript.errorMessage] as? String
    }

    /// Every comment a scripted check set: "name=comment,…".
    static var scriptedSummary: String {
        lock.lock()
        defer { lock.unlock() }
        let all = scripted.sorted { $0.key < $1.key }.map { "\(($0.key as NSString).lastPathComponent)=\($0.value)" }
        return all.isEmpty ? "-" : all.joined(separator: ",")
    }

    /// What a scripted check set (nil if it set nothing for `path`).
    static func scriptedComment(at path: String) -> String? {
        lock.lock()
        defer { lock.unlock() }
        return scripted[path]
    }
}
