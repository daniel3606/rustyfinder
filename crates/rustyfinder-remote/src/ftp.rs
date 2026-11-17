//! FTP and FTP over TLS, behind the core's `Vfs`.
//!
//! One control connection carries one command at a time, and a transfer
//! keeps it busy until it ends. So the backend keeps a small pool of
//! logged-in sessions: each call borrows one (opening another if all are
//! busy), and a download or upload holds its session until it finishes.
//! A session whose connection has gone stale is dropped and the command
//! is retried once on a fresh one. Idle sessions get a `NOOP` now and then
//! so the server does not close them.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use rustyfinder_core::paths;
use rustyfinder_core::{Entry, EntryKind, Vfs, VfsError, VfsResult};
use suppaftp::list::{File, ListParser, PosixPexQuery};
use suppaftp::native_tls::TlsConnector as NativeConnector;
use suppaftp::types::FileType;
use suppaftp::{FtpError, FtpResult, Mode, NativeTlsConnector, NativeTlsFtpStream, Status};

use crate::trust::{self, Handshake, FINGERPRINT_LABEL};
use crate::RemoteError;

type Session = NativeTlsFtpStream;
// The crate keeps its TLS stream type private; the connector names it.
type TlsStream = <NativeTlsConnector as suppaftp::TlsConnector>::Stream;
type Transfer = suppaftp::TransferStream<TlsStream>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FtpSecurity {
    /// Plain FTP.
    #[default]
    None,
    /// `AUTH TLS` after connecting; the data channel is protected too.
    Explicit,
    /// TLS from the first byte ("FTPS", port 990).
    Implicit,
}

#[derive(Debug, Clone, Default)]
pub struct FtpParams {
    pub host: String,
    /// `None` is the standard port: 21, or 990 for implicit TLS.
    pub port: Option<u16>,
    /// Empty logs in as "anonymous".
    pub user: String,
    pub password: String,
    pub security: FtpSecurity,
    /// Passive mode (the default). Off: the server connects back.
    pub passive: bool,
    /// Name encoding: `None` is automatic, else a label such as
    /// "windows-1251".
    pub encoding: Option<String>,
    /// Folder to start in; `None` is where the server puts the user.
    pub remote_path: Option<String>,
    /// Go on although the server's certificate cannot be verified.
    pub accept_invalid_certs: bool,
    /// Accept exactly this certificate (its SHA-256 fingerprint, as
    /// `trust::fingerprint` writes it), which the user chose to trust,
    /// although the system cannot verify it. Checked on every connection.
    pub trusted_fingerprint: Option<String>,
    /// A data connection that does not open, or stalls, this long fails
    /// the transfer. `None` is 30 s.
    pub data_timeout: Option<Duration>,
    /// Idle sessions are kept alive with a `NOOP` this often. `None` is 60 s.
    pub keepalive: Option<Duration>,
}

impl FtpParams {
    fn port(&self) -> u16 {
        self.port.unwrap_or(if self.security == FtpSecurity::Implicit { 990 } else { 21 })
    }
}

/// How long the control connection may take to open, or wait for a reply.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
const DATA_TIMEOUT: Duration = Duration::from_secs(30);
const KEEPALIVE: Duration = Duration::from_secs(60);

/// File names on servers that do not speak UTF-8. Bytes that are valid
/// UTF-8 are taken as such; otherwise the encoding is guessed once (from
/// the first such bytes seen) and used both ways from then on, so a name
/// is sent back as the same bytes it arrived in.
#[derive(Debug)]
struct NameEncoding {
    legacy: Mutex<Option<&'static encoding_rs::Encoding>>,
}

impl NameEncoding {
    fn new(label: Option<&str>) -> Self {
        let fixed =
            label.and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes())).filter(|e| *e != encoding_rs::UTF_8);
        Self { legacy: Mutex::new(fixed) }
    }
}

impl suppaftp::Encoding for NameEncoding {
    fn decode(&self, bytes: &[u8]) -> String {
        if let Ok(text) = std::str::from_utf8(bytes) {
            return text.to_string();
        }
        let mut legacy = self.legacy.lock().unwrap();
        let encoding = *legacy.get_or_insert_with(|| {
            let mut detector = EncodingDetector::new(Iso2022JpDetection::Allow);
            detector.feed(bytes, true);
            detector.guess(None, Utf8Detection::Deny)
        });
        encoding.decode(bytes).0.into_owned()
    }

    fn encode(&self, text: &str) -> std::io::Result<Vec<u8>> {
        match *self.legacy.lock().unwrap() {
            // Plain ASCII is the same in every encoding offered here.
            Some(encoding) if !text.is_ascii() => {
                let (bytes, _, unmappable) = encoding.encode(text);
                if unmappable {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "the name has characters the server's encoding cannot hold",
                    ));
                }
                Ok(bytes.into_owned())
            }
            _ => Ok(text.as_bytes().to_vec()),
        }
    }
}

