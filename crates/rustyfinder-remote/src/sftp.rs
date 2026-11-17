//! SFTP over the system OpenSSH client.
//!
//! `ssh <options> -s host sftp` gives a raw SFTP channel on the process's
//! stdin and stdout; `russh-sftp` speaks the protocol over it. Using the
//! system `ssh` means everything in `~/.ssh/config` applies (Host aliases,
//! ProxyJump, IdentityFile...), as do `known_hosts` and the agent.
//!
//! The backend is blocking, like every `Vfs`: it owns a small tokio
//! runtime and waits on it. Never call it from inside that runtime.
//!
//! A session that died (sleep, a network change, a server restart) is
//! noticed by the first request that fails and opened again once, without
//! asking, before that request gives up (edge-cases spec §5).
//!
//! Names are bytes on the server. The (patched) protocol layer keeps bytes
//! that are not UTF-8 as private-use characters, so every name can still
//! be addressed; with a per-connection encoding they are decoded with it.

use std::io::{self, Read, Write};
use std::process::Stdio;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::fs::{File, Metadata};
use russh_sftp::client::{Config, SftpSession};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use rustyfinder_core::{Entry, EntryKind, Space, Vfs, VfsError};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::runtime::Runtime;

use crate::askpass::{AskpassServer, PromptKind, Prompter, SOCKET_ENV};

/// Seconds ssh waits for the TCP connection and each SFTP reply.
const TIMEOUT_SECS: u64 = 60;

type VfsResult<T> = Result<T, VfsError>;

