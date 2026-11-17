//! Server connections beyond connecting: trusted certificates, the text of
//! connection failures, saved connections, addresses of remote items, S3
//! regions.

use rustyfinder_core::connections::{SavedConnection, SavedConnections};

use crate::{RemoteProtocol, RustyFinderApp};

/// Certificates the user chose to always trust, in the support folder.
pub(crate) const TRUST_FILE: &str = "trusted-certificates.json";
const CONNECTIONS_FILE: &str = "connections.json";

/// The standard port of a protocol.
pub(crate) fn default_port(protocol: RemoteProtocol, implicit_tls: bool) -> u16 {
    match protocol {
        RemoteProtocol::Sftp => 22,
        RemoteProtocol::Ftp => 21,
        RemoteProtocol::Ftps if implicit_tls => 990,
        RemoteProtocol::Ftps => 21,
        RemoteProtocol::WebDav => 80,
        RemoteProtocol::WebDavs | RemoteProtocol::S3 => 443,
        RemoteProtocol::Smb => 445,
        RemoteProtocol::Afp => 548,
        RemoteProtocol::Nfs => 2049,
        RemoteProtocol::Vnc => 5900,
    }
}

/// The certificate fingerprint in a connection failure ("AA:2B:…"), when
/// the failure was a certificate the system could not verify.
#[uniffi::export]
pub fn certificate_fingerprint(message: String) -> Option<String> {
    rustyfinder_remote::trust::fingerprint_in(&message)
}

/// The new host key's `SHA256:` fingerprint in a "host key changed" failure.
#[uniffi::export]
pub fn changed_host_key_fingerprint(message: String) -> Option<String> {
    rustyfinder_remote::sftp::changed_host_key_fingerprint(&message)
}

/// True if the failure is the warning that a WebDAV server wants the
/// password unencrypted (Basic over plain HTTP).
#[uniffi::export]
pub fn is_insecure_password_warning(message: String) -> bool {
    message.starts_with(rustyfinder_remote::webdav::INSECURE_BASIC)
}

/// An AWS region for the Connect panel's pop-up.
#[derive(Debug, Clone, uniffi::Record)]
pub struct S3Region {
    pub code: String,
    pub name: String,
}

/// Every current AWS region, in the order the AWS console lists them.
#[uniffi::export]
pub fn s3_regions() -> Vec<S3Region> {
    rustyfinder_remote::s3::REGIONS
        .iter()
        .map(|(code, name)| S3Region { code: code.to_string(), name: name.to_string() })
        .collect()
}

/// Storage classes offered for S3 uploads, STANDARD first.
#[uniffi::export]
pub fn s3_storage_classes() -> Vec<String> {
    rustyfinder_remote::s3::STORAGE_CLASSES.iter().map(|s| s.to_string()).collect()
}

/// One line of server-specific facts in Get Info.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ItemDetail {
    pub label: String,
    pub value: String,
}

/// A server connection the user saved (see `rustyfinder_core::connections`).
#[derive(Debug, Clone, uniffi::Record)]
pub struct SavedConnectionRecord {
    /// 0 for a new one.
    pub id: u64,
    pub name: String,
    pub protocol: RemoteProtocol,
    pub host: String,
    pub port: Option<u16>,
    pub user: String,
    pub remote_path: String,
    pub local_path: String,
    /// `#RRGGBB`, or empty.
    pub color: String,
    pub encoding: String,
    pub passive: bool,
    pub implicit_tls: bool,
    pub region: String,
    pub path_style: Option<bool>,
    pub storage_class: String,
}

fn scheme(protocol: RemoteProtocol) -> &'static str {
    match protocol {
        RemoteProtocol::Sftp => "sftp",
        RemoteProtocol::Ftp => "ftp",
        RemoteProtocol::Ftps => "ftps",
        RemoteProtocol::WebDav => "http",
        RemoteProtocol::WebDavs => "https",
        RemoteProtocol::S3 => "s3",
        RemoteProtocol::Smb => "smb",
        RemoteProtocol::Afp => "afp",
        RemoteProtocol::Nfs => "nfs",
        RemoteProtocol::Vnc => "vnc",
    }
}