/// Forwards to the shared encoding (the client wants to own its copy).
#[derive(Debug)]
struct SharedEncoding(Arc<NameEncoding>);

impl suppaftp::Encoding for SharedEncoding {
    fn decode(&self, bytes: &[u8]) -> String {
        self.0.decode(bytes)
    }
    fn encode(&self, text: &str) -> std::io::Result<Vec<u8>> {
        self.0.encode(text)
    }
}

/// TLS for the control and data connections. With a pinned fingerprint
/// the system's verdict is set aside, and only that one certificate is
/// accepted; this is checked on every connection, data ones included.
#[derive(Debug)]
struct Tls {
    inner: NativeTlsConnector,
    pin: Option<String>,
}

impl Tls {
    fn new(params: &FtpParams) -> Result<Self, RemoteError> {
        let lenient = params.accept_invalid_certs || params.trusted_fingerprint.is_some();
        let connector = NativeConnector::builder()
            .danger_accept_invalid_certs(lenient)
            .danger_accept_invalid_hostnames(lenient)
            .build()
            .map_err(|e| RemoteError::Certificate(e.to_string()))?;
        Ok(Self { inner: NativeTlsConnector::from(connector), pin: params.trusted_fingerprint.clone() })
    }
}

impl suppaftp::TlsConnector for Tls {
    type Stream = TlsStream;

    fn connect(&self, domain: &str, stream: TcpStream) -> FtpResult<TlsStream> {
        use suppaftp::TlsStream as _;
        let mut tls = self.inner.connect(domain, stream)?;
        if let Some(pin) = &self.pin {
            let presented = tls
                .mut_ref()
                .peer_certificate()
                .ok()
                .flatten()
                .and_then(|c| c.to_der().ok())
                .map(|d| trust::fingerprint(&d));
            if presented.as_deref() != Some(pin.as_str()) {
                return Err(FtpError::SecureError("The certificate is not the one you chose to trust.".into()));
            }
        }
        Ok(tls)
    }
}

struct Shared {
    params: FtpParams,
    /// Logged-in sessions nobody is using, and since when.
    idle: Mutex<Vec<(Session, Instant)>>,
    /// What the server said it can do (`FEAT`), upper-cased keys.
    features: HashMap<String, Option<String>>,
    names: Arc<NameEncoding>,
    /// Extended passive mode was refused once: use plain `PASV`.
    plain_passive: AtomicBool,
    /// The last folder listed, kept briefly so a run of `stat` calls on
    /// one folder (no `MLST` on the server) costs one listing.
    recent: Mutex<Option<(String, Instant, Vec<Entry>)>>,
}

pub struct FtpFs {
    shared: Arc<Shared>,
    label: String,
    home: String,
}

fn remote_error(error: &FtpError) -> RemoteError {
    match error {
        FtpError::UnexpectedResponse(response) if response.status == Status::NotLoggedIn => {
            RemoteError::Auth("The server refused the user name or password.".into())
        }
        FtpError::SecureError(message) => RemoteError::Certificate(message.clone()),
        FtpError::ConnectionError(e) => RemoteError::Unreachable(e.to_string()),
        other => RemoteError::Other(other.to_string()),
    }
}

