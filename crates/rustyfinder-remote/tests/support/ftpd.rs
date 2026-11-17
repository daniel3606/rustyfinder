//! A scripted FTP server for tests. It keeps its files in a folder on
//! disk and logs every command it receives, so a test can check both what
//! ended up on "the server" and what was sent to it. It can speak TLS,
//! explicit (`AUTH TLS`) or implicit, with the self-signed certificate in
//! `support/tls`.
#![allow(dead_code)] // shared by tests and an example that use different parts

use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tls {
    /// `AUTH TLS` after the greeting.
    Explicit,
    /// TLS from the first byte.
    Implicit,
}

#[derive(Clone, Copy)]
pub struct Config {
    pub utf8: bool,
    pub mlsd: bool,
    pub mfmt: bool,
    pub epsv: bool,
    /// Answer PASV with an address that cannot be reached (a NAT'd server).
    pub private_pasv: bool,
    /// Windows/IIS style LIST lines.
    pub dos_list: bool,
    /// EPLF LIST lines.
    pub eplf: bool,
    /// Encoding of names on the wire when not UTF-8.
    pub legacy: Option<&'static encoding_rs::Encoding>,
    /// `REST STREAM` in FEAT, and REST honoured.
    pub rest: bool,
    /// `SITE CHMOD` works (else "500 Unknown SITE command").
    pub site_chmod: bool,
    /// `SITE UTIME path atime mtime ctime UTC` works.
    pub site_utime: bool,
    /// RETR opens the data connection and then sends nothing at all.
    pub stall_retr: bool,
    pub tls: Option<Tls>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            utf8: true,
            mlsd: true,
            mfmt: true,
            epsv: true,
            private_pasv: false,
            dos_list: false,
            eplf: false,
            legacy: None,
            rest: true,
            site_chmod: true,
            site_utime: false,
            stall_retr: false,
            tls: None,
        }
    }
}

/// The fixture certificate (self-signed, for "localhost" and 127.0.0.1).
pub fn acceptor() -> native_tls::TlsAcceptor {
    static IDENTITY: OnceLock<Vec<u8>> = OnceLock::new();
    let p12 = IDENTITY.get_or_init(|| {
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/tls/localhost.p12")).unwrap()
    });
    let identity = native_tls::Identity::from_pkcs12(p12, "fixture").expect("fixture identity");
    native_tls::TlsAcceptor::new(identity).expect("acceptor")
}

/// The fixture certificate, DER bytes.
pub fn certificate() -> Vec<u8> {
    fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/tls/cert.der")).unwrap()
}

/// A connection, plain or upgraded to TLS.
enum Conn {
    Plain(TcpStream),
    Tls(Box<native_tls::TlsStream<TcpStream>>),
    /// Only while upgrading.
    Gone,
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.read(buf),
            Conn::Tls(s) => s.read(buf),
            Conn::Gone => Ok(0),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.write(buf),
            Conn::Tls(s) => s.write(buf),
            Conn::Gone => Err(std::io::Error::other("gone")),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Conn::Plain(s) => s.flush(),
            Conn::Tls(s) => s.flush(),
            Conn::Gone => Ok(()),
        }
    }
}

impl Conn {
    fn close(self) {
        if let Conn::Tls(mut s) = self {
            let _ = s.shutdown();
        }
    }
}

fn timestamp(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (day, month) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn parse_timestamp(text: &str) -> SystemTime {
    let n = |range: std::ops::Range<usize>| text[range].parse::<i64>().unwrap();
    let (y, m, d) = (n(0..4), n(4..6), n(6..8));
    let y2 = if m <= 2 { y - 1 } else { y };
    let (era, yoe) = (y2.div_euclid(400), y2.rem_euclid(400));
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86400 + n(8..10) * 3600 + n(10..12) * 60 + n(12..14);
    UNIX_EPOCH + Duration::from_secs(secs as u64)
}

/// Listen on a free local port and serve `root`. Returns the port.
pub fn start(root: PathBuf, config: Config, log: Arc<Mutex<Vec<String>>>, drop_next: Arc<AtomicBool>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (log, root, drop_next) = (Arc::clone(&log), root.clone(), Arc::clone(&drop_next));
            std::thread::spawn(move || {
                let _ = serve(stream, config, &root, &log, &drop_next);
            });
        }
    });
    port
}