/// What the Connect panel collected.
#[derive(Debug, Clone, Default)]
pub struct SftpParams {
    /// Host name, address, or a `Host` alias from the ssh config.
    pub host: String,
    /// Empty: let the ssh config (or the local user name) decide.
    pub user: String,
    pub port: Option<u16>,
    pub password: Option<String>,
    /// Private key file to use instead of a password or the agent.
    pub key_file: Option<String>,
    /// Folder to start in; `None` means the login directory.
    pub remote_path: Option<String>,
    /// Program ssh runs to ask questions (see `askpass`).
    pub askpass_program: String,
    /// Use this ssh config instead of `~/.ssh/config` (tests).
    pub config_file: Option<String>,
    /// Use this known_hosts instead of `~/.ssh/known_hosts` (tests).
    pub known_hosts_file: Option<String>,
    /// Encoding of names on the server, e.g. "windows-1251"; `None` is UTF-8.
    pub encoding: Option<String>,
    /// After a "host key changed" warning, the user chose to connect anyway
    /// to the server presenting this new key (its `SHA256:` fingerprint):
    /// the old record goes and this key is accepted. Any other key is not.
    pub accept_changed_host_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectError {
    #[error("Authorization failed.")]
    AuthFailed,
    #[error("The server “{0}” can’t be found.")]
    CannotResolve(String),
    #[error("The server did not answer in time.")]
    Timeout,
    /// The server's key differs from the one on record. `0` is ssh's own
    /// explanation, including the new fingerprint.
    #[error("The host key of this server has changed.\n{0}")]
    HostKeyChanged(String),
    #[error("The host key was not accepted.")]
    HostKeyRejected,
    #[error("The connection was refused.")]
    Refused,
    #[error("{0}")]
    Other(String),
}

/// The new key's `SHA256:` fingerprint in ssh's "host key changed" text.
pub fn changed_host_key_fingerprint(message: &str) -> Option<String> {
    message
        .split_whitespace()
        .find(|word| word.starts_with("SHA256:"))
        .map(|word| word.trim_end_matches(['.', ',']).to_string())
}

/// Options before the host: the fixed ones from the spec plus what the
/// user chose.
fn ssh_options(params: &SftpParams) -> Vec<String> {
    fn option(args: &mut Vec<String>, o: &str) {
        args.push("-o".into());
        args.push(o.into());
    }
    let mut args: Vec<String> = Vec::new();
    option(&mut args, &format!("ConnectTimeout={TIMEOUT_SECS}"));
    // Two unanswered probes 30 s apart and the session is declared dead.
    option(&mut args, "ServerAliveInterval=30");
    option(&mut args, "ServerAliveCountMax=2");
    // A wrong password fails at once; the app asks again, not ssh.
    option(&mut args, "NumberOfPasswordPrompts=1");
    option(&mut args, "ForwardX11=no");
    option(&mut args, "ClearAllForwardings=yes");
    option(&mut args, "BatchMode=no");
    if let Some(key) = &params.key_file {
        option(&mut args, "IdentitiesOnly=yes");
        args.push("-i".into());
        args.push(key.clone());
    } else if params.password.is_some() {
        // A password was typed: do not offer agent keys first, which can
        // exhaust the server's attempts before the password is tried.
        option(&mut args, "PubkeyAuthentication=no");
        option(&mut args, "PreferredAuthentications=password,keyboard-interactive");
    }
    if let Some(known) = &params.known_hosts_file {
        option(&mut args, &format!("UserKnownHostsFile={known}"));
    }
    if let Some(config) = &params.config_file {
        args.push("-F".into());
        args.push(config.clone());
    }
    if let Some(port) = params.port {
        args.push("-p".into());
        args.push(port.to_string());
    }
    if !params.user.is_empty() {
        args.push("-l".into());
        args.push(params.user.clone());
    }
    args
}

/// The ssh command line for `params`.
fn ssh_arguments(params: &SftpParams) -> Vec<String> {
    let mut args = ssh_options(params);
    // -s: what follows the host names a subsystem, not a shell command.
    args.push("-s".into());
    // "--" so a host starting with "-" can never be read as an option.
    args.push("--".into());
    args.push(params.host.clone());
    args.push("sftp".into());
    args
}

/// Turn what ssh printed on failure into a reason the user can act on.
fn explain(stderr: &str, host: &str) -> ConnectError {
    let lower = stderr.to_lowercase();
    if lower.contains("remote host identification has changed") {
        ConnectError::HostKeyChanged(stderr.trim().to_string())
    } else if lower.contains("host key verification failed") {
        ConnectError::HostKeyRejected
    } else if lower.contains("permission denied") || lower.contains("too many authentication failures") {
        ConnectError::AuthFailed
    } else if lower.contains("could not resolve hostname") {
        ConnectError::CannotResolve(host.to_string())
    } else if lower.contains("timed out") {
        ConnectError::Timeout
    } else if lower.contains("connection refused") {
        ConnectError::Refused
    } else {
        // The last line is usually the one that says what went wrong.
        let last = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("The connection could not be made.");
        ConnectError::Other(last.trim().to_string())
    }
}

/// Forget the recorded key of the server `params` reach, in every
/// known_hosts file ssh would consult, so the key it presents now can be
/// accepted. `ssh-keygen -R` keeps a `.old` copy of each file it changes.
fn forget_host_key(params: &SftpParams) -> Result<(), ConnectError> {
    let mut args = ssh_options(params);
    args.extend(["-G".into(), "--".into(), params.host.clone()]);
    let output = std::process::Command::new("/usr/bin/ssh")
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| ConnectError::Other(e.to_string()))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let value = |key: &str| {
        text.lines().find_map(|l| l.strip_prefix(key).and_then(|rest| rest.strip_prefix(' '))).map(str::to_string)
    };
    let host = value("hostname").unwrap_or_else(|| params.host.clone());
    let port = value("port").and_then(|p| p.parse::<u16>().ok()).unwrap_or(22);
    let pattern = if port == 22 { host } else { format!("[{host}]:{port}") };
    for file in value("userknownhostsfile").unwrap_or_default().split_whitespace() {
        if std::path::Path::new(file).exists() {
            let _ = std::process::Command::new("/usr/bin/ssh-keygen")
                .args(["-R", &pattern, "-f", file])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    Ok(())
}

/// Answers the unknown-host question for the one key the user agreed to
/// after the "host key changed" warning; everything else goes to the user.
struct AcceptChangedKey {
    fingerprint: String,
    user: Arc<dyn Prompter>,
}

impl Prompter for AcceptChangedKey {
    fn prompt(&self, text: &str, kind: PromptKind) -> Option<String> {
        if kind == PromptKind::HostKey && text.contains(&self.fingerprint) {
            return Some("yes".into());
        }
        self.user.prompt(text, kind)
    }
}

/// One running ssh process and the SFTP session over it.
struct Live {
    session: Arc<SftpSession>,
    child: Child,
    _askpass: AskpassServer,
}

/// Start ssh and open the SFTP session. Returns the session and the
/// canonical start folder (as sent on the wire).
fn open_live(
    params: &SftpParams,
    prompter: Arc<dyn Prompter>,
    runtime: &Runtime,
) -> Result<(Live, String), ConnectError> {
    let other = |e: io::Error| ConnectError::Other(e.to_string());
    let askpass = AskpassServer::start(params.password.clone(), prompter).map_err(other)?;
    let names = Names::new(params.encoding.as_deref());
    runtime.block_on(async {
        let mut child = Command::new("/usr/bin/ssh")
            .args(ssh_arguments(params))
            .env("SSH_ASKPASS", &params.askpass_program)
            .env("SSH_ASKPASS_REQUIRE", "force")
            // Older ssh only uses askpass when a display is set.
            .env("DISPLAY", ":0")
            .env(SOCKET_ENV, askpass.socket_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(other)?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");

        let config = Config { request_timeout_secs: TIMEOUT_SECS, ..Config::default() };
        let session = match SftpSession::new_with_config(tokio::io::join(stdout, stdin), config).await {
            Ok(session) => session,
            Err(_) => {
                // ssh gave up before the SFTP channel came up; it says why.
                let mut text = String::new();
                let _ = tokio::time::timeout(Duration::from_secs(5), stderr.read_to_string(&mut text)).await;
                let _ = child.kill().await;
                return Err(explain(&text, &params.host));
            }
        };
        // "." resolves to the login directory, which is where a session
        // starts unless a remote path was given.
        let start = params.remote_path.as_deref().filter(|p| !p.is_empty()).unwrap_or(".");
        let home = session
            .canonicalize(names.wire(start))
            .await
            .map_err(|e| ConnectError::Other(format!("The folder “{start}” can’t be opened: {e}")))?;
        Ok((Live { session: Arc::new(session), child, _askpass: askpass }, home))
    })
}

/// Open an SFTP session. Blocks until connected or failed; `prompter` is
/// asked for anything ssh needs (see `askpass`).
pub fn connect(params: &SftpParams, prompter: Arc<dyn Prompter>) -> Result<SftpFs, ConnectError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("rustyfinder-sftp")
        .build()
        .map_err(|e| ConnectError::Other(e.to_string()))?;
    let prompter: Arc<dyn Prompter> = match &params.accept_changed_host_key {
        Some(fingerprint) => {
            forget_host_key(params)?;
            Arc::new(AcceptChangedKey { fingerprint: fingerprint.clone(), user: prompter })
        }
        None => prompter,
    };
    let (live, home) = open_live(params, Arc::clone(&prompter), &runtime)?;
    let names = Names::new(params.encoding.as_deref());
    let label = if params.user.is_empty() { params.host.clone() } else { format!("{}@{}", params.user, params.host) };
    // Reconnecting never needs the override again: the new key is on record.
    let params = SftpParams { accept_changed_host_key: None, ..params.clone() };
    Ok(SftpFs {
        home: names.local(&home),
        names,
        live: RwLock::new(live),
        reconnects: Mutex::new(0),
        runtime,
        label,
        params,
        prompter,
    })
}

/// How names travel: bytes on the wire (private-use characters stand for
/// bytes that are not UTF-8), text in RustyFinder.
#[derive(Debug, Clone, Copy)]
struct Names {
    /// The server's encoding when it is not UTF-8.
    legacy: Option<&'static encoding_rs::Encoding>,
}

impl Names {
    fn new(label: Option<&str>) -> Self {
        let legacy =
            label.and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes())).filter(|e| *e != encoding_rs::UTF_8);
        Self { legacy }
    }