/// Open and log in one session.
fn open_session(p: &FtpParams, names: &Arc<NameEncoding>, plain_passive: bool) -> Result<Session, RemoteError> {
    let addr = (p.host.as_str(), p.port())
        .to_socket_addrs()
        .map_err(|e| RemoteError::Unreachable(format!("“{}” can’t be found: {e}", p.host)))?
        .next()
        .ok_or_else(|| RemoteError::Unreachable(format!("“{}” has no address.", p.host)))?;
    let mut ftp = match p.security {
        FtpSecurity::Implicit => {
            Session::connect_secure_implicit(addr, Tls::new(p)?, &p.host).map_err(|e| remote_error(&e))?
        }
        _ => Session::connect_timeout(addr, CONNECT_TIMEOUT).map_err(|e| remote_error(&e))?,
    };
    // A server that stops answering fails the command instead of hanging.
    let _ = ftp.get_ref().set_read_timeout(Some(CONNECT_TIMEOUT));
    let data_timeout = p.data_timeout.unwrap_or(DATA_TIMEOUT);
    ftp = ftp.passive_stream_builder(move |addr| {
        let stream = TcpStream::connect_timeout(&addr, data_timeout).map_err(FtpError::ConnectionError)?;
        // A data connection that stalls fails the transfer, which can
        // then be retried and resumed.
        stream.set_read_timeout(Some(data_timeout)).map_err(FtpError::ConnectionError)?;
        stream.set_write_timeout(Some(data_timeout)).map_err(FtpError::ConnectionError)?;
        Ok(stream)
    });
    if p.security == FtpSecurity::Explicit {
        // The system trust store decides, unless the user chose otherwise.
        ftp = ftp.into_secure(Tls::new(p)?, &p.host).map_err(|e| remote_error(&e))?;
    }
    let user = if p.user.is_empty() { "anonymous" } else { p.user.as_str() };
    ftp.login(user, p.password.as_str()).map_err(|e| remote_error(&e))?;
    if p.security == FtpSecurity::Implicit {
        // Protect the data connections too, as `into_secure` does for
        // explicit TLS.
        ftp.custom_command("PBSZ 0", &[Status::CommandOk]).map_err(|e| remote_error(&e))?;
        ftp.custom_command("PROT P", &[Status::CommandOk]).map_err(|e| remote_error(&e))?;
    }
    ftp.set_encoding(SharedEncoding(Arc::clone(names)));
    if p.passive {
        ftp.set_mode(if plain_passive { Mode::Passive } else { Mode::ExtendedPassive });
        // A server behind NAT may name a private address in its PASV
        // reply: connect to the address the control connection uses.
        ftp.set_passive_nat_workaround(true);
    } else {
        ftp = ftp.active_mode(data_timeout);
    }
    ftp.transfer_type(FileType::Binary).map_err(|e| remote_error(&e))?;
    Ok(ftp)
}

/// Connect, log in, and find out what the server can do.
pub fn connect(params: FtpParams) -> Result<FtpFs, RemoteError> {
    let names = Arc::new(NameEncoding::new(params.encoding.as_deref()));
    let mut ftp = match open_session(&params, &names, false) {
        Ok(ftp) => ftp,
        Err(RemoteError::Certificate(reason))
            if params.security != FtpSecurity::None && !params.accept_invalid_certs =>
        {
            // Say which certificate it is, so the user can decide to trust
            // exactly that one.
            let handshake =
                if params.security == FtpSecurity::Implicit { Handshake::Direct } else { Handshake::FtpAuthTls };
            return Err(match trust::probe(&params.host, params.port(), handshake) {
                Ok(der) => {
                    RemoteError::Certificate(format!("{reason}\n{FINGERPRINT_LABEL}{}", trust::fingerprint(&der)))
                }
                Err(_) => RemoteError::Certificate(reason),
            });
        }
        Err(other) => return Err(other),
    };
    let features: HashMap<String, Option<String>> =
        ftp.feat().unwrap_or_default().into_iter().map(|(k, v)| (k.to_uppercase(), v)).collect();
    if features.contains_key("UTF8") {
        let _ = ftp.opts("UTF8", Some("ON"));
    }
    let home = match &params.remote_path {
        Some(path) if !path.is_empty() => paths::normalize(path, "/"),
        _ => ftp.pwd().unwrap_or_else(|_| "/".into()),
    };
    let user = if params.user.is_empty() { "anonymous" } else { params.user.as_str() };
    let label = format!("{user}@{}", params.host);
    let keepalive = params.keepalive.unwrap_or(KEEPALIVE);
    let shared = Arc::new(Shared {
        params,
        idle: Mutex::new(vec![(ftp, Instant::now())]),
        features,
        names,
        plain_passive: AtomicBool::new(false),
        recent: Mutex::new(None),
    });
    keep_alive(&shared, keepalive);
    let fs = FtpFs { shared, label, home };
    // Prove the data channel works now, where a failure can be explained.
    fs.list(&fs.home).map_err(|e| RemoteError::Other(e.to_string()))?;
    Ok(fs)
}

/// Send `NOOP` on sessions idle for `every`, so servers with an idle
/// timeout keep them; ones that do not answer are dropped. Ends when the
/// connection is closed.
fn keep_alive(shared: &Arc<Shared>, every: Duration) {
    let weak = Arc::downgrade(shared);
    let tick = every.min(Duration::from_secs(1));
    let _ = std::thread::Builder::new().name("rustyfinder-ftp-noop".into()).spawn(move || loop {
        std::thread::sleep(tick);
        let Some(shared) = weak.upgrade() else { return };
        let due: Vec<Session> = {
            let mut idle = shared.idle.lock().unwrap();
            let (due, rest): (Vec<_>, Vec<_>) = idle.drain(..).partition(|(_, at)| at.elapsed() >= every);
            *idle = rest;
            due.into_iter().map(|(session, _)| session).collect()
        };
        for mut session in due {
            if session.noop().is_ok() {
                shared.idle.lock().unwrap().push((session, Instant::now()));
            }
        }
    });
}

