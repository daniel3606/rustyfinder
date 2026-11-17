//! WebDAV over HTTP and HTTPS, behind the core's `Vfs`.
//!
//! Every operation is one HTTP request (`PROPFIND`, `GET`, `PUT`, `MKCOL`,
//! `DELETE`, `MOVE`, `COPY`, `PROPPATCH`). The authentication scheme is
//! learned once, when connecting, and sent with every later request, so a
//! streamed upload never has to be repeated after a challenge. A password
//! is never sent in the clear (Basic over plain HTTP) unless the user
//! agreed to it after a warning.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use md5::{Digest, Md5};
use rustyfinder_core::paths;
use rustyfinder_core::{Entry, EntryKind, Vfs, VfsError, VfsResult};
use ureq::http::{Request, Response};
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::{Agent, Body, SendBody};

use crate::trust::{self, Handshake, FINGERPRINT_LABEL};
use crate::webdav_tls::PinnedTls;
use crate::RemoteError;

#[derive(Debug, Clone, Default)]
pub struct WebDavParams {
    pub host: String,
    /// `None` is 80, or 443 with `https`.
    pub port: Option<u16>,
    pub https: bool,
    /// Empty connects without credentials (anonymous shares, tokens in
    /// the path).
    pub user: String,
    pub password: String,
    /// The folder on the server that is the connection's top, e.g.
    /// `/remote.php/dav/files/bob/`.
    pub remote_path: Option<String>,
    pub accept_invalid_certs: bool,
    /// The SHA-256 fingerprint (as `trust::fingerprint` writes it) of a
    /// certificate the user chose to trust for this server: it, and nothing
    /// else, is accepted although the system cannot verify it.
    pub trusted_fingerprint: Option<String>,
    /// Send the password with Basic authentication over plain HTTP, where
    /// anyone on the way can read it. Only after the user agreed.
    pub allow_insecure_basic: bool,
}

/// The start of the error when Basic authentication would go over plain
/// HTTP; the app asks before trying again with `allow_insecure_basic`.
pub const INSECURE_BASIC: &str = "This server asks for the password without encryption";

/// How the server wants requests signed.
#[derive(Debug, Clone)]
enum Auth {
    None,
    Basic(String),
    Digest { realm: String, nonce: String, opaque: Option<String>, qop: bool },
}

pub struct WebDavFs {
    agent: Agent,
    /// `https://host:port`
    origin: String,
    params: WebDavParams,
    auth: Mutex<Auth>,
    /// Digest request counter.
    count: AtomicU32,
    label: String,
    home: String,
}

/// Metadata requests must answer within this; transfers may only stall
/// this long.
const PATIENCE: Duration = Duration::from_secs(60);

const PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?><D:propfind xmlns:D="DAV:"><D:prop><D:resourcetype/><D:getcontentlength/><D:getlastmodified/><D:creationdate/></D:prop></D:propfind>"#;