fn protocol(scheme: &str) -> RemoteProtocol {
    match scheme {
        "ftp" => RemoteProtocol::Ftp,
        "ftps" => RemoteProtocol::Ftps,
        "http" => RemoteProtocol::WebDav,
        "https" => RemoteProtocol::WebDavs,
        "s3" => RemoteProtocol::S3,
        "smb" => RemoteProtocol::Smb,
        "afp" => RemoteProtocol::Afp,
        "nfs" => RemoteProtocol::Nfs,
        "vnc" => RemoteProtocol::Vnc,
        _ => RemoteProtocol::Sftp,
    }
}

impl From<SavedConnection> for SavedConnectionRecord {
    fn from(c: SavedConnection) -> Self {
        Self {
            id: c.id,
            name: c.display_name(),
            protocol: protocol(&c.scheme),
            host: c.host,
            port: c.port,
            user: c.user,
            remote_path: c.remote_path,
            local_path: c.local_path,
            color: c.color,
            encoding: c.encoding,
            passive: c.passive,
            implicit_tls: c.implicit_tls,
            region: c.region,
            path_style: c.path_style,
            storage_class: c.storage_class,
        }
    }
}

impl From<SavedConnectionRecord> for SavedConnection {
    fn from(r: SavedConnectionRecord) -> Self {
        Self {
            id: r.id,
            name: r.name,
            scheme: scheme(r.protocol).to_string(),
            host: r.host,
            port: r.port,
            user: r.user,
            remote_path: r.remote_path,
            local_path: r.local_path,
            color: r.color,
            encoding: r.encoding,
            passive: r.passive,
            implicit_tls: r.implicit_tls,
            region: r.region,
            path_style: r.path_style,
            storage_class: r.storage_class,
        }
    }
}

#[uniffi::export]
impl RustyFinderApp {
    /// Always trust the certificate this server presents, if it is the one
    /// with `fingerprint` (the one the user saw). False if the server now
    /// presents another one, or cannot be reached.
    pub fn trust_server_certificate(
        &self,
        protocol: RemoteProtocol,
        implicit_tls: bool,
        host: String,
        port: Option<u16>,
        fingerprint: String,
    ) -> bool {
        use rustyfinder_remote::trust::{self, Handshake, TrustStore};
        let handshake = match (protocol, implicit_tls) {
            (RemoteProtocol::Ftps, false) => Handshake::FtpAuthTls,
            _ => Handshake::Direct,
        };
        let port = port.unwrap_or(default_port(protocol, implicit_tls));
        match trust::probe(&host, port, handshake) {
            Ok(der) if trust::fingerprint(&der) == fingerprint => {
                TrustStore::open(self.store.support_file(TRUST_FILE)).trust(&host, port, &der);
                true
            }
            _ => false,
        }
    }

    /// Stop trusting a server's certificate.
    pub fn forget_server_certificate(
        &self,
        protocol: RemoteProtocol,
        implicit_tls: bool,
        host: String,
        port: Option<u16>,
    ) {
        let port = port.unwrap_or(default_port(protocol, implicit_tls));
        rustyfinder_remote::trust::TrustStore::open(self.store.support_file(TRUST_FILE)).forget(&host, port);
    }

    /// The address of a remote item as a URL without any password
    /// ("Copy Path" → URL), or `None` for the local disk.
    pub fn item_url(&self, connection: u64, path: String) -> Option<String> {
        self.registry.get(connection)?.url_for(&path)
    }

    /// Facts only the item's server knows, for Get Info: (label, value),
    /// e.g. the storage class of an S3 object.
    pub fn item_details(&self, connection: u64, path: String) -> Vec<ItemDetail> {
        let Some(vfs) = self.registry.get(connection) else { return Vec::new() };
        vfs.details(&path).into_iter().map(|(label, value)| ItemDetail { label, value }).collect()
    }

    /// Saved server connections, oldest first.
    pub fn saved_connections(&self) -> Vec<SavedConnectionRecord> {
        self.saved().all().into_iter().map(Into::into).collect()
    }

    /// Save a connection (id 0: a new one); returns it as stored. The
    /// password goes to the keychain, never here.
    pub fn save_connection(&self, connection: SavedConnectionRecord) -> SavedConnectionRecord {
        self.saved().save(connection.into()).into()
    }

    pub fn remove_saved_connection(&self, id: u64) -> bool {
        self.saved().remove(id)
    }
}

impl RustyFinderApp {
    fn saved(&self) -> SavedConnections {
        SavedConnections::open(self.store.support_file(CONNECTIONS_FILE))
    }
}