fn body(response: &suppaftp::types::Response) -> String {
    String::from_utf8_lossy(&response.body).trim().to_string()
}

fn vfs_error(path: &str, error: FtpError) -> VfsError {
    match error {
        FtpError::UnexpectedResponse(response) => {
            let text = body(&response);
            let lower = text.to_lowercase();
            match response.status {
                Status::FileUnavailable if lower.contains("permission") || lower.contains("denied") => {
                    VfsError::PermissionDenied(path.to_string())
                }
                Status::FileUnavailable
                    if lower.contains("exist") && !lower.contains("not exist") && !lower.contains("no such") =>
                {
                    VfsError::AlreadyExists(path.to_string())
                }
                Status::FileUnavailable => VfsError::NotFound(path.to_string()),
                Status::ExceededStorage => VfsError::NoSpace,
                _ => VfsError::Io {
                    path: path.to_string(),
                    message: if text.is_empty() {
                        format!("The server answered {}.", response.status as u32)
                    } else {
                        text
                    },
                },
            }
        }
        other => VfsError::Io { path: path.to_string(), message: other.to_string() },
    }
}

/// True if the error means the connection itself is gone.
fn is_broken(error: &FtpError) -> bool {
    matches!(error, FtpError::ConnectionError(_) | FtpError::BadResponse)
}

/// True if the server does not know the command.
fn not_implemented(error: &FtpError) -> bool {
    matches!(error, FtpError::UnexpectedResponse(r)
        if matches!(r.status, Status::BadCommand | Status::BadArguments | Status::NotImplemented | Status::NotImplementedParameter | Status::CommandNotImplemented))
}

/// True if the server does not know extended passive mode.
fn refuses_epsv(error: &FtpError) -> bool {
    not_implemented(error)
}

fn to_ms(time: SystemTime) -> Option<i64> {
    let ms = time.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
    // The parser reports "no date" as the epoch itself.
    (ms > 0).then_some(ms)
}

fn entry_from(file: &File) -> Option<Entry> {
    let name = file.name();
    if name == "." || name == ".." || name.is_empty() {
        return None;
    }
    let bits = |who: PosixPexQuery, shift: u32| -> u32 {
        (u32::from(file.can_read(who)) << 2 | u32::from(file.can_write(who)) << 1 | u32::from(file.can_execute(who)))
            << shift
    };
    let mode = bits(PosixPexQuery::Owner, 6) | bits(PosixPexQuery::Group, 3) | bits(PosixPexQuery::Others, 0);
    let kind = if file.is_directory() {
        EntryKind::Directory
    } else if file.is_symlink() {
        EntryKind::Symlink
    } else {
        EntryKind::File
    };
    Some(Entry {
        name: name.to_string(),
        kind,
        is_dir: file.is_directory(),
        size: if file.is_directory() { 0 } else { file.size() as u64 },
        modified_ms: to_ms(file.modified()),
        created_ms: None,
        is_hidden: name.starts_with('.'),
        // A listing that carries no permissions (MLSD without the unix
        // fact) reports none at all.
        mode: (mode != 0).then_some(mode),
        link_target: file.symlink().map(|p| p.to_string_lossy().into_owned()),
        is_broken_link: false,
        ext: Default::default(),
    })
}

/// One line in the Easily Parsed LIST Format (EPLF):
/// `+i8388621.29609,m824255902,/,\tdocs` or `+m824255902,r,s1234,up644,\tf.txt`.
pub fn parse_eplf(line: &str) -> Option<Entry> {
    let rest = line.strip_prefix('+')?;
    let (facts, name) = rest.split_once('\t')?;
    let name = name.trim_end_matches(['\r', '\n']);
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    let (mut is_dir, mut size, mut modified_ms, mut mode) = (false, 0u64, None, None);
    for fact in facts.split(',') {
        match fact.chars().next() {
            Some('/') => is_dir = true,
            Some('s') => size = fact[1..].parse().ok()?,
            Some('m') => modified_ms = fact[1..].parse::<i64>().ok().map(|s| s * 1000),
            Some('u') if fact.starts_with("up") => mode = u32::from_str_radix(&fact[2..], 8).ok(),
            _ => {}
        }
    }
    Some(Entry {
        name: name.to_string(),
        kind: if is_dir { EntryKind::Directory } else { EntryKind::File },
        is_dir,
        size: if is_dir { 0 } else { size },
        modified_ms,
        created_ms: None,
        is_hidden: name.starts_with('.'),
        mode,
        link_target: None,
        is_broken_link: false,
        ext: Default::default(),
    })
}