fn hex_md5(text: &str) -> String {
    Md5::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Percent-encode a path for a request line or a `Destination` header:
/// every byte outside the unreserved set, slashes kept.
pub fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 16);
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Undo percent-encoding; bytes that do not form UTF-8 are replaced.
pub fn decode_path(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    while i < bytes.len() {
        // "%" followed by two hex digits is one byte; anything else stays.
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(high << 4 | low);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Days since 1970-01-01 of a civil date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let (era, yoe) = (y.div_euclid(400), y.rem_euclid(400));
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// `Tue, 15 Nov 1994 08:12:31 GMT` (RFC 1123) as ms since the epoch.
pub fn parse_http_date(text: &str) -> Option<i64> {
    let rest = text.split_once(',').map_or(text, |(_, r)| r);
    let mut parts = rest.split_whitespace();
    let day: i64 = parts.next()?.parse().ok()?;
    let month_name = parts.next()?;
    let month = MONTHS.iter().position(|m| m.eq_ignore_ascii_case(month_name))? as i64 + 1;
    let year: i64 = parts.next()?.parse().ok()?;
    let mut clock = parts.next()?.split(':');
    let (h, m, s): (i64, i64, i64) =
        (clock.next()?.parse().ok()?, clock.next()?.parse().ok()?, clock.next().unwrap_or("0").parse().ok()?);
    Some((days_from_civil(year, month, day) * 86400 + h * 3600 + m * 60 + s) * 1000)
}

/// `2020-01-01T00:00:00Z` or with an offset (ISO 8601) as ms since the epoch.
pub fn parse_iso_date(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date, time) = text.split_once(['T', ' '])?;
    let mut d = date.split('-');
    let (year, month, day): (i64, i64, i64) =
        (d.next()?.parse().ok()?, d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    // Split off the zone: "Z", "+02:00" or "-0500".
    let zone_at = time.find(['Z', 'z', '+']).or_else(|| time.rfind('-'));
    let (clock, zone) = match zone_at {
        Some(i) => (&time[..i], &time[i..]),
        None => (time, "Z"),
    };
    let mut c = clock.split(':');
    let (h, m): (i64, i64) = (c.next()?.parse().ok()?, c.next()?.parse().ok()?);
    let s: f64 = c.next().unwrap_or("0").parse().ok()?;
    let offset = if zone.eq_ignore_ascii_case("z") {
        0
    } else {
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let digits: String = zone.chars().filter(char::is_ascii_digit).collect();
        if digits.len() < 4 {
            return None;
        }
        sign * (digits[..2].parse::<i64>().ok()? * 3600 + digits[2..4].parse::<i64>().ok()? * 60)
    };
    let secs = days_from_civil(year, month, day) * 86400 + h * 3600 + m * 60 - offset;
    Some(secs * 1000 + (s * 1000.0) as i64)
}

/// The items in a `PROPFIND` answer, as (decoded path, entry).
pub fn parse_multistatus(xml: &str) -> Option<Vec<(String, Entry)>> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let is = |node: &roxmltree::Node, name: &str| node.is_element() && node.tag_name().name() == name;
    let mut out = Vec::new();
    for response in doc.descendants().filter(|n| is(n, "response")) {
        let Some(href) = response.children().find(|n| is(n, "href")).and_then(|n| n.text()) else { continue };
        // The href may be a whole URL; only its path matters.
        let raw_path = match href.find("://") {
            Some(i) => href[i + 3..].find('/').map_or("/", |j| &href[i + 3 + j..]),
            None => href,
        };
        let path = decode_path(raw_path.trim());
        let prop = |name: &str| -> Option<roxmltree::Node> {
            response
                .descendants()
                .filter(|n| is(n, "propstat"))
                // A property the server does not have comes back under 404.
                .filter(|ps| {
                    ps.children().find(|n| is(n, "status")).and_then(|n| n.text()).is_none_or(|s| s.contains(" 200"))
                })
                .flat_map(|ps| ps.descendants().filter(|n| is(n, "prop")).collect::<Vec<_>>())
                .flat_map(|p| p.children().collect::<Vec<_>>())
                .find(|n| is(n, name))
        };
        let is_dir = prop("resourcetype").is_some_and(|n| n.children().any(|c| is(&c, "collection")));
        let size =
            prop("getcontentlength").and_then(|n| n.text()).and_then(|t| t.trim().parse::<u64>().ok()).unwrap_or(0);
        let name = paths::file_name(path.trim_end_matches('/')).to_string();
        out.push((
            path,
            Entry {
                name: name.clone(),
                kind: if is_dir { EntryKind::Directory } else { EntryKind::File },
                is_dir,
                size: if is_dir { 0 } else { size },
                modified_ms: prop("getlastmodified").and_then(|n| n.text()).and_then(parse_http_date),
                created_ms: prop("creationdate").and_then(|n| n.text()).and_then(parse_iso_date),
                is_hidden: name.starts_with('.'),
                mode: None,
                link_target: None,
                is_broken_link: false,
                ext: Default::default(),
            },
        ));
    }
    Some(out)
}

/// One field of a `WWW-Authenticate` challenge: `realm="x"`.
fn challenge_field(challenge: &str, name: &str) -> Option<String> {
    let at = challenge.find(&format!("{name}="))?;
    let rest = &challenge[at + name.len() + 1..];
    Some(match rest.strip_prefix('"') {
        Some(quoted) => quoted[..quoted.find('"')?].to_string(),
        None => rest[..rest.find([',', ' ']).unwrap_or(rest.len())].to_string(),
    })
}

fn connect_error(error: &ureq::Error) -> RemoteError {
    let text = error.to_string();
    match error {
        ureq::Error::HostNotFound => RemoteError::Unreachable("The server can’t be found.".into()),
        ureq::Error::Timeout(_) => RemoteError::Unreachable("The server did not answer in time.".into()),
        ureq::Error::Io(_) => RemoteError::Unreachable(text),
        _ if text.to_lowercase().contains("certificate") || text.to_lowercase().contains("tls") => {
            RemoteError::Certificate(text)
        }
        _ => RemoteError::Other(text),
    }
}

/// Connect: one `PROPFIND` on the top folder, which also settles how to
/// authenticate.
pub fn connect(params: WebDavParams) -> Result<WebDavFs, RemoteError> {
    // The system trust store decides, unless the user chose to go on once,
    // or trusts this server's own certificate (see `webdav_tls`).
    let tls = TlsConfig::builder()
        .provider(TlsProvider::NativeTls)
        .root_certs(RootCerts::PlatformVerifier)
        .disable_verification(params.accept_invalid_certs)
        .build();
    let config = Agent::config_builder()
        // Statuses are part of the protocol here, not errors.
        .http_status_as_error(false)
        // PROPFIND, MKCOL, MOVE and COPY are not plain HTTP methods.
        .allow_non_standard_methods(true)
        .timeout_connect(Some(PATIENCE))
        .timeout_recv_response(Some(PATIENCE))
        .timeout_per_read(Some(PATIENCE))
        .timeout_per_write(Some(PATIENCE))
        .max_redirects(5)
        .tls_config(tls)
        .build();
    let scheme = if params.https { "https" } else { "http" };
    let default_port = if params.https { 443 } else { 80 };
    let origin = match params.port {
        Some(port) if port != default_port => format!("{scheme}://{}:{port}", params.host),
        _ => format!("{scheme}://{}", params.host),
    };
    let home = match params.remote_path.as_deref() {
        Some(path) if !path.is_empty() => paths::normalize(path, "/"),
        _ => "/".to_string(),
    };
    let label = if params.user.is_empty() { params.host.clone() } else { format!("{}@{}", params.user, params.host) };
    let agent: Agent = match &params.trusted_fingerprint {
        Some(fingerprint) => {
            use ureq::unversioned::transport::{Connector, TcpConnector};
            let chain = ().chain(TcpConnector::default()).chain(PinnedTls { fingerprint: fingerprint.clone() });
            Agent::with_parts(config, chain, ureq::unversioned::resolver::DefaultResolver::default())
        }
        None => config.into(),
    };
    let fs = WebDavFs {
        agent,
        origin,
        auth: Mutex::new(Auth::None),
        count: AtomicU32::new(0),
        label,
        home: home.clone(),
        params,
    };

    let mut response = match fs.send("PROPFIND", &home, &[("Depth", "0")], Some(PROPFIND_BODY)) {
        Ok(response) => response,
        Err(e) => {
            return Err(match connect_error(&e) {
                // Name the certificate, so the user can choose to trust it.
                RemoteError::Certificate(reason) if fs.params.https => {
                    let port = fs.params.port.unwrap_or(443);
                    match trust::probe(&fs.params.host, port, Handshake::Direct) {
                        Ok(der) => RemoteError::Certificate(format!(
                            "{reason}\n{FINGERPRINT_LABEL}{}",
                            trust::fingerprint(&der)
                        )),
                        Err(_) => RemoteError::Certificate(reason),
                    }
                }
                other => other,
            });
        }
    };
    if response.status().as_u16() == 401 {
        // Learn the scheme from the challenge, then ask again.
        let challenges: Vec<String> = response
            .headers()
            .get_all("www-authenticate")
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_string))
            .collect();
        let digest = challenges.iter().find(|c| c.to_lowercase().starts_with("digest"));
        let basic = challenges.iter().any(|c| c.to_lowercase().starts_with("basic"));
        let auth = match (digest, basic) {
            _ if fs.params.user.is_empty() => {
                return Err(RemoteError::Auth("The server asks for a user name and password.".into()));
            }
            (Some(c), _) => Auth::Digest {
                realm: challenge_field(c, "realm").unwrap_or_default(),
                nonce: challenge_field(c, "nonce").unwrap_or_default(),
                opaque: challenge_field(c, "opaque"),
                qop: challenge_field(c, "qop").is_some_and(|q| q.split(',').any(|v| v.trim() == "auth")),
            },
            (None, true) if !fs.params.https && !fs.params.allow_insecure_basic => {
                return Err(RemoteError::Auth(format!(
                    "{INSECURE_BASIC} (Basic authentication over HTTP). Anyone on the network could read it."
                )));
            }
            (None, true) => Auth::Basic(base64(format!("{}:{}", fs.params.user, fs.params.password).as_bytes())),
            (None, false) => {
                return Err(RemoteError::Auth("The server wants a kind of sign-in RustyFinder does not know.".into()))
            }
        };
        *fs.auth.lock().unwrap() = auth;
        response = fs.send("PROPFIND", &home, &[("Depth", "0")], Some(PROPFIND_BODY)).map_err(|e| connect_error(&e))?;
    }
    match response.status().as_u16() {
        207 | 200 => Ok(fs),
        401 | 403 => Err(RemoteError::Auth("The server refused the user name or password.".into())),
        404 => Err(RemoteError::Other(format!("The folder “{home}” does not exist on the server."))),
        405 | 501 => Err(RemoteError::Other("This server does not speak WebDAV at that address.".into())),
        other => Err(RemoteError::Other(format!("The server answered {other}."))),
    }
}