    /// A name or path as RustyFinder shows it, from what the server sent.
    fn local(&self, wire: &str) -> String {
        let Some(encoding) = self.legacy else { return wire.to_string() };
        let bytes = russh_sftp::unescape_bytes(wire);
        match encoding.decode_without_bom_handling_and_without_replacement(&bytes) {
            Some(text) => text.into_owned(),
            // Not valid in that encoding either: keep it addressable.
            None => wire.to_string(),
        }
    }

    /// The same, to send back: exactly the bytes it came as.
    fn wire(&self, local: &str) -> String {
        let Some(encoding) = self.legacy else { return local.to_string() };
        if local.is_ascii() {
            return local.to_string();
        }
        let mut bytes = Vec::with_capacity(local.len());
        let raw = russh_sftp::RAW_BYTE_BASE..=russh_sftp::RAW_BYTE_BASE + 0xFF;
        for c in local.chars() {
            if raw.contains(&(c as u32)) {
                bytes.push((c as u32 - russh_sftp::RAW_BYTE_BASE) as u8);
                continue;
            }
            let mut utf8 = [0u8; 4];
            let (encoded, _, unmappable) = encoding.encode(c.encode_utf8(&mut utf8));
            if unmappable {
                bytes.extend_from_slice(c.to_string().as_bytes());
            } else {
                bytes.extend_from_slice(&encoded);
            }
        }
        russh_sftp::escape_bytes(&bytes)
    }
}