/// One `LIST` line in any of the formats servers use; anything else (a
/// "total" line) is skipped, never fatal.
fn parse_list_line(line: &str) -> Option<Entry> {
    if line.starts_with('+') {
        return parse_eplf(line);
    }
    ListParser::parse_posix(line).or_else(|_| ListParser::parse_dos(line)).ok().and_then(|f| entry_from(&f))
}

impl FtpFs {
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn home(&self) -> &str {
        &self.home
    }

    fn has(&self, feature: &str) -> bool {
        self.shared.features.contains_key(feature)
    }

    fn take(&self) -> Result<Session, RemoteError> {
        if let Some((session, _)) = self.shared.idle.lock().unwrap().pop() {
            return Ok(session);
        }
        let mut session =
            open_session(&self.shared.params, &self.shared.names, self.shared.plain_passive.load(Ordering::SeqCst))?;
        if self.has("UTF8") {
            let _ = session.opts("UTF8", Some("ON"));
        }
        Ok(session)
    }

    fn give_back(&self, session: Session) {
        self.shared.idle.lock().unwrap().push((session, Instant::now()));
    }

    /// Run one command on a borrowed session. A stale connection is
    /// replaced and the command tried once more; a server that refuses
    /// extended passive mode is asked again the plain way.
    fn run<R>(&self, path: &str, mut command: impl FnMut(&mut Session) -> Result<R, FtpError>) -> VfsResult<R> {
        let mut retried = false;
        loop {
            let mut session =
                self.take().map_err(|e| VfsError::Io { path: path.to_string(), message: e.to_string() })?;
            match command(&mut session) {
                Ok(value) => {
                    self.give_back(session);
                    return Ok(value);
                }
                Err(error) if is_broken(&error) && !retried => {
                    // Every idle session is as old as this one: start over.
                    self.shared.idle.lock().unwrap().clear();
                    retried = true;
                }
                Err(error) if refuses_epsv(&error) && !self.shared.plain_passive.swap(true, Ordering::SeqCst) => {
                    session.set_mode(Mode::Passive);
                    self.give_back(session);
                }
                Err(error) => {
                    if !is_broken(&error) {
                        self.give_back(session);
                    }
                    return Err(vfs_error(path, error));
                }
            }
        }
    }

    /// Start a transfer. Unlike `run`, the session is not given back: it
    /// belongs to the transfer until that ends.
    fn begin(
        &self,
        path: &str,
        mut start: impl FnMut(&mut Session) -> Result<Transfer, FtpError>,
    ) -> VfsResult<(Session, Transfer)> {
        let mut retried = false;
        loop {
            let mut session =
                self.take().map_err(|e| VfsError::Io { path: path.to_string(), message: e.to_string() })?;
            match start(&mut session) {
                Ok(stream) => return Ok((session, stream)),
                Err(error) if is_broken(&error) && !retried => {
                    self.shared.idle.lock().unwrap().clear();
                    retried = true;
                }
                Err(error) if refuses_epsv(&error) && !self.shared.plain_passive.swap(true, Ordering::SeqCst) => {
                    session.set_mode(Mode::Passive);
                    self.give_back(session);
                }
                Err(error) => {
                    if !is_broken(&error) {
                        self.give_back(session);
                    }
                    return Err(vfs_error(path, error));
                }
            }
        }
    }

    fn forget_recent(&self) {
        *self.shared.recent.lock().unwrap() = None;
    }

    /// `YYYYMMDDHHMMSS` in UTC, as `MFMT` wants it.
    fn timestamp(ms: i64) -> String {
        let secs = ms.div_euclid(1000);
        let days = secs.div_euclid(86400);
        let rem = secs.rem_euclid(86400);
        // Civil-from-days (Howard Hinnant's algorithm).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        format!("{year:04}{month:02}{day:02}{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
    }

    fn download(&self, session: Session, stream: Transfer) -> Box<dyn Read + Send> {
        Box::new(Download {
            shared: Arc::clone(&self.shared),
            session: Some(session),
            stream: Some(stream),
            done: false,
        })
    }

    fn upload(&self, session: Session, stream: Transfer, path: &str) -> Box<dyn Write + Send> {
        Box::new(Upload {
            shared: Arc::clone(&self.shared),
            session: Some(session),
            stream: Some(stream),
            path: path.to_string(),
        })
    }
}

impl Vfs for FtpFs {
    fn id(&self) -> String {
        format!("ftp://{}", self.label)
    }

