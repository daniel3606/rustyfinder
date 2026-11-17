import Foundation

/// Small choices the app remembers (on/off switches, column lists).
/// Scripted checks run with their own support folder and must neither read
/// nor change the user's, so there the values live in a file in that
/// folder instead; they still survive a relaunch within one check.
enum Prefs {
    private static let scratchFile: URL? = ProcessInfo.processInfo.environment["RUSTYFINDER_SUPPORT_DIR"]
        .map { URL(fileURLWithPath: $0).appendingPathComponent("prefs.plist") }
    private static var scratch: [String: Any] = scratchFile
        .flatMap { NSDictionary(contentsOf: $0) as? [String: Any] } ?? [:]

    private static func value(_ key: String) -> Any? {
        scratchFile == nil ? UserDefaults.standard.object(forKey: key) : scratch[key]
    }

    private static func store(_ key: String, _ value: Any) {
        guard let file = scratchFile else {
            UserDefaults.standard.set(value, forKey: key)
            return
        }
        scratch[key] = value
        (scratch as NSDictionary).write(to: file, atomically: true)
    }

    static func bool(_ key: String) -> Bool { value(key) as? Bool ?? false }

    static func set(_ key: String, _ on: Bool) { store(key, on) }

    /// A remembered list of strings, nil when never set.
    static func strings(_ key: String) -> [String]? { value(key) as? [String] }

    static func setStrings(_ key: String, _ list: [String]) { store(key, list) }
}