/// A live SFTP session. Dropping it ends the ssh process.
pub struct SftpFs {
    live: RwLock<Live>,
    /// How often the session was opened again after dying.
    reconnects: Mutex<u32>,
    runtime: Runtime,
    label: String,
    home: String,
    names: Names,
    params: SftpParams,
    prompter: Arc<dyn Prompter>,
}

/// True if the error means the session itself is gone, not that the
/// request was refused.
fn is_dead(error: &SftpError) -> bool {
    match error {
        SftpError::IO(_) | SftpError::Timeout => true,
        SftpError::UnexpectedBehavior(text) => {
            let text = text.to_lowercase();
            text.contains("closed") || text.contains("dropped") || text.contains("recverror")
        }
        SftpError::Status(status) => {
            matches!(status.status_code, StatusCode::NoConnection | StatusCode::ConnectionLost)
        }
        _ => false,
    }
}

impl SftpFs {
    /// "bob@example.org", for tabs and the sidebar.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The folder the session started in (the login directory by default).
    pub fn home(&self) -> &str {
        &self.home
    }

    /// Process id of the ssh client, while it runs.
    pub fn ssh_pid(&self) -> Option<u32> {
        self.live.read().unwrap().child.id()
    }

    /// How often a dead session was opened again.
    pub fn reconnects(&self) -> u32 {
        *self.reconnects.lock().unwrap()
    }

    fn session(&self) -> Arc<SftpSession> {
        Arc::clone(&self.live.read().unwrap().session)
    }

    fn ssh_exited(&self) -> bool {
        self.live.write().unwrap().child.try_wait().ok().flatten().is_some()
    }

    /// Open the session again, unless another call already did since
    /// `stale` was in use.
    fn reconnect(&self, stale: &Arc<SftpSession>) -> Result<(), ConnectError> {
        let mut live = self.live.write().unwrap();
        if !Arc::ptr_eq(&live.session, stale) {
            return Ok(());
        }
        let (fresh, _) = open_live(&self.params, Arc::clone(&self.prompter), &self.runtime)?;
        let old = std::mem::replace(&mut *live, fresh);
        *self.reconnects.lock().unwrap() += 1;
        drop(live);
        close_live(&self.runtime, old);
        Ok(())
    }

    /// Run one request. If the session turns out to be dead, it is opened
    /// again once and the request repeated.
    fn call<T, F, Fut>(&self, path: &str, request: F) -> VfsResult<T>
    where
        F: Fn(Arc<SftpSession>) -> Fut,
        Fut: std::future::Future<Output = Result<T, SftpError>>,
    {
        let session = self.session();
        match self.runtime.block_on(request(Arc::clone(&session))) {
            Ok(value) => Ok(value),
            Err(error) if is_dead(&error) || self.ssh_exited() => {
                if self.reconnect(&session).is_err() {
                    return Err(map_error(path, error));
                }
                self.runtime.block_on(request(self.session())).map_err(|e| map_error(path, e))
            }
            Err(error) => Err(map_error(path, error)),
        }
    }

    fn exists(&self, wire: &str) -> bool {
        let wire = wire.to_string();
        self.call(&wire, |s| {
            let p = wire.clone();
            async move { s.symlink_metadata(p).await }
        })
        .is_ok()
    }