    fn list(&self, path: &str) -> VfsResult<Vec<Entry>> {
        let entries = if self.has("MLSD") {
            // Exact times, types and sizes, in UTC.
            let lines = self.run(path, |ftp| ftp.mlsd(Some(path)))?;
            lines.iter().filter_map(|l| ListParser::parse_mlsd(l).ok()).filter_map(|f| entry_from(&f)).collect()
        } else {
            // `LIST` takes no reliable path argument (spaces, options), so
            // go to the folder and list "here"; "-a" asks for dot-files.
            let lines = self.run(path, |ftp| {
                ftp.cwd(path)?;
                ftp.list(Some("-a")).or_else(|e| if is_broken(&e) { Err(e) } else { ftp.list(None) })
            })?;
            // Unix `ls -l` lines, the Windows form, or EPLF.
            lines.iter().filter_map(|l| parse_list_line(l)).collect()
        };
        let entries: Vec<Entry> = entries;
        *self.shared.recent.lock().unwrap() = Some((path.to_string(), Instant::now(), entries.clone()));
        Ok(entries)
    }

    fn stat(&self, path: &str) -> VfsResult<Entry> {
        if path == "/" {
            return Ok(Entry {
                name: String::new(),
                kind: EntryKind::Directory,
                is_dir: true,
                size: 0,
                modified_ms: None,
                created_ms: None,
                is_hidden: false,
                mode: None,
                link_target: None,
                is_broken_link: false,
                ext: Default::default(),
            });
        }
        let name = paths::file_name(path).to_string();
        if self.has("MLST") {
            let line = self.run(path, |ftp| ftp.mlst(Some(path)))?;
            if let Some(mut entry) = ListParser::parse_mlst(&line).ok().and_then(|f| entry_from(&f)) {
                // The reply may name the item by its full path.
                entry.name = name;
                return Ok(entry);
            }
        }
        let parent = paths::parent(path).unwrap_or_else(|| "/".to_string());
        let cached = {
            let recent = self.shared.recent.lock().unwrap();
            recent
                .as_ref()
                .filter(|(dir, at, _)| *dir == parent && at.elapsed() < Duration::from_secs(2))
                .map(|(_, _, entries)| entries.clone())
        };
        let entries = match cached {
            Some(entries) => entries,
            None => self.list(&parent)?,
        };
        entries.into_iter().find(|e| e.name == name).ok_or_else(|| VfsError::NotFound(path.to_string()))
    }

    fn mkdir(&self, path: &str) -> VfsResult<()> {
        self.forget_recent();
        match self.run(path, |ftp| ftp.mkdir(path)) {
            // Servers answer 550 for "exists" and "no such parent" alike.
            Err(VfsError::NotFound(_)) if self.stat(path).is_ok() => Err(VfsError::AlreadyExists(path.to_string())),
            other => other,
        }
    }

    fn rename(&self, from: &str, to: &str, replace: bool) -> VfsResult<()> {
        if !replace && self.stat(to).is_ok() {
            return Err(VfsError::AlreadyExists(to.to_string()));
        }
        self.forget_recent();
        self.run(from, |ftp| ftp.rename(from, to))
    }

    fn remove_file(&self, path: &str) -> VfsResult<()> {
        self.forget_recent();
        self.run(path, |ftp| ftp.rm(path))
    }

    fn remove_dir(&self, path: &str) -> VfsResult<()> {
        self.forget_recent();
        self.run(path, |ftp| ftp.rmdir(path))
    }

    fn open_read(&self, path: &str) -> VfsResult<Box<dyn Read + Send>> {
        let (session, stream) = self.begin(path, |ftp| ftp.retr_as_stream(path))?;
        Ok(self.download(session, stream))
    }

    fn create(&self, path: &str) -> VfsResult<Box<dyn Write + Send>> {
        if self.stat(path).is_ok() {
            return Err(VfsError::AlreadyExists(path.to_string()));
        }
        self.forget_recent();
        let (session, stream) = self.begin(path, |ftp| ftp.put_with_stream(path))?;
        Ok(self.upload(session, stream, path))
    }

    fn resume_support(&self) -> (bool, bool) {
        // Downloads need `REST`; uploads fall back on `APPE`, which every
        // server knows.
        (self.has("REST"), true)
    }

    fn open_read_at(&self, path: &str, offset: u64) -> VfsResult<Box<dyn Read + Send>> {
        if offset > 0 && !self.has("REST") {
            return Err(VfsError::Unsupported(path.to_string()));
        }
        let at = offset as usize;
        let (session, stream) = self.begin(path, |ftp| {
            ftp.resume_transfer(at)?;
            ftp.retr_as_stream(path)
        })?;
        Ok(self.download(session, stream))
    }

