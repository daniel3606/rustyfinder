//! Server certificates the user chose to trust (FTP over TLS, WebDAV over
//! HTTPS; remote-ftp spec §2).
//!
//! When the system cannot verify a server's certificate (self-signed, wrong
//! host, expired), the connection fails with the certificate's SHA-256
//! fingerprint in the message. The user may then go on once, or "always
//! trust" it: the certificate is kept in a small file in the app's support
//! folder, per host and port, and later connections accept exactly that
//! certificate and no other.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use sha2::{Digest, Sha256};

/// How the TLS handshake starts on a server's port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handshake {
    /// TLS from the first byte (HTTPS, implicit FTPS).
    Direct,
    /// FTP's `AUTH TLS` first (explicit FTPS).
    FtpAuthTls,
}

/// Label before the fingerprint in connection error messages.
pub const FINGERPRINT_LABEL: &str = "SHA-256 fingerprint: ";

/// `AA:2B:…` for a DER-encoded certificate.
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// The fingerprint in a connection error message, if there is one.
pub fn fingerprint_in(message: &str) -> Option<String> {
    let at = message.find(FINGERPRINT_LABEL)?;
    let rest = &message[at + FINGERPRINT_LABEL.len()..];
    let end = rest.find(|c: char| !(c.is_ascii_hexdigit() || c == ':')).unwrap_or(rest.len());
    (end > 0).then(|| rest[..end].to_string())
}

/// Fetch the certificate a server presents, without judging it.
pub fn probe(host: &str, port: u16, handshake: Handshake) -> Result<Vec<u8>, String> {
    let timeout = Duration::from_secs(30);
    // "localhost" may name ::1 first while the server listens on 127.0.0.1
    // only: try each address, as the connection itself did.
    let mut last = format!("“{host}” has no address.");
    let mut tcp = None;
    for addr in (host, port).to_socket_addrs().map_err(|e| e.to_string())? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => {
                tcp = Some(stream);
                break;
            }
            Err(e) => last = e.to_string(),
        }
    }
    let tcp = tcp.ok_or(last)?;
    tcp.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    if handshake == Handshake::FtpAuthTls {
        let mut reader = BufReader::new(tcp.try_clone().map_err(|e| e.to_string())?);
        let reply = |reader: &mut BufReader<TcpStream>| -> Result<String, String> {
            // Multi-line replies end with "NNN " (a space after the code).
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                    return Err("The server closed the connection.".into());
                }
                if line.len() >= 4 && line.as_bytes()[3] == b' ' {
                    return Ok(line);
                }
            }
        };
        reply(&mut reader)?;
        (&tcp).write_all(b"AUTH TLS\r\n").map_err(|e| e.to_string())?;
        let answer = reply(&mut reader)?;
        if !answer.starts_with("234") {
            return Err(format!("The server does not offer TLS: {}", answer.trim()));
        }
    }
    let connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
        .map_err(|e| e.to_string())?;
    let stream = connector.connect(host, tcp).map_err(|e| e.to_string())?;
    let cert = stream.peer_certificate().map_err(|e| e.to_string())?.ok_or("The server sent no certificate.")?;
    cert.to_der().map_err(|e| e.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Trusted {
    host: String,
    port: u16,
    /// The certificate, DER bytes in hex.
    der: String,
}

/// The certificates the user chose to trust, kept in a JSON file.
#[derive(Debug)]
pub struct TrustStore {
    file: PathBuf,
    entries: Mutex<Vec<Trusted>>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

impl TrustStore {
    pub fn open(file: impl Into<PathBuf>) -> Self {
        let file = file.into();
        let entries =
            std::fs::read(&file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Self { file, entries: Mutex::new(entries) }
    }

    /// The certificate trusted for this server, as DER bytes.
    pub fn certificate(&self, host: &str, port: u16) -> Option<Vec<u8>> {
        let entries = self.entries.lock().unwrap();
        entries.iter().find(|t| t.host.eq_ignore_ascii_case(host) && t.port == port).and_then(|t| unhex(&t.der))
    }

    /// Always trust `der` for this server (replacing an earlier one).
    pub fn trust(&self, host: &str, port: u16, der: &[u8]) {
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|t| !(t.host.eq_ignore_ascii_case(host) && t.port == port));
        entries.push(Trusted { host: host.to_string(), port, der: hex(der) });
        self.save(&entries);
    }

    pub fn forget(&self, host: &str, port: u16) {
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|t| !(t.host.eq_ignore_ascii_case(host) && t.port == port));
        self.save(&entries);
    }

    fn save(&self, entries: &[Trusted]) {
        let Ok(bytes) = serde_json::to_vec_pretty(entries) else { return };
        let tmp = self.file.with_extension("json.tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, &self.file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_are_found_in_messages() {
        let fp = fingerprint(b"certificate");
        assert_eq!(fp.len(), 32 * 3 - 1);
        let message = format!("The server’s certificate can’t be verified. Self-signed.\n{FINGERPRINT_LABEL}{fp}");
        assert_eq!(fingerprint_in(&message), Some(fp));
        assert_eq!(fingerprint_in("nothing here"), None);
    }

    #[test]
    fn trusted_certificates_are_kept_per_server() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("trusted.json");
        let store = TrustStore::open(&file);
        store.trust("Files.Example.org", 990, &[1, 2, 255]);
        assert_eq!(TrustStore::open(&file).certificate("files.example.org", 990), Some(vec![1, 2, 255]));
        assert_eq!(store.certificate("files.example.org", 21), None, "another port is another server");
        store.trust("files.example.org", 990, &[9]);
        assert_eq!(store.certificate("files.example.org", 990), Some(vec![9]));
        store.forget("files.example.org", 990);
        assert_eq!(TrustStore::open(&file).certificate("files.example.org", 990), None);
    }
}