impl WebDavFs {
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn home(&self) -> &str {
        &self.home
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.origin, encode_path(path))
    }

    /// The `Authorization` header for one request, if any is needed.
    fn authorization(&self, method: &str, path: &str) -> Option<String> {
        match &*self.auth.lock().unwrap() {
            Auth::None => None,
            Auth::Basic(token) => Some(format!("Basic {token}")),
            Auth::Digest { realm, nonce, opaque, qop } => {
                let (user, uri) = (&self.params.user, encode_path(path));
                let ha1 = hex_md5(&format!("{user}:{realm}:{}", self.params.password));
                let ha2 = hex_md5(&format!("{method}:{uri}"));
                let mut header =
                    format!("Digest username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\"");
                if *qop {
                    let nc = format!("{:08x}", self.count.fetch_add(1, Ordering::SeqCst) + 1);
                    let cnonce = hex_md5(&format!("{nc}{nonce}{uri}"));
                    let response = hex_md5(&format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}"));
                    header.push_str(&format!(", qop=auth, nc={nc}, cnonce=\"{cnonce}\", response=\"{response}\""));
                } else {
                    header.push_str(&format!(", response=\"{}\"", hex_md5(&format!("{ha1}:{nonce}:{ha2}"))));
                }
                if let Some(opaque) = opaque {
                    header.push_str(&format!(", opaque=\"{opaque}\""));
                }
                Some(header)
            }
        }
    }

    fn request(&self, method: &str, path: &str, headers: &[(&str, &str)]) -> ureq::http::request::Builder {
        let mut builder = Request::builder().method(method).uri(self.url(path));
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        if let Some(value) = self.authorization(method, path) {
            builder = builder.header("Authorization", value);
        }
        builder
    }

    /// Send a request with a small (or no) body.
    fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> Result<Response<Body>, ureq::Error> {
        let mut builder = self.request(method, path, headers);
        if body.is_some() {
            builder = builder.header("Content-Type", "application/xml; charset=utf-8");
        }
        match body {
            Some(text) => self.agent.run(builder.body(text.to_string())?),
            None => self.agent.run(builder.body(())?),
        }
    }

    /// Send, and turn everything that is not success into a `VfsError`.
    fn ok(&self, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&str>) -> VfsResult<Response<Body>> {
        let response = self
            .send(method, path, headers, body)
            .map_err(|e| VfsError::Io { path: path.to_string(), message: e.to_string() })?;
        if let Some(busy) = throttled(path, &response) {
            return Err(busy);
        }
        status_error(path, response.status().as_u16()).map_or(Ok(response), Err)
    }

    fn propfind(&self, path: &str, depth: &str) -> VfsResult<Vec<(String, Entry)>> {
        let mut response = self.ok("PROPFIND", path, &[("Depth", depth)], Some(PROPFIND_BODY))?;
        let xml = response
            .body_mut()
            .read_to_string()
            .map_err(|e| VfsError::Io { path: path.to_string(), message: e.to_string() })?;
        parse_multistatus(&xml).ok_or_else(|| VfsError::Io {
            path: path.to_string(),
            message: "The server’s answer could not be read.".into(),
        })
    }

    /// `Destination` for MOVE and COPY: an absolute, percent-encoded URL.
    fn destination(&self, to: &str, is_dir: bool) -> String {
        let mut url = self.url(to);
        if is_dir && !url.ends_with('/') {
            url.push('/');
        }
        url
    }

    /// Collections are addressed with a trailing slash (some servers
    /// redirect otherwise, and a redirected MOVE is not followed).
    fn dir_path(path: &str) -> String {
        if path.ends_with('/') {
            path.to_string()
        } else {
            format!("{path}/")
        }
    }
}

