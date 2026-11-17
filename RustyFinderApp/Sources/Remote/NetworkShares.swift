import AppKit
import NetFS

/// SMB, AFP and NFS shares. The system mounts them; a pane then browses
/// the mount point like any folder on this Mac, so everything local
/// (operations, tags, Quick Look) applies there too.
@MainActor
enum NetworkShares {
    struct Failure: Error {
        let message: String
    }

    /// The URL for a share: `smb://host[:port]/share`.
    static func url(scheme: String, host: String, port: UInt16?, share: String) -> URL? {
        var parts = URLComponents()
        parts.scheme = scheme
        parts.host = host
        parts.port = port.map(Int.init)
        let trimmed = share.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        parts.path = trimmed.isEmpty ? "" : "/" + trimmed
        return parts.url
    }

    /// Mount `url` and report where it was mounted. With a share in the
    /// URL nothing is shown; without one the system asks which share.
    static func mount(
        _ url: URL, user: String?, password: String?,
        completion: @escaping (Result<String, Failure>) -> Void
    ) {
        // Scripted checks run unattended: never touch the network there.
        if ExternalApps.isDryRun {
            ExternalApps.record("mount \(url.absoluteString)")
            completion(.failure(Failure(message: "Can’t mount network drive: not available during scripted checks.")))
            return
        }
        let hasShare = url.path.count > 1
        let open = NSMutableDictionary()
        open[kNAUIOptionKey] = hasShare ? kNAUIOptionNoUI : kNAUIOptionAllowUI
        var request: AsyncRequestID?
        let status = NetFSMountURLAsync(
            url as CFURL, nil,
            (user?.isEmpty == false ? user : nil) as CFString?,
            (password?.isEmpty == false ? password : nil) as CFString?,
            open as CFMutableDictionary, nil, &request, DispatchQueue.main
        ) { status, _, mountPoints in
            let paths = (mountPoints as? [String]) ?? []
            if status == 0, let path = paths.first {
                completion(.success(path))
            } else {
                completion(.failure(Failure(message: describe(status))))
            }
        }
        if status != 0 {
            completion(.failure(Failure(message: describe(status))))
        }
    }

    /// "Can't mount network drive: (code)" with the system's explanation.
    private static func describe(_ status: Int32) -> String {
        var reason = ""
        switch status {
        case EAUTH, EACCES, EPERM, -5045, -5023: reason = "The user name or password was not accepted."
        case ENOENT, -5019: reason = "The share does not exist on the server."
        case EHOSTUNREACH, EHOSTDOWN, ETIMEDOUT, ECONNREFUSED, -5016, -1073: reason = "The server can’t be reached."
        case ECANCELED, -128: reason = "The request was cancelled."
        default: reason = status > 0 ? String(cString: strerror(status)) : ""
        }
        return "Can’t mount network drive: (\(status)) \(reason)".trimmingCharacters(in: .whitespaces)
    }
}