fn serve(
    stream: TcpStream,
    config: Config,
    root: &Path,
    log: &Mutex<Vec<String>>,
    drop_next: &AtomicBool,
) -> std::io::Result<()> {
    let first = match config.tls {
        Some(Tls::Implicit) => Conn::Tls(Box::new(acceptor().accept(stream).map_err(std::io::Error::other)?)),
        _ => Conn::Plain(stream),
    };
    let mut input = BufReader::new(first);
    let encode = |text: &str| -> Vec<u8> {
        match config.legacy {
            Some(e) => e.encode(text).0.into_owned(),
            None => text.as_bytes().to_vec(),
        }
    };
    let reply = |input: &mut BufReader<Conn>, text: &str| -> std::io::Result<()> {
        let mut bytes = encode(text);
        bytes.extend_from_slice(b"\r\n");
        input.get_mut().write_all(&bytes)
    };
    reply(&mut input, "220 scripted FTP server")?;
    let mut cwd = String::from("/");
    let mut data: Option<TcpListener> = None;
    let mut rename_from: Option<PathBuf> = None;
    let mut rest: u64 = 0;
    let mut logged_in = false;
    let mut protect_data = false;
    // Data connections: accepted, and wrapped in TLS after PROT P.
    let open_data = |listener: TcpListener, protect: bool| -> std::io::Result<Conn> {
        let (conn, _) = listener.accept()?;
        if protect {
            Ok(Conn::Tls(Box::new(acceptor().accept(conn).map_err(std::io::Error::other)?)))
        } else {
            Ok(Conn::Plain(conn))
        }
    };
    loop {
        let mut raw = Vec::new();
        if input.read_until(b'\n', &mut raw)? == 0 {
            return Ok(());
        }
        let line = match config.legacy {
            Some(e) => e.decode(&raw).0.into_owned(),
            None => String::from_utf8_lossy(&raw).into_owned(),
        };
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        log.lock().unwrap().push(line.clone());
        if drop_next.swap(false, Ordering::SeqCst) {
            return Ok(()); // the connection "timed out"
        }
        let (verb, arg) = match line.split_once(' ') {
            Some((v, a)) => (v.to_uppercase(), a.to_string()),
            None => (line.to_uppercase(), String::new()),
        };
        let resolve = |arg: &str| -> PathBuf {
            let full =
                if arg.starts_with('/') { arg.to_string() } else { format!("{}/{arg}", cwd.trim_end_matches('/')) };
            root.join(full.trim_start_matches('/'))
        };
        if !logged_in && !matches!(verb.as_str(), "USER" | "PASS" | "QUIT" | "FEAT" | "AUTH" | "PBSZ" | "PROT") {
            reply(&mut input, "530 Please log in")?;
            continue;
        }
        match verb.as_str() {
            "AUTH" if config.tls == Some(Tls::Explicit) => {
                reply(&mut input, "234 AUTH TLS OK")?;
                let plain = std::mem::replace(input.get_mut(), Conn::Gone);
                let Conn::Plain(tcp) = plain else { return Ok(()) };
                *input.get_mut() = Conn::Tls(Box::new(acceptor().accept(tcp).map_err(std::io::Error::other)?));
            }
            "PBSZ" => reply(&mut input, "200 PBSZ=0")?,
            "PROT" => {
                protect_data = arg.eq_ignore_ascii_case("P");
                reply(&mut input, "200 Protection set")?;
            }
            "USER" => reply(&mut input, "331 Password required")?,
            "PASS" => {
                logged_in = arg == "secret";
                reply(&mut input, if logged_in { "230 Logged in" } else { "530 Login incorrect" })?;
            }
            "FEAT" => {
                let mut text = String::from("211-Features:\r\n");
                if config.utf8 {
                    text.push_str(" UTF8\r\n");
                }
                if config.mlsd {
                    text.push_str(" MLSD\r\n MLST type*;size*;modify*;\r\n");
                }
                if config.mfmt {
                    text.push_str(" MFMT\r\n");
                }
                if config.rest {
                    text.push_str(" REST STREAM\r\n");
                }
                text.push_str("211 End");
                reply(&mut input, &text)?;
            }
            "OPTS" | "TYPE" | "NOOP" => reply(&mut input, "200 OK")?,
            "PWD" => reply(&mut input, &format!("257 \"{cwd}\" is the current directory"))?,
            "CWD" => {
                if resolve(&arg).is_dir() {
                    cwd =
                        if arg.starts_with('/') { arg.clone() } else { format!("{}/{arg}", cwd.trim_end_matches('/')) };
                    reply(&mut input, "250 OK")?;
                } else {
                    reply(&mut input, "550 No such directory")?;
                }
            }
            "EPSV" if config.epsv => {
                let listener = TcpListener::bind("127.0.0.1:0")?;
                let port = listener.local_addr()?.port();
                data = Some(listener);
                reply(&mut input, &format!("229 Entering Extended Passive Mode (|||{port}|)"))?;
            }
            "PASV" => {
                let listener = TcpListener::bind("127.0.0.1:0")?;
                let port = listener.local_addr()?.port();
                data = Some(listener);
                let host = if config.private_pasv { "10,255,255,1" } else { "127,0,0,1" };
                reply(&mut input, &format!("227 Entering Passive Mode ({host},{},{})", port >> 8, port & 0xff))?;
            }
            "REST" if config.rest => {
                rest = arg.parse().unwrap_or(0);
                reply(&mut input, "350 Restarting")?;
            }
            "LIST" | "MLSD" => {
                let target = if arg.is_empty() || arg.starts_with('-') { resolve("") } else { resolve(&arg) };
                let Ok(reader) = fs::read_dir(&target) else {
                    reply(&mut input, "550 No such directory")?;
                    continue;
                };
                let mut text = String::new();
                if verb == "LIST" && !config.dos_list && !config.eplf {
                    text.push_str("total 8\r\n");
                }
                for item in reader.flatten() {
                    let name = item.file_name().to_string_lossy().into_owned();
                    let meta = item.metadata()?;
                    let secs = meta.modified()?.duration_since(UNIX_EPOCH).unwrap().as_secs();
                    let line = if verb == "MLSD" {
                        let kind = if meta.is_dir() { "dir" } else { "file" };
                        format!("type={kind};modify={};size={}; {name}", timestamp(meta.modified()?), meta.len())
                    } else if config.eplf {
                        if meta.is_dir() {
                            format!("+i{secs}.1,m{secs},/,\t{name}")
                        } else {
                            format!("+i{secs}.2,m{secs},r,s{},up644,\t{name}", meta.len())
                        }
                    } else if config.dos_list {
                        if meta.is_dir() {
                            format!("01-15-20  10:30AM       <DIR>          {name}")
                        } else {
                            format!("01-15-20  10:30AM  {:>18} {name}", meta.len())
                        }
                    } else {
                        let kind = if meta.is_dir() { "drwxr-xr-x" } else { "-rw-r--r--" };
                        format!("{kind} 1 bob staff {:>8} Jan 15  2020 {name}", meta.len())
                    };
                    text.push_str(&line);
                    text.push_str("\r\n");
                }
                let Some(listener) = data.take() else {
                    reply(&mut input, "425 Use PASV first")?;
                    continue;
                };
                reply(&mut input, "150 Here comes the listing")?;
                let mut conn = open_data(listener, protect_data)?;
                conn.write_all(&encode(&text))?;
                conn.close();
                reply(&mut input, "226 Transfer complete")?;
            }
            "MLST" => {
                let path = resolve(&arg);
                match fs::metadata(&path) {
                    Ok(meta) => {
                        let kind = if meta.is_dir() { "dir" } else { "file" };
                        let facts = format!("type={kind};size={};modify={};", meta.len(), timestamp(meta.modified()?));
                        reply(&mut input, &format!("250-Listing\r\n {facts} {arg}\r\n250 End"))?;
                    }
                    Err(_) => reply(&mut input, "550 No such file")?,
                }
            }
            "RETR" => {
                let Ok(bytes) = fs::read(resolve(&arg)) else {
                    reply(&mut input, "550 No such file")?;
                    continue;
                };
                let Some(listener) = data.take() else {
                    reply(&mut input, "425 Use PASV first")?;
                    continue;
                };
                reply(&mut input, "150 Opening data connection")?;
                if config.stall_retr {
                    // Never accept, never send: the client's data timeout
                    // has to end this.
                    std::thread::sleep(Duration::from_secs(30));
                    return Ok(());
                }
                let mut conn = open_data(listener, protect_data)?;
                // A stopped download closes the connection early.
                let _ = conn.write_all(&bytes[(rest as usize).min(bytes.len())..]);
                rest = 0;
                conn.close();
                reply(&mut input, "226 Transfer complete")?;
            }
            "STOR" | "APPE" => {
                let Some(listener) = data.take() else {
                    reply(&mut input, "425 Use PASV first")?;
                    continue;
                };
                reply(&mut input, "150 Ready for data")?;
                let mut conn = open_data(listener, protect_data)?;
                let mut bytes = Vec::new();
                // A stopped upload ends early; keep what arrived.
                let _ = conn.read_to_end(&mut bytes);
                let path = resolve(&arg);
                if verb == "APPE" {
                    fs::OpenOptions::new().append(true).create(true).open(&path)?.write_all(&bytes)?;
                } else if rest > 0 {
                    let mut file = fs::OpenOptions::new().write(true).open(&path)?;
                    file.seek(SeekFrom::Start(rest))?;
                    file.write_all(&bytes)?;
                    rest = 0;
                } else {
                    fs::write(&path, bytes)?;
                }
                reply(&mut input, "226 Transfer complete")?;
            }
            "MKD" => match fs::create_dir(resolve(&arg)) {
                Ok(()) => reply(&mut input, &format!("257 \"{arg}\" created"))?,
                Err(_) => reply(&mut input, "550 Create directory operation failed")?,
            },
            "RMD" => match fs::remove_dir(resolve(&arg)) {
                Ok(()) => reply(&mut input, "250 Removed")?,
                Err(_) => reply(&mut input, "550 Remove directory operation failed")?,
            },
            "DELE" => match fs::remove_file(resolve(&arg)) {
                Ok(()) => reply(&mut input, "250 Deleted")?,
                Err(_) => reply(&mut input, "550 Delete operation failed")?,
            },
            "RNFR" => {
                let path = resolve(&arg);
                if path.exists() {
                    rename_from = Some(path);
                    reply(&mut input, "350 Ready for RNTO")?;
                } else {
                    reply(&mut input, "550 No such file")?;
                }
            }
            "RNTO" => match rename_from.take().map(|from| fs::rename(from, resolve(&arg))) {
                Some(Ok(())) => reply(&mut input, "250 Renamed")?,
                _ => reply(&mut input, "550 Rename failed")?,
            },
            "MFMT" if config.mfmt => {
                let (stamp, path) = arg.split_once(' ').unwrap_or((&arg, ""));
                fs::File::options().write(true).open(resolve(path))?.set_modified(parse_timestamp(stamp))?;
                reply(&mut input, &format!("213 Modify={stamp}; {path}"))?;
            }
            "SITE" => {
                let mut words = arg.split(' ');
                match (words.next().map(str::to_uppercase).as_deref(), config) {
                    (Some("CHMOD"), Config { site_chmod: true, .. }) => {
                        use std::os::unix::fs::PermissionsExt;
                        let mode = u32::from_str_radix(words.next().unwrap_or(""), 8).unwrap_or(0o644);
                        let path = words.collect::<Vec<_>>().join(" ");
                        fs::set_permissions(resolve(&path), fs::Permissions::from_mode(mode))?;
                        reply(&mut input, "200 SITE CHMOD command successful")?;
                    }
                    (Some("UTIME"), Config { site_utime: true, .. }) => {
                        // SITE UTIME <path> <atime> <mtime> <ctime> UTC
                        let rest: Vec<&str> = words.collect();
                        if rest.len() < 5 || rest[rest.len() - 1] != "UTC" {
                            reply(&mut input, "501 Bad arguments")?;
                            continue;
                        }
                        let path = rest[..rest.len() - 4].join(" ");
                        let mtime = parse_timestamp(rest[rest.len() - 3]);
                        fs::File::options().write(true).open(resolve(&path))?.set_modified(mtime)?;
                        reply(&mut input, "200 UTIME OK")?;
                    }
                    _ => reply(&mut input, "500 Unknown SITE command")?,
                }
            }
            "ABOR" => reply(&mut input, "226 Aborted")?,
            "QUIT" => {
                reply(&mut input, "221 Bye")?;
                return Ok(());
            }
            _ => reply(&mut input, "502 Command not implemented")?,
        }
    }
}
