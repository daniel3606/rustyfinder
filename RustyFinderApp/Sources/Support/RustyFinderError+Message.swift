import Foundation

extension RustyFinderError {
    /// The human-readable text carried by every variant.
    var message: String {
        switch self {
        case .NotFound(let message), .PermissionDenied(let message), .NotADirectory(let message),
             .AlreadyExists(let message), .Cancelled(let message), .Unsupported(let message),
             .Io(let message), .Invalid(let message), .ReadOnly(let message), .NoSpace(let message):
            return message
        }
    }
}