    fn open_write_at(&self, path: &str, offset: u64) -> VfsResult<Box<dyn Write + Send>> {
        self.forget_recent();
        let at = offset as usize;
        let rest = self.has("REST");
        let (session, stream) = self.begin(path, |ftp| {
            if rest {
                ftp.resume_transfer(at)?;
                match ftp.put_with_stream(path) {
                    // `REST` before `STOR` refused: append instead.
                    Err(e) if not_implemented(&e) => {
                        ftp.resume_transfer(0)?;
                        ftp.append_with_stream(path)
                    }
                    other => other,
                }
            } else {
                // `APPE` adds to the end, which is the offset: the engine
                // resumes from the partial file's own size.
                ftp.append_with_stream(path)
            }
        })?;
        Ok(self.upload(session, stream, path))
    }

    fn set_times(&self, path: &str, modified_ms: Option<i64>, _created_ms: Option<i64>) -> VfsResult<()> {
        let Some(ms) = modified_ms else { return Ok(()) };
        let stamp = Self::timestamp(ms);
        self.forget_recent();
        if self.has("MFMT") {
            let command = format!("MFMT {stamp} {path}");
            return self.run(path, |ftp| ftp.custom_command(command.clone(), &[Status::File])).map(|_| ());
        }
        // No MFMT: `SITE UTIME`, in the two forms servers use. Neither is
        // an error where unsupported; the upload time then stands.
        let five = format!("UTIME {path} {stamp} {stamp} {stamp} UTC");
        if self.run(path, |ftp| ftp.site(five.clone())).is_err() {
            let two = format!("UTIME {stamp} {path}");
            let _ = self.run(path, |ftp| ftp.site(two.clone()));
        }
        Ok(())
    }

    fn set_mode(&self, path: &str, mode: u32) -> VfsResult<()> {
        let command = format!("CHMOD {:o} {path}", mode & 0o7777);
        self.forget_recent();
        match self.run(path, |ftp| ftp.site(command.clone())) {
            Ok(_) => Ok(()),
            Err(e @ (VfsError::PermissionDenied(_) | VfsError::NotFound(_))) => Err(e),
            // The server has no SITE CHMOD (Windows servers, many hosted
            // ones): say so instead of pretending it worked.
            Err(_) => Err(VfsError::Unsupported(format!("This server can’t change permissions of “{path}”."))),
        }
    }

    fn nfc_names(&self) -> bool {
        true
    }

    fn url_for(&self, path: &str) -> Option<String> {
        let p = &self.shared.params;
        let scheme = if p.security == FtpSecurity::None { "ftp" } else { "ftps" };
        let default = if p.security == FtpSecurity::Implicit { 990 } else { 21 };
        let mut url = format!("{scheme}://");
        if !p.user.is_empty() {
            url.push_str(&crate::webdav::encode_path(&p.user));
            url.push('@');
        }
        url.push_str(&p.host);
        if p.port() != default {
            url.push_str(&format!(":{}", p.port()));
        }
        url.push_str(&crate::webdav::encode_path(path));
        Some(url)
    }
}

/// A download in progress. It owns its session until the last byte.
struct Download {
    shared: Arc<Shared>,
    session: Option<Session>,
    stream: Option<Transfer>,
    done: bool,
}

impl Read for Download {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let Some(stream) = self.stream.as_mut() else { return Ok(0) };
        let n = stream.read(buffer)?;
        if n == 0 && !self.done {
            self.done = true;
            // The server confirms the transfer after the data ends.
            if let Some(stream) = self.stream.take() {
                stream.finish().map_err(|e| std::io::Error::other(e.to_string()))?;
            }
            if let Some(session) = self.session.take() {
                self.shared.idle.lock().unwrap().push((session, Instant::now()));
            }
        }
        Ok(n)
    }
}

impl Drop for Download {
    fn drop(&mut self) {
        // Dropped before the end (a stopped copy). Servers differ in how
        // many replies an aborted transfer produces, and a miscounted reply
        // would garble every later command on this session. So the session
        // is closed with the transfer; the next call opens a fresh one.
        // Closing the control connection first means dropping the transfer
        // does not wait for a reply from a server that may never send one.
        if !self.done {
            if let Some(session) = &self.session {
                let _ = session.get_ref().shutdown(std::net::Shutdown::Both);
            }
        }
        self.stream.take();
        self.session.take();
    }
}

/// An upload in progress. `flush` ends it and reports what the server
/// said; the copy engine flushes when the last byte is written.
struct Upload {
    shared: Arc<Shared>,
    session: Option<Session>,
    stream: Option<Transfer>,
    path: String,
}

impl Upload {
    fn finish(&mut self) -> std::io::Result<()> {
        let Some(stream) = self.stream.take() else { return Ok(()) };
        let result = stream.finish();
        if let Some(session) = self.session.take() {
            if result.is_ok() {
                self.shared.idle.lock().unwrap().push((session, Instant::now()));
            }
        }
        result.map_err(|e| std::io::Error::other(vfs_error(&self.path, e).to_string()))
    }
}

