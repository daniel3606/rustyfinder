//! A scripted WebDAV server for tests. It serves a folder on disk, speaks
//! just enough HTTP/1.1 (keep-alive, Content-Length and chunked bodies,
//! `Expect: 100-continue`, byte ranges), and logs every request. It can
//! speak HTTPS with the self-signed certificate in `support/tls`.
#![allow(dead_code)] // shared by tests and an example that use different parts

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// An anonymous share.
    None,
    Basic,
    Digest,
}

#[derive(Clone, Copy)]
pub struct Config {
    pub auth: AuthMode,
    /// Leave `creationdate` out of every answer, as some servers do.
    pub omit_creationdate: bool,
    /// Serve HTTPS with the fixture certificate.
    pub tls: bool,
    /// Honour `Range` requests on GET (else send the whole file).
    pub ranges: bool,
    /// Answer this many GETs with "503, Retry-After: 0" first.
    pub busy_gets: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config { auth: AuthMode::Basic, omit_creationdate: false, tls: false, ranges: true, busy_gets: 0 }
    }
}

/// The fixture certificate (self-signed, for "localhost" and 127.0.0.1).
pub fn acceptor() -> native_tls::TlsAcceptor {
    let p12 = fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/tls/localhost.p12")).unwrap();
    let identity = native_tls::Identity::from_pkcs12(&p12, "fixture").expect("fixture identity");
    native_tls::TlsAcceptor::new(identity).expect("acceptor")
}

/// The fixture certificate, DER bytes.
pub fn certificate() -> Vec<u8> {
    fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/tls/cert.der")).unwrap()
}

/// A connection, plain or TLS.
enum Conn {
    Plain(TcpStream),
    Tls(Box<native_tls::TlsStream<TcpStream>>),
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.read(buf),
            Conn::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.write(buf),
            Conn::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Conn::Plain(s) => s.flush(),
            Conn::Tls(s) => s.flush(),
        }
    }
}

/// Parse an RFC 1123 date as written by `http_date`.
fn parse_http_date(text: &str) -> Option<SystemTime> {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mut parts = text.split_once(',')?.1.split_whitespace();
    let d: i64 = parts.next()?.parse().ok()?;
    let month = parts.next()?;
    let mo = MONTHS.iter().position(|m| *m == month)? as i64 + 1;
    let y: i64 = parts.next()?.parse().ok()?;
    let mut clock = parts.next()?.split(':').map(|p| p.parse::<i64>().ok());
    let (h, mi, s) = (clock.next()??, clock.next()??, clock.next()??);
    let y2 = if mo <= 2 { y - 1 } else { y };
    let (era, yoe) = (y2.div_euclid(400), y2.rem_euclid(400));
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some(UNIX_EPOCH + Duration::from_secs((days * 86400 + h * 3600 + mi * 60 + s) as u64))
}

/// The one account: bob / secret.
const BASIC: &str = "Basic Ym9iOnNlY3JldA==";
const NONCE: &str = "n0nce-for-tests";

/// Listen on a free local port and serve `root`. Returns the port.
pub fn start(root: PathBuf, config: Config, log: Arc<Mutex<Vec<String>>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let busy = Arc::new(AtomicU32::new(config.busy_gets));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (log, root, busy) = (Arc::clone(&log), root.clone(), Arc::clone(&busy));
            std::thread::spawn(move || {
                let conn = if config.tls {
                    match acceptor().accept(stream) {
                        Ok(tls) => Conn::Tls(Box::new(tls)),
                        Err(_) => return,
                    }
                } else {
                    Conn::Plain(stream)
                };
                let _ = serve(conn, config, &root, &log, &busy);
            });
        }
    });
    port
}

fn civil(secs: i64) -> (i64, i64, i64, i64, i64, i64) {
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (day, month) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    (yoe + era * 400 + i64::from(month <= 2), month, day, rem / 3600, rem % 3600 / 60, rem % 60)
}

fn http_date(time: SystemTime) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (y, mo, d, h, mi, s) = civil(time.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64);
    format!("Mon, {d:02} {} {y} {h:02}:{mi:02}:{s:02} GMT", MONTHS[(mo - 1) as usize])
}