    fn entry(&self, session: &SftpSession, name: String, wire_path: &str, attrs: &FileAttributes) -> Entry {
        let kind = if attrs.is_symlink() {
            EntryKind::Symlink
        } else if attrs.is_dir() {
            EntryKind::Directory
        } else if attrs.is_regular() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        let mut is_dir = kind == EntryKind::Directory;
        let mut size = if kind == EntryKind::File { attrs.size.unwrap_or(0) } else { 0 };
        let mut link_target = None;
        let mut is_broken_link = false;
        if kind == EntryKind::Symlink {
            // One extra round trip per link, to know whether it leads to a
            // folder (so it can be entered) or nowhere.
            link_target = self.runtime.block_on(session.read_link(wire_path)).ok().map(|t| self.names.local(&t));
            match self.runtime.block_on(session.metadata(wire_path)) {
                Ok(target) => {
                    is_dir = target.is_dir();
                    if target.is_regular() {
                        size = target.size.unwrap_or(0);
                    }
                }
                Err(_) => is_broken_link = true,
            }
        }
        let name = self.names.local(&name);
        Entry {
            is_hidden: name.starts_with('.'),
            name,
            kind,
            is_dir,
            size,
            modified_ms: attrs.mtime.map(|s| i64::from(s) * 1000),
            created_ms: None,
            mode: attrs.permissions.map(|p| p & 0o7777),
            link_target,
            is_broken_link,
            ext: rustyfinder_core::EntryExt {
                uid: attrs.uid,
                gid: attrs.gid,
                owner: attrs.user.clone(),
                group: attrs.group.clone(),
                ..Default::default()
            },
        }
    }
}

/// Close a session politely, then make sure no ssh process is left.
fn close_live(runtime: &Runtime, mut live: Live) {
    let session = Arc::clone(&live.session);
    let _ = runtime.block_on(async { tokio::time::timeout(Duration::from_secs(2), session.close()).await });
    let _ = live.child.start_kill();
    let _ = runtime.block_on(async { tokio::time::timeout(Duration::from_secs(2), live.child.wait()).await });
}

impl Drop for SftpFs {
    fn drop(&mut self) {
        if let Ok(mut live) = self.live.write() {
            let session = Arc::clone(&live.session);
            let _ =
                self.runtime.block_on(async { tokio::time::timeout(Duration::from_secs(2), session.close()).await });
            let _ = live.child.start_kill();
            let _ =
                self.runtime.block_on(async { tokio::time::timeout(Duration::from_secs(2), live.child.wait()).await });
        }
    }
}

fn map_error(path: &str, error: SftpError) -> VfsError {
    match error {
        SftpError::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => VfsError::NotFound(path.to_string()),
            StatusCode::PermissionDenied => VfsError::PermissionDenied(path.to_string()),
            StatusCode::OpUnsupported => VfsError::Unsupported(path.to_string()),
            _ => VfsError::Io {
                path: path.to_string(),
                message: if status.error_message.is_empty() {
                    "The server refused the request.".to_string()
                } else {
                    status.error_message
                },
            },
        },
        SftpError::Timeout => {
            VfsError::Io { path: path.to_string(), message: "The server did not answer in time.".into() }
        }
        other => VfsError::Io { path: path.to_string(), message: other.to_string() },
    }
}

fn join(dir: &str, name: &str) -> String {
    rustyfinder_core::paths::join(dir, name)
}

/// One request with owned arguments, so it can be sent twice if the
/// session has to be opened again in between.
macro_rules! request {
    ($fs:expr, $path:expr, [$($arg:ident),*], |$s:ident| $body:expr) => {
        $fs.call($path, |$s: Arc<SftpSession>| {
            $(let $arg = $arg.clone();)*
            async move { $body }
        })
    };
}

impl Vfs for SftpFs {
    fn id(&self) -> String {
        format!("sftp://{}", self.label)
    }

    fn list(&self, path: &str) -> VfsResult<Vec<Entry>> {
        let wire = self.names.wire(path);
        let listing = request!(self, path, [wire], |s| s.read_dir(wire).await)?;
        let session = self.session();
        Ok(listing
            .map(|item| {
                let name = item.file_name();
                let full = join(&wire, &name);
                self.entry(&session, name, &full, &item.metadata())
            })
            .collect())
    }

    fn stat(&self, path: &str) -> VfsResult<Entry> {
        let wire = self.names.wire(path);
        let attrs: Metadata = request!(self, path, [wire], |s| s.symlink_metadata(wire).await)?;
        let name = rustyfinder_core::paths::file_name(&wire).to_string();
        Ok(self.entry(&self.session(), name, &wire, &attrs))
    }

