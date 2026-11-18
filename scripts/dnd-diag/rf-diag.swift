// Polls system-wide input state while a drag bug is reproduced and prints
// every change: event taps (CGGetEventTapList), secure input, the window
// server's left-button state, the frontmost app, and windows the watched
// process owns above the normal layer (a drag image window, for example).
//
//   swiftc -O -o rf-diag rf-diag.swift && ./rf-diag RustyFinder [seconds]
import AppKit

let target = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "RustyFinder"
let seconds = CommandLine.arguments.count > 2 ? Double(CommandLine.arguments[2]) ?? 60 : 60
let start = Date()

func stamp() -> String { String(format: "%7.2f", Date().timeIntervalSince(start)) }

func taps() -> String {
    var count: UInt32 = 0
    CGGetEventTapList(0, nil, &count)
    var list = [CGEventTapInformation](repeating: CGEventTapInformation(), count: Int(count))
    CGGetEventTapList(count, &list, &count)
    return list.prefix(Int(count)).map { t in
        let name = NSRunningApplication(processIdentifier: pid_t(t.tappingProcess))?.localizedName ?? "pid\(t.tappingProcess)"
        let kind = t.options == .listenOnly ? "listen" : "ACTIVE"
        return "\(name)[\(kind) on=\(t.enabled) mask=0x\(String(t.eventsOfInterest, radix: 16))]"
    }.sorted().joined(separator: " ")
}

func secureInput() -> String {
    let dict = CGSessionCopyCurrentDictionary() as? [String: Any]
    return "\(dict?["kCGSSessionSecureInputPID"] ?? 0)"
}

func ownedWindows() -> String {
    guard let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] else { return "?" }
    return list.compactMap { w -> String? in
        guard (w[kCGWindowOwnerName as String] as? String) == target else { return nil }
        let layer = w[kCGWindowLayer as String] as? Int ?? 0
        guard layer != 0 else { return nil }
        let on = w[kCGWindowIsOnscreen as String] as? Bool ?? false
        let b = w[kCGWindowBounds as String] as? [String: Any] ?? [:]
        return "layer\(layer)\(on ? "/on" : "")@\(b["X"] ?? 0),\(b["Y"] ?? 0) \(b["Width"] ?? 0)x\(b["Height"] ?? 0)"
    }.joined(separator: " ")
}

var last: [String: String] = [:]
while Date().timeIntervalSince(start) < seconds {
    let state: [String: String] = [
        "taps": taps(),
        "secureInputPID": secureInput(),
        "leftDown": "session=\(CGEventSource.buttonState(.combinedSessionState, button: .left)) hid=\(CGEventSource.buttonState(.hidSystemState, button: .left))",
        "front": NSWorkspace.shared.frontmostApplication?.localizedName ?? "?",
        "\(target)Windows(layer!=0)": ownedWindows(),
    ]
    for (key, value) in state.sorted(by: { $0.key < $1.key }) where last[key] != value {
        print("\(stamp()) \(key): \(value)")
        fflush(stdout)
    }
    last = state
    Thread.sleep(forTimeInterval: 0.25)
}
