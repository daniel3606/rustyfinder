import AppKit
import UserNotifications

/// A system notification when a long activity finishes while RustyFinder
/// is in the background (file-ops §5.6). Permission is asked the first
/// time one is due, not at launch.
@MainActor
enum CompletionNotifier {
    /// Activities shorter than this finish without a notification.
    static var threshold: TimeInterval {
        ProcessInfo.processInfo.environment["RUSTYFINDER_NOTIFY_AFTER"].flatMap(TimeInterval.init) ?? 10
    }

    /// RustyFinder is not the frontmost app. Scripted checks can't rely on
    /// focus (the app activates at launch), so they say which to assume.
    private static var inBackground: Bool {
        switch ProcessInfo.processInfo.environment["RUSTYFINDER_PRETEND_BACKGROUND"] {
        case "1": return true
        case "0": return false
        default: return !NSApp.isActive
        }
    }

    /// When each activity was first seen.
    private static var started: [UInt64: Date] = [:]
    private(set) static var last = "-"

    static func seen(_ activities: [ActivityRecord]) {
        let now = Date()
        for activity in activities where started[activity.id] == nil { started[activity.id] = now }
    }

    static func finished(_ report: FinishRecord) {
        guard let start = started.removeValue(forKey: report.id) else { return }
        guard Date().timeIntervalSince(start) >= threshold, inBackground, report.notable else { return }
        let title = report.title
        let body: String
        switch report.result {
        case .success: body = "Finished."
        case .partial: body = "Finished, but some items failed." + (report.message.map { " \($0)" } ?? "")
        case .failed: body = "Failed." + (report.message.map { " \($0)" } ?? "")
        case .stopped: body = "Stopped."
        }
        last = "\(title): \(body)"
        if ExternalApps.isDryRun { return }
        let center = UNUserNotificationCenter.current()
        center.requestAuthorization(options: [.alert, .sound]) { granted, _ in
            guard granted else { return }
            let content = UNMutableNotificationContent()
            content.title = title
            content.body = body
            center.add(UNNotificationRequest(identifier: "activity-\(report.id)", content: content, trigger: nil))
        }
    }
}