/// A "busy, try later" answer: 429, or 503 with `Retry-After` in seconds.
fn throttled(path: &str, response: &Response<Body>) -> Option<VfsError> {
    let status = response.status().as_u16();
    if status != 429 && status != 503 {
        return None;
    }
    let after = response.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok());
    if status == 503 && after.is_none() {
        return None;
    }
    let secs = after.unwrap_or(5);
    Some(VfsError::Throttled {
        path: path.to_string(),
        message: format!("The server is busy (retry after {secs} s)."),
        retry_after_secs: secs,
    })
}

/// An RFC 1123 date (as in `getlastmodified`) for ms since the epoch.
pub fn http_date(ms: i64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{}, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
        DAYS[days.rem_euclid(7) as usize],
        MONTHS[(month - 1) as usize],
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// What an HTTP status means for the item at `path`; `None` for success.
fn status_error(path: &str, status: u16) -> Option<VfsError> {
    let message = |text: &str| VfsError::Io { path: path.to_string(), message: text.to_string() };
    Some(match status {
        200..=299 => return None,
        401 | 403 => VfsError::PermissionDenied(path.to_string()),
        404 | 410 => VfsError::NotFound(path.to_string()),
        405 | 412 => VfsError::AlreadyExists(path.to_string()),
        409 => message("The folder it should go into does not exist."),
        423 => message("The item is locked by another user."),
        507 => VfsError::NoSpace,
        other => message(&format!("The server answered {other}.")),
    })
}

impl Vfs for WebDavFs {
    fn id(&self) -> String {
        format!("{}{}", self.origin, self.home)
    }

    fn list(&self, path: &str) -> VfsResult<Vec<Entry>> {
        let here = path.trim_end_matches('/');
        Ok(self
            .propfind(&Self::dir_path(path), "1")?
            .into_iter()
            // The folder itself is in the answer too: drop it.
            .filter(|(item, entry)| item.trim_end_matches('/') != here && !entry.name.is_empty())
            .map(|(_, entry)| entry)
            .collect())
    }

    fn stat(&self, path: &str) -> VfsResult<Entry> {
        let mut found = self.propfind(path, "0")?;
        let (_, mut entry) = found.drain(..).next().ok_or_else(|| VfsError::NotFound(path.to_string()))?;
        entry.name = paths::file_name(path.trim_end_matches('/')).to_string();
        Ok(entry)
    }

    fn mkdir(&self, path: &str) -> VfsResult<()> {
        self.ok("MKCOL", &Self::dir_path(path), &[], None).map(|_| ())
    }

    fn rename(&self, from: &str, to: &str, replace: bool) -> VfsResult<()> {
        let is_dir = self.stat(from)?.is_dir;
        let source = if is_dir { Self::dir_path(from) } else { from.to_string() };
        let destination = self.destination(to, is_dir);
        // "Overwrite: F" makes the server refuse (412) if the name is taken.
        let headers = [("Destination", destination.as_str()), ("Overwrite", if replace { "T" } else { "F" })];
        self.ok("MOVE", &source, &headers, None).map(|_| ()).map_err(|e| match e {
            VfsError::AlreadyExists(_) => VfsError::AlreadyExists(to.to_string()),
            other => other,
        })
    }

    fn remove_file(&self, path: &str) -> VfsResult<()> {
        self.ok("DELETE", path, &[], None).map(|_| ())
    }

    fn remove_dir(&self, path: &str) -> VfsResult<()> {
        self.ok("DELETE", &Self::dir_path(path), &[], None).map(|_| ())
    }

    fn open_read(&self, path: &str) -> VfsResult<Box<dyn Read + Send>> {
        let response = self.ok("GET", path, &[], None)?;
        Ok(Box::new(response.into_body().into_reader()))
    }

    fn resume_support(&self) -> (bool, bool) {
        // Downloads continue with a Range request; a PUT cannot append.
        (true, false)
    }

    fn open_read_at(&self, path: &str, offset: u64) -> VfsResult<Box<dyn Read + Send>> {
        let range = format!("bytes={offset}-");
        let response = self.ok("GET", path, &[("Range", range.as_str())], None)?;
        let partial = response.status().as_u16() == 206;
        let mut reader = response.into_body().into_reader();
        if !partial && offset > 0 {
            // The server ignored the range and sends it all: skip what is
            // already here. Slower, but the result is the same.
            std::io::copy(&mut (&mut reader).take(offset), &mut std::io::sink())
                .map_err(|e| VfsError::from_io(path, &e))?;
        }
        Ok(Box::new(reader))
    }

    fn create(&self, path: &str) -> VfsResult<Box<dyn Write + Send>> {
        if self.stat(path).is_ok() {
            return Err(VfsError::AlreadyExists(path.to_string()));
        }
        // The engine writes into one end of a pipe while a thread sends the
        // other end as the request body: nothing is buffered whole.
        let (reader, writer) = std::io::pipe().map_err(|e| VfsError::from_io(path, &e))?;
        let builder = self.request("PUT", path, &[("Content-Type", "application/octet-stream")]);
        let agent = self.agent.clone();
        let target = path.to_string();
        let sender = std::thread::spawn(move || -> Result<(), String> {
            // The body is made here: it must not cross threads.
            let request = builder.body(SendBody::from_owned_reader(reader)).map_err(|e| e.to_string())?;
            let response = agent.run(request).map_err(|e| e.to_string())?;
            status_error(&target, response.status().as_u16()).map_or(Ok(()), |e| Err(e.to_string()))
        });
        Ok(Box::new(Upload { writer: Some(writer), sender: Some(sender) }))
    }

    fn set_times(&self, path: &str, modified_ms: Option<i64>, _created_ms: Option<i64>) -> VfsResult<()> {
        let Some(ms) = modified_ms else { return Ok(()) };
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><D:propertyupdate xmlns:D=\"DAV:\"><D:set><D:prop>\
             <D:getlastmodified>{}</D:getlastmodified></D:prop></D:set></D:propertyupdate>",
            http_date(ms)
        );
        // Best effort: many servers keep the time of the upload instead.
        let _ = self.ok("PROPPATCH", path, &[], Some(&body));
        Ok(())
    }

    fn nfc_names(&self) -> bool {
        true
    }

    fn url_for(&self, path: &str) -> Option<String> {
        Some(self.url(path))
    }

    fn copy_within(&self, from: &str, to: &str, _progress: &mut dyn FnMut(u64) -> bool) -> VfsResult<bool> {
        // The server copies; nothing passes through this Mac.
        let destination = self.destination(to, false);
        self.ok("COPY", from, &[("Destination", destination.as_str()), ("Overwrite", "F")], None).map(|_| true).map_err(
            |e| match e {
                VfsError::AlreadyExists(_) => VfsError::AlreadyExists(to.to_string()),
                other => other,
            },
        )
    }
}

