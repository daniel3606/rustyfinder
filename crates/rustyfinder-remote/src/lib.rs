//! rustyfinder-remote: remote file systems behind the core's `Vfs` trait.
//!
//! - `askpass`  relays ssh's questions (password, passphrase, host key) to
//!   the app and back
//! - `sftp`     SFTP over the system OpenSSH client
//! - `ftp`      FTP and FTP over TLS
//! - `webdav`   WebDAV over HTTP and HTTPS
//! - `s3`       Amazon S3 and S3-compatible services
//! - `trust`    server certificates the user chose to trust

pub mod askpass;
pub mod ftp;
pub mod s3;
pub mod sftp;
pub mod trust;
pub mod webdav;
mod webdav_tls;

/// Why a connection to a server (FTP, WebDAV, S3) could not be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemoteError {
    /// The server cannot be found or does not answer.
    #[error("{0}")]
    Unreachable(String),
    /// The server refused the credentials.
    #[error("{0}")]
    Auth(String),
    /// The server's certificate could not be verified.
    #[error("The server’s certificate can’t be verified, so it may not be the server it claims to be. {0}")]
    Certificate(String),
    #[error("{0}")]
    Other(String),
}

pub use askpass::{relay_main, PromptKind, Prompter};
pub use sftp::{connect, ConnectError, SftpFs, SftpParams};