    fn space(&self, path: &str) -> Option<Space> {
        // Needs the server's statvfs extension (OpenSSH has it).
        let wire = self.names.wire(path);
        let info = request!(self, path, [wire], |s| s.fs_info(wire).await).ok()??;
        Some(Space {
            free: info.blocks_avail.saturating_mul(info.fragment_size),
            total: info.blocks.saturating_mul(info.fragment_size),
        })
    }

    fn mkdir(&self, path: &str) -> VfsResult<()> {
        let wire = self.names.wire(path);
        // Servers answer a plain "failure" for an existing name; tell the
        // caller which failure it was, so it can raise a conflict.
        if self.exists(&wire) {
            return Err(VfsError::AlreadyExists(path.to_string()));
        }
        request!(self, path, [wire], |s| s.create_dir(wire).await)
    }

    fn rename(&self, from: &str, to: &str, replace: bool) -> VfsResult<()> {
        let (old, new) = (self.names.wire(from), self.names.wire(to));
        if self.exists(&new) {
            if !replace {
                return Err(VfsError::AlreadyExists(to.to_string()));
            }
            // Where the server has it, one atomic step: the target is the
            // old item or the new one, never missing (SF-10).
            if request!(self, from, [old, new], |s| s.posix_rename(old, new).await)? {
                return Ok(());
            }
            // The protocol's own rename refuses an existing target, so the
            // old file goes first.
            request!(self, to, [new], |s| s.remove_file(new).await)?;
        }
        request!(self, from, [old, new], |s| s.rename(old, new).await)
    }

    fn remove_file(&self, path: &str) -> VfsResult<()> {
        let wire = self.names.wire(path);
        request!(self, path, [wire], |s| s.remove_file(wire).await)
    }

    fn remove_dir(&self, path: &str) -> VfsResult<()> {
        let wire = self.names.wire(path);
        request!(self, path, [wire], |s| s.remove_dir(wire).await).map_err(|e| match e {
            // "Failure" on a folder that still has children.
            VfsError::Io { .. }
                if request!(self, path, [wire], |s| s.read_dir(wire).await).is_ok_and(|mut d| d.next().is_some()) =>
            {
                VfsError::NotEmpty(path.to_string())
            }
            other => other,
        })
    }

    fn open_read(&self, path: &str) -> VfsResult<Box<dyn Read + Send>> {
        self.open_read_at(path, 0)
    }

    fn create(&self, path: &str) -> VfsResult<Box<dyn Write + Send>> {
        let wire = self.names.wire(path);
        if self.exists(&wire) {
            return Err(VfsError::AlreadyExists(path.to_string()));
        }
        let flags = OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE;
        let file = request!(self, path, [wire], |s| s.open_with_flags(wire, flags).await)?;
        Ok(Box::new(Blocking::new(self, file, path)))
    }

    fn resume_support(&self) -> (bool, bool) {
        // Reads and writes carry an explicit offset in the protocol, so
        // this works with every server, old OpenSSH ones included.
        (true, true)
    }

    fn open_read_at(&self, path: &str, offset: u64) -> VfsResult<Box<dyn Read + Send>> {
        let wire = self.names.wire(path);
        let mut file = request!(self, path, [wire], |s| s.open(wire).await)?;
        if offset > 0 {
            self.runtime.block_on(file.seek(io::SeekFrom::Start(offset))).map_err(|e| VfsError::from_io(path, &e))?;
        }
        Ok(Box::new(Blocking::new(self, file, path)))
    }

    fn open_write_at(&self, path: &str, offset: u64) -> VfsResult<Box<dyn Write + Send>> {
        let wire = self.names.wire(path);
        // Cut what lies past the offset first, so the result is never longer
        // than what is written now.
        let mut attrs = FileAttributes::empty();
        attrs.size = Some(offset);
        request!(self, path, [wire, attrs], |s| s.set_metadata(wire, attrs).await)?;
        let mut file = request!(self, path, [wire], |s| s.open_with_flags(wire, OpenFlags::WRITE).await)?;
        self.runtime.block_on(file.seek(io::SeekFrom::Start(offset))).map_err(|e| VfsError::from_io(path, &e))?;
        Ok(Box::new(Blocking::new(self, file, path)))
    }