/// A `PUT` in progress. `flush` ends the body and waits for the server's
/// verdict; the copy engine flushes after the last byte.
struct Upload {
    writer: Option<std::io::PipeWriter>,
    sender: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl Upload {
    fn finish(&mut self) -> std::io::Result<()> {
        // Closing the write end tells the sending thread the body is done.
        self.writer.take();
        match self.sender.take().map(|handle| handle.join()) {
            None | Some(Ok(Ok(()))) => Ok(()),
            Some(Ok(Err(message))) => Err(std::io::Error::other(message)),
            Some(Err(_)) => Err(std::io::Error::other("the upload stopped unexpectedly")),
        }
    }
}

impl Write for Upload {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self.writer.as_mut() {
            Some(writer) => writer.write(buffer),
            None => Err(std::io::Error::other("the upload has already ended")),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.finish()
    }
}

impl Drop for Upload {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_percent_encoded_and_decoded() {
        // WD-03
        assert_eq!(encode_path("/My Docs #1/a%b?.txt"), "/My%20Docs%20%231/a%25b%3F.txt");
        assert_eq!(encode_path("/résumé"), "/r%C3%A9sum%C3%A9");
        assert_eq!(decode_path("/My%20Docs%20%231/a%25b%3F.txt"), "/My Docs #1/a%b?.txt");
        assert_eq!(decode_path("/r%C3%A9sum%C3%A9"), "/résumé");
        assert_eq!(decode_path("/100%"), "/100%", "a stray percent sign stays");
        assert_eq!(decode_path("/a%2"), "/a%2");
    }