fn iso_date(time: SystemTime) -> String {
    let (y, mo, d, h, mi, s) = civil(time.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

fn encode(path: &str) -> String {
    path.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => (b as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn md5_hex(text: &str) -> String {
    use md5::{Digest, Md5};
    Md5::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn field(header: &str, name: &str) -> String {
    let Some(at) = header.find(&format!("{name}=")) else { return String::new() };
    let rest = &header[at + name.len() + 1..];
    match rest.strip_prefix('"') {
        Some(quoted) => quoted[..quoted.find('"').unwrap_or(quoted.len())].to_string(),
        None => rest[..rest.find([',', ' ']).unwrap_or(rest.len())].to_string(),
    }
}

fn authorized(config: Config, method: &str, header: Option<&String>) -> bool {
    match (config.auth, header) {
        (AuthMode::None, _) => true,
        (AuthMode::Basic, Some(value)) => value == BASIC,
        (AuthMode::Digest, Some(value)) if value.starts_with("Digest") => {
            let ha1 = md5_hex("bob:dav:secret");
            let ha2 = md5_hex(&format!("{method}:{}", field(value, "uri")));
            let expected =
                md5_hex(&format!("{ha1}:{NONCE}:{}:{}:auth:{ha2}", field(value, "nc"), field(value, "cnonce")));
            field(value, "username") == "bob" && field(value, "nonce") == NONCE && field(value, "response") == expected
        }
        _ => false,
    }
}

fn respond(out: &mut impl Write, status: &str, headers: &[(&str, String)], body: &[u8]) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    out.write_all(head.as_bytes())?;
    out.write_all(body)
}

fn propfind_entry(href: &str, path: &Path, config: Config) -> String {
    let meta = fs::metadata(path).unwrap();
    let is_dir = meta.is_dir();
    let href = if is_dir && !href.ends_with('/') { format!("{href}/") } else { href.to_string() };
    let mut props = String::new();
    props.push_str(if is_dir { "<D:resourcetype><D:collection/></D:resourcetype>" } else { "<D:resourcetype/>" });
    if !is_dir {
        props.push_str(&format!("<D:getcontentlength>{}</D:getcontentlength>", meta.len()));
    }
    props.push_str(&format!("<D:getlastmodified>{}</D:getlastmodified>", http_date(meta.modified().unwrap())));
    if !config.omit_creationdate {
        props.push_str(&format!("<D:creationdate>{}</D:creationdate>", iso_date(meta.modified().unwrap())));
    }
    format!(
        "<D:response><D:href>{}</D:href><D:propstat><D:prop>{props}</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",
        encode(&href)
    )
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        fs::create_dir(to)?;
        for item in fs::read_dir(from)? {
            let item = item?;
            copy_tree(&item.path(), &to.join(item.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

fn remove_tree(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn serve(stream: Conn, config: Config, root: &Path, log: &Mutex<Vec<String>>, busy: &AtomicU32) -> std::io::Result<()> {
    let mut input = BufReader::new(stream);
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let mut parts = line.split_whitespace();
        let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("/").to_string());
        let mut headers: Vec<(String, String)> = Vec::new();
        loop {
            let mut header = String::new();
            input.read_line(&mut header)?;
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                headers.push((name.trim().to_lowercase(), value.trim().to_string()));
            }
        }
        let header = |name: &str| headers.iter().find(|(n, _)| n == name).map(|(_, v)| v);
        if header("expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue")) {
            input.get_mut().write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        }
        // The body, whichever way its length is given.
        let mut body = Vec::new();
        if header("transfer-encoding").is_some_and(|v| v.to_lowercase().contains("chunked")) {
            loop {
                let mut size = String::new();
                input.read_line(&mut size)?;
                let n = usize::from_str_radix(size.trim().split(';').next().unwrap_or("0"), 16).unwrap_or(0);
                if n == 0 {
                    // Trailers, then the blank line.
                    loop {
                        let mut trailer = String::new();
                        input.read_line(&mut trailer)?;
                        if trailer.trim().is_empty() {
                            break;
                        }
                    }
                    break;
                }
                let mut chunk = vec![0u8; n];
                input.read_exact(&mut chunk)?;
                body.extend_from_slice(&chunk);
                let mut crlf = [0u8; 2];
                input.read_exact(&mut crlf)?;
            }
        } else if let Some(length) = header("content-length").and_then(|v| v.parse::<usize>().ok()) {
            body = vec![0u8; length];
            input.read_exact(&mut body)?;
        }

        let url_path = decode(target.split('?').next().unwrap_or("/"));
        let depth = header("depth").cloned().unwrap_or_default();
        let range = header("range").cloned().unwrap_or_default();
        let shown = if range.is_empty() { String::new() } else { format!(" range={range}") };
        log.lock().unwrap().push(format!("{method} {url_path} depth={depth}{shown}"));
        let path = root.join(url_path.trim_start_matches('/'));
        let out = input.get_mut();

        if !authorized(config, &method, header("authorization")) {
            let challenge = match config.auth {
                AuthMode::Digest => format!("Digest realm=\"dav\", qop=\"auth\", nonce=\"{NONCE}\", opaque=\"0paque\""),
                _ => "Basic realm=\"dav\"".to_string(),
            };
            respond(out, "401 Unauthorized", &[("WWW-Authenticate", challenge)], b"")?;
            continue;
        }
        // Where MOVE and COPY go: an absolute URL in the Destination header.
        let destination = header("destination").map(|url| {
            let path = url.find("://").and_then(|i| url[i + 3..].find('/').map(|j| &url[i + 3 + j..])).unwrap_or(url);
            root.join(decode(path).trim_start_matches('/').trim_end_matches('/'))
        });
        let overwrite = header("overwrite").is_none_or(|v| v != "F");
        match method.as_str() {
            "OPTIONS" => respond(out, "200 OK", &[("DAV", "1".into())], b"")?,
            "PROPFIND" => {
                if !path.exists() {
                    respond(out, "404 Not Found", &[], b"")?;
                    continue;
                }
                let mut xml =
                    String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?><D:multistatus xmlns:D=\"DAV:\">");
                xml.push_str(&propfind_entry(&url_path, &path, config));
                if depth != "0" && path.is_dir() {
                    for item in fs::read_dir(&path)?.flatten() {
                        let name = item.file_name().to_string_lossy().into_owned();
                        let href = format!("{}/{name}", url_path.trim_end_matches('/'));
                        xml.push_str(&propfind_entry(&href, &item.path(), config));
                    }
                }
                xml.push_str("</D:multistatus>");
                respond(out, "207 Multi-Status", &[("Content-Type", "application/xml".into())], xml.as_bytes())?;
            }
            "GET" if busy.load(Ordering::SeqCst) > 0 => {
                busy.fetch_sub(1, Ordering::SeqCst);
                respond(out, "503 Service Unavailable", &[("Retry-After", "0".into())], b"")?;
            }
            "GET" => match fs::read(&path) {
                Ok(bytes) => {
                    let from = range
                        .strip_prefix("bytes=")
                        .and_then(|r| r.strip_suffix('-'))
                        .and_then(|n| n.parse::<usize>().ok());
                    match from {
                        Some(from) if config.ranges && from <= bytes.len() => {
                            let content_range = format!("bytes {from}-{}/{}", bytes.len().max(1) - 1, bytes.len());
                            respond(out, "206 Partial Content", &[("Content-Range", content_range)], &bytes[from..])?
                        }
                        _ => respond(out, "200 OK", &[], &bytes)?,
                    }
                }
                Err(_) => respond(out, "404 Not Found", &[], b"")?,
            },
            "PROPPATCH" => {
                let text = String::from_utf8_lossy(&body).into_owned();
                let date = text
                    .split_once("getlastmodified>")
                    .and_then(|(_, rest)| rest.split_once('<'))
                    .and_then(|(value, _)| parse_http_date(value));
                match (date, fs::File::options().write(true).open(&path)) {
                    (Some(time), Ok(file)) => {
                        file.set_modified(time)?;
                        let xml = format!(
                            "<?xml version=\"1.0\"?><D:multistatus xmlns:D=\"DAV:\"><D:response><D:href>{}</D:href>\
                             <D:propstat><D:prop><D:getlastmodified/></D:prop><D:status>HTTP/1.1 200 OK</D:status>\
                             </D:propstat></D:response></D:multistatus>",
                            encode(&url_path)
                        );
                        respond(out, "207 Multi-Status", &[], xml.as_bytes())?
                    }
                    _ => respond(out, "409 Conflict", &[], b"")?,
                }
            }
            "PUT" => {
                if !path.parent().is_some_and(Path::is_dir) {
                    respond(out, "409 Conflict", &[], b"")?;
                } else {
                    fs::write(&path, &body)?;
                    respond(out, "201 Created", &[], b"")?;
                }
            }
            "MKCOL" => {
                if path.exists() {
                    respond(out, "405 Method Not Allowed", &[], b"")?;
                } else if fs::create_dir(&path).is_ok() {
                    respond(out, "201 Created", &[], b"")?;
                } else {
                    respond(out, "409 Conflict", &[], b"")?;
                }
            }
            "DELETE" => {
                if remove_tree(&path).is_ok() {
                    respond(out, "204 No Content", &[], b"")?;
                } else {
                    respond(out, "404 Not Found", &[], b"")?;
                }
            }
            "MOVE" | "COPY" => {
                let Some(to) = destination else {
                    respond(out, "400 Bad Request", &[], b"")?;
                    continue;
                };
                if !path.exists() {
                    respond(out, "404 Not Found", &[], b"")?;
                } else if to.exists() && !overwrite {
                    respond(out, "412 Precondition Failed", &[], b"")?;
                } else {
                    let replaced = to.exists();
                    if replaced {
                        remove_tree(&to)?;
                    }
                    if method == "MOVE" {
                        fs::rename(&path, &to)?
                    } else {
                        copy_tree(&path, &to)?
                    }
                    respond(out, if replaced { "204 No Content" } else { "201 Created" }, &[], b"")?;
                }
            }
            _ => respond(out, "405 Method Not Allowed", &[], b"")?,
        }
    }
}
