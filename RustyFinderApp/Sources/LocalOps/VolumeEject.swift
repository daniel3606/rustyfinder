import AppKit

/// Ejecting a volume (edge-cases §2, keyboard ⌘⇧E): RustyFinder's own tabs
/// on it move to the starting folder first so they never keep it busy, and
/// if something else does, the alert names it.
@MainActor
enum VolumeEject {
    private(set) static var lastEvent = "-"

    /// The mounted volume holding `path`, if it can be ejected (not the
    /// startup disk).
    static func ejectableVolume(of path: String) -> URL? {
        let url = URL(fileURLWithPath: path)
        guard let volume = (try? url.resourceValues(forKeys: [.volumeURLKey]))?.volume else { return nil }
        let keys: Set<URLResourceKey> = [.volumeIsRootFileSystemKey, .volumeIsEjectableKey, .volumeIsRemovableKey,
                                         .volumeIsInternalKey, .volumeIsLocalKey]
        guard let values = try? volume.resourceValues(forKeys: keys), values.volumeIsRootFileSystem != true,
              volume.path != "/", !volume.path.hasPrefix("/System/Volumes")
        else { return nil }
        let external = values.volumeIsEjectable == true || values.volumeIsRemovable == true
            || values.volumeIsInternal == false || values.volumeIsLocal == false
        return external ? volume : nil
    }

    /// Move panes off `volume`, then unmount and eject it off the main
    /// thread. `window` shows the alert if it fails.
    static func eject(_ volume: URL, window: NSWindow?) {
        let mount = volume.standardizedFileURL.path
        for controller in (NSApp.delegate as? AppDelegate)?.windowControllers ?? [] {
            controller.leaveVolume(mount)
        }
        lastEvent = "ejecting \(volume.lastPathComponent)"
        // Give the panes a moment to let go of their folder watches.
        DispatchQueue.global(qos: .userInitiated).asyncAfter(deadline: .now() + 0.3) {
            var failure: Error?
            do {
                try NSWorkspace.shared.unmountAndEjectDevice(at: volume)
            } catch {
                failure = error
            }
            let users = failure == nil ? [] : processes(using: mount)
            DispatchQueue.main.async {
                guard let failure else {
                    lastEvent = "ejected \(volume.lastPathComponent)"
                    return
                }
                // Applications by their proper name, other processes as lsof says.
                var names: [String] = []
                for (pid, command) in users {
                    let name = NSRunningApplication(processIdentifier: pid)?.localizedName ?? command
                    if !names.contains(name) { names.append(name) }
                }
                report(volume, failure, users: names, window: window)
            }
        }
    }

    /// A failed unmount: say which applications keep the volume busy.
    static func reportFailure(_ volume: URL, _ error: Error, users: [(pid_t, String)], window: NSWindow?) {
        var names: [String] = []
        for (pid, command) in users {
            let name = NSRunningApplication(processIdentifier: pid)?.localizedName ?? command
            if !names.contains(name) { names.append(name) }
        }
        report(volume, error, users: names, window: window)
    }

    private static func report(_ volume: URL, _ error: Error, users: [String], window: NSWindow?) {
        let name = FileManager.default.displayName(atPath: volume.path)
        let alert = NSAlert()
        alert.alertStyle = .warning
        if users.isEmpty {
            alert.messageText = "The volume “\(name)” couldn’t be ejected."
            alert.informativeText = error.localizedDescription
        } else {
            let list = ListFormatter.localizedString(byJoining: users)
            alert.messageText = "The volume “\(name)” can’t be ejected because it’s in use by \(list)."
            alert.informativeText = "Quit \(users.count == 1 ? "that application" : "those applications") or close its files on the volume, then try again."
        }
        lastEvent = alert.messageText
        if let window { alert.beginSheetModal(for: window) } else { alert.runModal() }
    }

    /// The processes with files open on the volume mounted at `mount`
    /// (other than RustyFinder): pid and command name, as `lsof` reports them.
    nonisolated static func processes(using mount: String) -> [(pid_t, String)] {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/sbin/lsof")
        // -F pc: machine-readable pid and command; +f --: the whole file system.
        process.arguments = ["-F", "pc", "+f", "--", mount]
        let out = Pipe()
        process.standardOutput = out
        process.standardError = FileHandle.nullDevice
        guard (try? process.run()) != nil else { return [] }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        var found: [(pid_t, String)] = []
        var pid: pid_t = 0
        for line in String(decoding: data, as: UTF8.self).split(separator: "\n") {
            if line.hasPrefix("p") {
                pid = pid_t(line.dropFirst()) ?? 0
            } else if line.hasPrefix("c"), pid != getpid() {
                found.append((pid, String(line.dropFirst())))
            }
        }
        return found
    }
}