    #[test]
    fn dates_are_written_as_http_dates() {
        assert_eq!(http_date(1_577_836_800_000), "Wed, 01 Jan 2020 00:00:00 GMT");
        assert_eq!(http_date(1_709_210_096_000), "Thu, 29 Feb 2024 12:34:56 GMT");
        assert_eq!(parse_http_date(&http_date(1_000_000_000_000)), Some(1_000_000_000_000));
    }

    #[test]
    fn dates_in_both_forms() {
        assert_eq!(parse_http_date("Wed, 01 Jan 2020 00:00:00 GMT"), Some(1_577_836_800_000));
        assert_eq!(parse_http_date("Thu, 29 Feb 2024 12:34:56 GMT"), Some(1_709_210_096_000));
        assert_eq!(parse_http_date("garbage"), None);
        assert_eq!(parse_iso_date("2020-01-01T00:00:00Z"), Some(1_577_836_800_000));
        assert_eq!(parse_iso_date("2020-01-01T02:00:00+02:00"), Some(1_577_836_800_000));
        assert_eq!(parse_iso_date("2019-12-31T19:00:00-0500"), Some(1_577_836_800_000));
        assert_eq!(parse_iso_date("2020-01-01T00:00:00.500Z"), Some(1_577_836_800_500));
        assert_eq!(parse_iso_date("not a date"), None);
    }