impl Write for Upload {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self.stream.as_mut() {
            Some(stream) => stream.write(buffer),
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
    use suppaftp::Encoding;

    #[test]
    fn timestamps_are_utc_calendar_dates() {
        assert_eq!(FtpFs::timestamp(0), "19700101000000");
        // FT-09: 2020-01-01T00:00:00Z
        assert_eq!(FtpFs::timestamp(1_577_836_800_000), "20200101000000");
        assert_eq!(FtpFs::timestamp(1_709_210_096_000), "20240229123456");
    }

    #[test]
    fn utf8_names_pass_through_and_legacy_names_round_trip() {
        // FT-04: a CP1251 name is decoded, then sent back as the same bytes.
        let names = NameEncoding::new(None);
        assert_eq!(names.decode("résumé.txt".as_bytes()), "résumé.txt");
        assert_eq!(names.encode("résumé.txt").unwrap(), "résumé.txt".as_bytes());

        let (raw, _, _) = encoding_rs::WINDOWS_1251.encode("Привет, это список файлов на сервере.txt");
        let decoded = names.decode(&raw);
        assert_eq!(decoded, "Привет, это список файлов на сервере.txt");
        assert_eq!(names.encode(&decoded).unwrap(), raw.as_ref());
        // Once the server's encoding is known, it stays.
        assert_eq!(names.encode("Привет.txt").unwrap(), encoding_rs::WINDOWS_1251.encode("Привет.txt").0.as_ref());
        assert!(names.encode("日本語").is_err(), "a name the encoding cannot hold is refused, not mangled");
        assert_eq!(names.encode("plain.txt").unwrap(), b"plain.txt");
    }

    #[test]
    fn a_chosen_encoding_is_used_without_guessing() {
        let names = NameEncoding::new(Some("shift_jis"));
        let (raw, _, _) = encoding_rs::SHIFT_JIS.encode("日本語.txt");
        assert_eq!(names.decode(&raw), "日本語.txt");
        assert_eq!(names.encode("日本語.txt").unwrap(), raw.as_ref());
    }

    #[test]
    fn listing_lines_become_entries() {
        // FT-05: Windows/IIS lines.
        let dir = ListParser::parse_dos("04-08-14  03:09PM       <DIR>          My Docs").unwrap();
        let entry = entry_from(&dir).unwrap();
        assert!(entry.is_dir && entry.name == "My Docs", "{entry:?}");
        let file = ListParser::parse_dos("10-19-20  03:19PM              2048 report 1.pdf").unwrap();
        let entry = entry_from(&file).unwrap();
        assert_eq!((entry.name.as_str(), entry.size, entry.is_dir), ("report 1.pdf", 2048, false));
        assert!(entry.modified_ms.is_some());

        // Unix lines, with permissions.
        let unix = ListParser::parse_posix("-rw-r----- 1 bob staff 1234 Nov 5 2018 notes.txt").unwrap();
        let entry = entry_from(&unix).unwrap();
        assert_eq!((entry.name.as_str(), entry.size, entry.mode), ("notes.txt", 1234, Some(0o640)));
        assert!(ListParser::parse_posix("total 12").is_err(), "a total line is skipped, not fatal");

        // The folder's own entries are dropped.
        let here = ListParser::parse_posix("drwxr-xr-x 2 bob staff 64 Nov 5 2018 .").unwrap();
        assert!(entry_from(&here).is_none());
    }

    #[test]
    fn eplf_lines_become_entries() {
        let dir = parse_list_line("+i8388621.29609,m824255902,/,\tdocs").unwrap();
        assert!(dir.is_dir && dir.name == "docs");
        assert_eq!(dir.modified_ms, Some(824_255_902_000));
        let file = parse_list_line("+i8388621.44468,m839956783,r,s10376,up644,\tRFCEPLF  name.txt").unwrap();
        assert_eq!(
            (file.name.as_str(), file.size, file.is_dir, file.mode),
            ("RFCEPLF  name.txt", 10376, false, Some(0o644))
        );
        assert!(parse_list_line("+m1,/,\t.").is_none());
        assert!(parse_list_line("+no tab here").is_none());
        // The other formats still go through the same door.
        assert_eq!(parse_list_line("-rw-r--r-- 1 bob staff 5 Nov 5 2018 a.txt").unwrap().size, 5);
        assert!(parse_list_line("total 12").is_none());
    }

    #[test]
    fn implicit_tls_defaults_to_port_990() {
        let implicit = FtpParams { security: FtpSecurity::Implicit, ..FtpParams::default() };
        assert_eq!(implicit.port(), 990);
        assert_eq!(FtpParams::default().port(), 21);
        assert_eq!(FtpParams { port: Some(2121), ..implicit }.port(), 2121);
    }
}