    fn set_times(&self, path: &str, modified_ms: Option<i64>, _created_ms: Option<i64>) -> VfsResult<()> {
        // SFTP carries whole seconds, and no creation date.
        let Some(seconds) = modified_ms.and_then(|ms| u32::try_from(ms.div_euclid(1000)).ok()) else {
            return Ok(());
        };
        let mut attrs = FileAttributes::empty();
        attrs.atime = Some(seconds);
        attrs.mtime = Some(seconds);
        let wire = self.names.wire(path);
        request!(self, path, [wire, attrs], |s| s.set_metadata(wire, attrs).await)
    }

    fn set_mode(&self, path: &str, mode: u32) -> VfsResult<()> {
        let mut attrs = FileAttributes::empty();
        attrs.permissions = Some(mode & 0o7777);
        let wire = self.names.wire(path);
        request!(self, path, [wire, attrs], |s| s.set_metadata(wire, attrs).await)
    }

    fn symlink(&self, target: &str, link: &str) -> VfsResult<()> {
        // OpenSSH servers take the two paths in the opposite order to the
        // protocol draft, and OpenSSH's own client sends them that way to
        // every server. Do the same: target first, then the link.
        let (target, wire) = (self.names.wire(target), self.names.wire(link));
        request!(self, link, [target, wire], |s| s.symlink(target, wire).await)
    }

    fn nfc_names(&self) -> bool {
        true
    }

    fn url_for(&self, path: &str) -> Option<String> {
        let mut url = String::from("sftp://");
        if !self.params.user.is_empty() {
            url.push_str(&crate::webdav::encode_path(&self.params.user));
            url.push('@');
        }
        url.push_str(&self.params.host);
        if let Some(port) = self.params.port.filter(|p| *p != 22) {
            url.push_str(&format!(":{port}"));
        }
        url.push_str(&crate::webdav::encode_path(path));
        Some(url)
    }
}

/// Blocking reader/writer over an open remote file.
struct Blocking {
    handle: tokio::runtime::Handle,
    file: Option<File>,
}

impl Blocking {
    fn new(fs: &SftpFs, file: File, _path: &str) -> Self {
        Self { handle: fs.runtime.handle().clone(), file: Some(file) }
    }
}

impl Read for Blocking {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let file = self.file.as_mut().expect("open until dropped");
        self.handle.block_on(file.read(buf))
    }
}

impl Write for Blocking {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let file = self.file.as_mut().expect("open until dropped");
        self.handle.block_on(file.write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        let file = self.file.as_mut().expect("open until dropped");
        self.handle.block_on(file.flush())
    }
}