    #[test]
    fn a_listing_is_read_from_the_multistatus_answer() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
 <d:response><d:href>/dav/My%20Docs%20%231/</d:href>
  <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype>
   <d:getlastmodified>Wed, 01 Jan 2020 00:00:00 GMT</d:getlastmodified></d:prop>
   <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
 <d:response><d:href>http://dav.example.org/dav/My%20Docs%20%231/r%C3%A9sum%C3%A9.pdf</d:href>
  <d:propstat><d:prop><d:resourcetype/><d:getcontentlength>2048</d:getcontentlength>
   <d:getlastmodified>Wed, 01 Jan 2020 00:00:00 GMT</d:getlastmodified></d:prop>
   <d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  <d:propstat><d:prop><d:creationdate/></d:prop>
   <d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response>
</d:multistatus>"#;
        let items = parse_multistatus(xml).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].0, "/dav/My Docs #1/");
        assert!(items[0].1.is_dir && items[0].1.name == "My Docs #1");
        // WD-06: no creation date is not an error.
        let file = &items[1].1;
        assert_eq!((file.name.as_str(), file.size, file.is_dir, file.created_ms), ("résumé.pdf", 2048, false, None));
        assert_eq!(file.modified_ms, Some(1_577_836_800_000));
        assert!(parse_multistatus("<not xml").is_none());
    }

    #[test]
    fn basic_and_digest_credentials() {
        assert_eq!(base64(b"bob:secret"), "Ym9iOnNlY3JldA==");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        let challenge = r#"Digest realm="files", qop="auth,auth-int", nonce="abc123", opaque="xyz""#;
        assert_eq!(challenge_field(challenge, "realm").as_deref(), Some("files"));
        assert_eq!(challenge_field(challenge, "nonce").as_deref(), Some("abc123"));
        assert_eq!(challenge_field(challenge, "qop").as_deref(), Some("auth,auth-int"));
        assert_eq!(challenge_field("Digest realm=x, nonce=y", "nonce").as_deref(), Some("y"));
        assert_eq!(challenge_field(challenge, "missing"), None);
        // The standard MD5 test vector.
        assert_eq!(hex_md5("abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn statuses_become_the_right_errors() {
        assert!(status_error("/a", 201).is_none() && status_error("/a", 207).is_none());
        assert!(matches!(status_error("/a", 404), Some(VfsError::NotFound(_))));
        assert!(matches!(status_error("/a", 412), Some(VfsError::AlreadyExists(_))));
        assert!(matches!(status_error("/a", 405), Some(VfsError::AlreadyExists(_))));
        assert!(matches!(status_error("/a", 507), Some(VfsError::NoSpace)));
        assert!(status_error("/a", 423).unwrap().to_string().contains("locked by another user"));
    }
}
