use std::io;

pub type VfsResult<T> = Result<T, VfsError>;

/// Errors every backend maps its failures onto, so the UI can react by kind.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VfsError {
    #[error("“{0}” could not be found")]
    NotFound(String),
    #[error("You don’t have permission to access “{0}”")]
    PermissionDenied(String),
    #[error("“{0}” is not a folder")]
    NotADirectory(String),
    #[error("“{0}” already exists")]
    AlreadyExists(String),
    #[error("The operation was cancelled")]
    Cancelled,
    #[error("Not supported: {0}")]
    Unsupported(String),
    /// A rename across volumes; the caller should copy and delete instead.
    #[error("“{0}” is on a different volume")]
    CrossDevice(String),
    #[error("“{0}” is not empty")]
    NotEmpty(String),
    #[error("The volume is read only")]
    ReadOnly,
    #[error("There is not enough free space")]
    NoSpace,
    /// A rule was broken before anything was touched; the text says which.
    #[error("{0}")]
    Invalid(String),
    #[error("{path}: {message}")]
    Io { path: String, message: String },
    /// The server is busy and asked to be tried again after a while
    /// ("rate limited", "retry after N seconds").
    #[error("{path}: {message}")]
    Throttled { path: String, message: String, retry_after_secs: u64 },
}

impl VfsError {
    /// Map an `io::Error` raised while touching `path`.
    pub fn from_io(path: &str, err: &io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::NotFound => VfsError::NotFound(path.to_string()),
            io::ErrorKind::PermissionDenied => VfsError::PermissionDenied(path.to_string()),
            io::ErrorKind::AlreadyExists => VfsError::AlreadyExists(path.to_string()),
            io::ErrorKind::NotADirectory => VfsError::NotADirectory(path.to_string()),
            io::ErrorKind::CrossesDevices => VfsError::CrossDevice(path.to_string()),
            io::ErrorKind::DirectoryNotEmpty => VfsError::NotEmpty(path.to_string()),
            io::ErrorKind::ReadOnlyFilesystem => VfsError::ReadOnly,
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => VfsError::NoSpace,
            _ => VfsError::Io { path: path.to_string(), message: err.to_string() },
        }
    }
}