impl Drop for Blocking {
    fn drop(&mut self) {
        // Close before returning, so a date set right after the upload is
        // not undone by a late write. Nothing to report to: the write
        // itself already succeeded or failed through `write`/`flush`.
        if let Some(mut file) = self.file.take() {
            let _ = self.handle.block_on(async {
                let _ = file.flush().await;
                file.shutdown().await
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> SftpParams {
        SftpParams { host: "example.org".into(), askpass_program: "/x/askpass".into(), ..Default::default() }
    }

    fn has_option(args: &[String], option: &str) -> bool {
        args.windows(2).any(|w| w[0] == "-o" && w[1] == option)
    }

    #[test]
    fn fixed_options_are_always_passed() {
        let args = ssh_arguments(&params());
        for option in [
            "ConnectTimeout=60",
            "ServerAliveInterval=30",
            "ServerAliveCountMax=2",
            "NumberOfPasswordPrompts=1",
            "ForwardX11=no",
            "ClearAllForwardings=yes",
        ] {
            assert!(has_option(&args, option), "{option} missing from {args:?}");
        }
        // "-s" then the host, guarded by "--", then the subsystem name.
        assert_eq!(&args[args.len() - 4..], ["-s", "--", "example.org", "sftp"]);
        // No user given: none passed, so the ssh config can supply it.
        assert!(!args.contains(&"-l".to_string()));
    }

    #[test]
    fn a_password_turns_public_key_authentication_off_and_a_key_file_wins() {
        let with_password = ssh_arguments(&SftpParams { password: Some("p".into()), ..params() });
        assert!(has_option(&with_password, "PubkeyAuthentication=no"));

        let with_key =
            ssh_arguments(&SftpParams { password: Some("p".into()), key_file: Some("/keys/id".into()), ..params() });
        assert!(!has_option(&with_key, "PubkeyAuthentication=no"));
        assert!(has_option(&with_key, "IdentitiesOnly=yes"));
        assert!(with_key.windows(2).any(|w| w[0] == "-i" && w[1] == "/keys/id"));

        let neither = ssh_arguments(&params());
        assert!(!has_option(&neither, "PubkeyAuthentication=no"));
    }

    #[test]
    fn user_port_and_test_overrides_are_passed_through() {
        let args = ssh_arguments(&SftpParams {
            user: "bob".into(),
            port: Some(2222),
            config_file: Some("/t/config".into()),
            known_hosts_file: Some("/t/known".into()),
            ..params()
        });
        assert!(args.windows(2).any(|w| w[0] == "-l" && w[1] == "bob"));
        assert!(args.windows(2).any(|w| w[0] == "-p" && w[1] == "2222"));
        assert!(args.windows(2).any(|w| w[0] == "-F" && w[1] == "/t/config"));
        assert!(has_option(&args, "UserKnownHostsFile=/t/known"));
    }

    #[test]
    fn a_host_that_looks_like_an_option_cannot_inject_one() {
        let args = ssh_arguments(&SftpParams { host: "-oProxyCommand=evil".into(), ..params() });
        let guard = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(args[guard + 1], "-oProxyCommand=evil");
    }

    #[test]
    fn ssh_failures_are_explained() {
        assert_eq!(explain("bob@h: Permission denied (publickey,password).", "h"), ConnectError::AuthFailed);
        assert_eq!(
            explain("ssh: Could not resolve hostname nope: nodename nor servname provided", "nope"),
            ConnectError::CannotResolve("nope".into())
        );
        assert_eq!(explain("ssh: connect to host h port 22: Operation timed out", "h"), ConnectError::Timeout);
        assert_eq!(explain("ssh: connect to host h port 22: Connection refused", "h"), ConnectError::Refused);
        assert_eq!(explain("Host key verification failed.", "h"), ConnectError::HostKeyRejected);
        assert!(matches!(
            explain("@@@\n@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @\nSHA256:abc\nHost key verification failed.", "h"),
            ConnectError::HostKeyChanged(text) if text.contains("SHA256:abc")
        ));
        assert_eq!(explain("something odd\n\n", "h"), ConnectError::Other("something odd".into()));
    }

    #[test]
    fn the_new_fingerprint_is_found_in_the_warning() {
        let text = "The fingerprint for the ED25519 key sent by the remote host is\nSHA256:AbC+/12=.\nPlease contact";
        assert_eq!(changed_host_key_fingerprint(text).as_deref(), Some("SHA256:AbC+/12="));
        assert_eq!(changed_host_key_fingerprint("no key here"), None);
    }

    #[test]
    fn a_legacy_encoding_round_trips_names_exactly() {
        let names = Names::new(Some("windows-1251"));
        // What the protocol layer hands over for CP1251 bytes.
        let (raw, _, _) = encoding_rs::WINDOWS_1251.encode("Привет.txt");
        let wire = russh_sftp::escape_bytes(&raw);
        assert_eq!(names.local(&wire), "Привет.txt");
        assert_eq!(names.wire("Привет.txt"), wire);
        assert_eq!(names.wire("/home/plain"), "/home/plain");

        // Without an encoding, names pass through as they are.
        let utf8 = Names::new(None);
        assert_eq!(utf8.local("résumé"), "résumé");
        assert_eq!(utf8.wire("résumé"), "résumé");
        // Bytes that are not text stay addressable either way.
        let odd = russh_sftp::escape_bytes(&[0xff, b'A']);
        assert_eq!(russh_sftp::unescape_bytes(&odd), [0xff, b'A']);
        assert_eq!(utf8.wire(&odd), odd);
    }

    #[test]
    fn the_override_runs_ssh_with_the_same_options_as_the_connection() {
        let options = ssh_options(&SftpParams { port: Some(2222), ..params() });
        assert!(options.windows(2).any(|w| w[0] == "-p" && w[1] == "2222"));
        assert!(!options.contains(&"sftp".to_string()), "options only, no subsystem");
    }
}
