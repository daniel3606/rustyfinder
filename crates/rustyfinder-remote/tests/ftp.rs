//! The FTP backend against a scripted server running in this process
//! (see `support/ftpd.rs`).

#[path = "support/ftpd.rs"]
mod ftpd;

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use ftpd::{Config, Tls};
use rustyfinder_core::ops::{
    ConflictAnswer, ConflictChoice, ConflictInfo, Engine, ErrorChoice, ErrorInfo, OpContext, Partials, Policies,
    Progress, TransferMode,
};
use rustyfinder_core::{Vfs, VfsError};
use rustyfinder_remote::ftp::{connect, FtpFs, FtpParams, FtpSecurity};
use rustyfinder_remote::RemoteError;

struct Server {
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
    root: tempfile::TempDir,
    /// Close the control connection instead of answering the next command.
    drop_next: Arc<AtomicBool>,
}

impl Server {
    fn start(config: Config) -> Server {
        let log = Arc::new(Mutex::new(Vec::new()));
        let root = tempfile::tempdir().unwrap();
        let drop_next = Arc::new(AtomicBool::new(false));
        let port = ftpd::start(root.path().to_path_buf(), config, Arc::clone(&log), Arc::clone(&drop_next));
        Server { port, log, root, drop_next }
    }

    fn params(&self) -> FtpParams {
        FtpParams {
            host: "127.0.0.1".into(),
            port: Some(self.port),
            user: "bob".into(),
            password: "secret".into(),
            passive: true,
            ..FtpParams::default()
        }
    }

    fn connect(&self) -> FtpFs {
        connect(self.params()).expect("connect")
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.path().join(rel)
    }

    fn sent(&self, verb: &str) -> bool {
        self.log.lock().unwrap().iter().any(|line| line.split(' ').next() == Some(verb))
    }
}

fn names(fs: &dyn Vfs, dir: &str) -> Vec<String> {
    let mut names: Vec<String> = fs.list(dir).unwrap().into_iter().map(|e| e.name).collect();
    names.sort();
    names
}

fn download(fs: &dyn Vfs, path: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    fs.open_read(path).unwrap().read_to_end(&mut bytes).unwrap();
    bytes
}

fn upload(fs: &dyn Vfs, path: &str, content: &[u8]) {
    let mut writer = fs.create(path).unwrap();
    writer.write_all(content).unwrap();
    writer.flush().unwrap();
}

#[test]
fn listing_and_the_everyday_operations() {
    let server = Server::start(Config::default());
    fs::create_dir(server.path("docs")).unwrap();
    fs::write(server.path("hello world.txt"), "remote hello").unwrap();
    let ftp = server.connect();
    assert_eq!(ftp.home(), "/");
    assert_eq!(ftp.label(), "bob@127.0.0.1");
    assert_eq!(names(&ftp, "/"), ["docs", "hello world.txt"]);

    let entry = ftp.stat("/hello world.txt").unwrap();
    assert_eq!((entry.size, entry.is_dir), (12, false));
    assert!(ftp.stat("/docs").unwrap().is_dir && ftp.stat("/").unwrap().is_dir);
    assert!(matches!(ftp.stat("/nope"), Err(VfsError::NotFound(_))));

    assert_eq!(download(&ftp, "/hello world.txt"), b"remote hello");
    upload(&ftp, "/docs/new file.bin", &[0, 1, 2, 255, 13, 10]);
    assert_eq!(
        fs::read(server.path("docs/new file.bin")).unwrap(),
        [0, 1, 2, 255, 13, 10],
        "binary mode: bytes untouched"
    );
    assert!(matches!(ftp.create("/docs/new file.bin"), Err(VfsError::AlreadyExists(_))));

    ftp.mkdir("/made").unwrap();
    assert!(matches!(ftp.mkdir("/made"), Err(VfsError::AlreadyExists(_))));
    ftp.rename("/made", "/renamed", false).unwrap();
    assert!(matches!(ftp.rename("/renamed", "/docs", false), Err(VfsError::AlreadyExists(_))));
    assert_eq!(names(&ftp, "/"), ["docs", "hello world.txt", "renamed"]);
    ftp.remove_dir("/renamed").unwrap();
    ftp.remove_file("/hello world.txt").unwrap();
    assert_eq!(names(&ftp, "/"), ["docs"]);

    // FT-07: passive mode never sends an active-mode command.
    assert!(!server.sent("PORT") && !server.sent("EPRT"));
    assert!(server.sent("EPSV") && server.sent("MLSD"));
}

#[test]
fn ft01_a_private_address_in_the_pasv_reply_is_not_used() {
    let server = Server::start(Config { epsv: false, private_pasv: true, ..Config::default() });
    fs::write(server.path("a.txt"), "behind nat").unwrap();
    let ftp = server.connect();
    assert_eq!(names(&ftp, "/"), ["a.txt"]);
    assert_eq!(download(&ftp, "/a.txt"), b"behind nat");
    // Extended passive was refused once, then plain PASV was used.
    assert!(server.sent("PASV"));
}

#[test]
fn ft04_names_in_a_legacy_encoding_are_readable_and_usable() {
    let server = Server::start(Config {
        utf8: false,
        mlsd: false,
        legacy: Some(encoding_rs::WINDOWS_1251),
        ..Config::default()
    });
    fs::write(server.path("Привет мир, это файл.txt"), "cyrillic").unwrap();
    fs::write(server.path("plain.txt"), "p").unwrap();
    let ftp = server.connect();
    assert_eq!(names(&ftp, "/"), ["plain.txt", "Привет мир, это файл.txt"]);
    assert_eq!(download(&ftp, "/Привет мир, это файл.txt"), b"cyrillic");
    upload(&ftp, "/Новый.txt", b"new");
    assert_eq!(fs::read(server.path("Новый.txt")).unwrap(), b"new");
}

#[test]
fn ft05_windows_style_listings_are_parsed() {
    let server = Server::start(Config { mlsd: false, dos_list: true, ..Config::default() });
    fs::create_dir(server.path("My Docs")).unwrap();
    fs::write(server.path("report 1.pdf"), vec![0u8; 2048]).unwrap();
    let ftp = server.connect();
    let mut entries = ftp.list("/").unwrap();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(entries.len(), 2);
    assert_eq!((entries[0].name.as_str(), entries[0].is_dir), ("My Docs", true));
    assert_eq!((entries[1].name.as_str(), entries[1].size, entries[1].is_dir), ("report 1.pdf", 2048, false));
    assert!(entries[1].modified_ms.is_some());
}

#[test]
fn unix_listings_skip_the_total_line_and_keep_permissions() {
    let server = Server::start(Config { mlsd: false, ..Config::default() });
    fs::create_dir(server.path("sub")).unwrap();
    fs::write(server.path("sub/inner.txt"), "abc").unwrap();
    let ftp = server.connect();
    assert_eq!(names(&ftp, "/"), ["sub"]);
    let inner = ftp.list("/sub").unwrap();
    assert_eq!((inner[0].name.as_str(), inner[0].size, inner[0].mode), ("inner.txt", 3, Some(0o644)));
    // Without MLST, stat reads the parent's listing.
    assert_eq!(ftp.stat("/sub/inner.txt").unwrap().size, 3);
}

#[test]
fn ft09_the_modification_time_is_set_after_an_upload() {
    let server = Server::start(Config::default());
    let ftp = server.connect();
    upload(&ftp, "/dated.txt", b"x");
    let when_ms = 1_577_836_800_000; // 2020-01-01T00:00:00Z
    ftp.set_times("/dated.txt", Some(when_ms), None).unwrap();
    let on_server = fs::metadata(server.path("dated.txt")).unwrap().modified().unwrap();
    assert_eq!(on_server, UNIX_EPOCH + Duration::from_millis(when_ms as u64));
    assert_eq!(ftp.stat("/dated.txt").unwrap().modified_ms, Some(when_ms));
    assert_eq!(ftp.list("/").unwrap()[0].modified_ms, Some(when_ms));
}

#[test]
fn without_mfmt_setting_the_time_is_quietly_skipped() {
    let server = Server::start(Config { mfmt: false, ..Config::default() });
    let ftp = server.connect();
    upload(&ftp, "/a.txt", b"x");
    assert!(ftp.set_times("/a.txt", Some(1_577_836_800_000), None).is_ok());
    assert!(!server.sent("MFMT"));
}

#[test]
fn a_wrong_password_is_an_authorization_error() {
    let server = Server::start(Config::default());
    let params = FtpParams { password: "wrong".into(), ..server.params() };
    assert!(matches!(connect(params), Err(RemoteError::Auth(_))));
}

#[test]
fn nothing_listening_is_an_unreachable_error() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let params = FtpParams { host: "127.0.0.1".into(), port: Some(port), passive: true, ..FtpParams::default() };
    assert!(matches!(connect(params), Err(RemoteError::Unreachable(_))));
}

#[test]
fn a_dropped_control_connection_reconnects_on_next_use() {
    let server = Server::start(Config::default());
    fs::write(server.path("a.txt"), "still here").unwrap();
    let ftp = server.connect();
    server.drop_next.store(true, Ordering::SeqCst);
    assert_eq!(names(&ftp, "/"), ["a.txt"]);
    server.drop_next.store(true, Ordering::SeqCst);
    assert_eq!(download(&ftp, "/a.txt"), b"still here");
}

#[test]
fn a_download_that_is_dropped_halfway_leaves_the_connection_usable() {
    let server = Server::start(Config::default());
    fs::write(server.path("big.bin"), vec![7u8; 400_000]).unwrap();
    let ftp = server.connect();
    {
        let mut reader = ftp.open_read("/big.bin").unwrap();
        let mut some = [0u8; 1000];
        reader.read_exact(&mut some).unwrap();
    }
    assert_eq!(names(&ftp, "/"), ["big.bin"]);
    assert_eq!(download(&ftp, "/big.bin").len(), 400_000);
}

#[test]
fn two_transfers_at_once_use_two_sessions() {
    let server = Server::start(Config::default());
    fs::write(server.path("a.txt"), "aaa").unwrap();
    fs::write(server.path("b.txt"), "bbb").unwrap();
    let ftp = server.connect();
    let mut a = ftp.open_read("/a.txt").unwrap();
    let mut b = ftp.open_read("/b.txt").unwrap();
    // A listing while both are open needs a third.
    assert_eq!(names(&ftp, "/"), ["a.txt", "b.txt"]);
    let (mut ta, mut tb) = (String::new(), String::new());
    b.read_to_string(&mut tb).unwrap();
    a.read_to_string(&mut ta).unwrap();
    assert_eq!((ta.as_str(), tb.as_str()), ("aaa", "bbb"));
}

// ------------------------------------------------------------------ TLS

fn fixture_fingerprint() -> String {
    rustyfinder_remote::trust::fingerprint(&ftpd::certificate())
}

#[test]
fn ft02_ft03_explicit_tls_names_the_certificate_and_trusting_it_works() {
    let server = Server::start(Config { tls: Some(Tls::Explicit), ..Config::default() });
    fs::write(server.path("secret.txt"), "over tls").unwrap();
    let params = FtpParams { security: FtpSecurity::Explicit, ..server.params() };

    // Self-signed: refused, with the fingerprint the user can check.
    let message = match connect(params.clone()) {
        Err(RemoteError::Certificate(message)) => message,
        other => panic!("expected a certificate error, got {:?}", other.err()),
    };
    assert_eq!(rustyfinder_remote::trust::fingerprint_in(&message), Some(fixture_fingerprint()));

    // Trusted: control and data connections are both TLS, and work.
    let trusted = FtpParams { trusted_fingerprint: Some(fixture_fingerprint()), ..params.clone() };
    let ftp = connect(trusted).expect("connect with the trusted certificate");
    assert_eq!(names(&ftp, "/"), ["secret.txt"]);
    assert_eq!(download(&ftp, "/secret.txt"), b"over tls");
    upload(&ftp, "/up.txt", b"sent over tls");
    assert_eq!(fs::read(server.path("up.txt")).unwrap(), b"sent over tls");
    assert!(server.sent("AUTH") && server.sent("PROT"));

    // Another certificate than the one trusted is refused.
    let wrong = FtpParams { trusted_fingerprint: Some("AA:BB".into()), ..params };
    assert!(matches!(connect(wrong), Err(RemoteError::Certificate(m)) if m.contains("not the one you chose to trust")));
}

#[test]
fn implicit_tls_speaks_tls_from_the_first_byte() {
    let server = Server::start(Config { tls: Some(Tls::Implicit), ..Config::default() });
    fs::write(server.path("a.txt"), "implicit").unwrap();
    let params = FtpParams { security: FtpSecurity::Implicit, ..server.params() };
    let message = match connect(params.clone()) {
        Err(RemoteError::Certificate(message)) => message,
        other => panic!("expected a certificate error, got {:?}", other.err()),
    };
    assert_eq!(rustyfinder_remote::trust::fingerprint_in(&message), Some(fixture_fingerprint()));
    let ftp = connect(FtpParams { trusted_fingerprint: Some(fixture_fingerprint()), ..params }).expect("connect");
    assert_eq!(download(&ftp, "/a.txt"), b"implicit");
    assert!(!server.sent("AUTH"), "no AUTH TLS: the connection was TLS already");
    assert_eq!(ftp.url_for("/a b.txt").unwrap(), format!("ftps://bob@127.0.0.1:{}/a%20b.txt", server.port));
}

// --------------------------------------------- Listings, SITE, timeouts

#[test]
fn eplf_listings_are_parsed() {
    let server = Server::start(Config { mlsd: false, eplf: true, ..Config::default() });
    fs::create_dir(server.path("docs")).unwrap();
    fs::write(server.path("notes.txt"), "12345").unwrap();
    let ftp = server.connect();
    let mut entries = ftp.list("/").unwrap();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!((entries[0].name.as_str(), entries[0].is_dir), ("docs", true));
    assert_eq!((entries[1].name.as_str(), entries[1].size, entries[1].mode), ("notes.txt", 5, Some(0o644)));
    assert!(entries[1].modified_ms.is_some());
}

#[test]
fn permissions_go_through_site_chmod_where_the_server_has_it() {
    use std::os::unix::fs::PermissionsExt;
    let server = Server::start(Config::default());
    fs::write(server.path("f"), "x").unwrap();
    let ftp = server.connect();
    ftp.set_mode("/f", 0o600).unwrap();
    assert_eq!(fs::metadata(server.path("f")).unwrap().permissions().mode() & 0o777, 0o600);

    let without = Server::start(Config { site_chmod: false, ..Config::default() });
    fs::write(without.path("f"), "x").unwrap();
    assert!(matches!(without.connect().set_mode("/f", 0o600), Err(VfsError::Unsupported(_))));
}

#[test]
fn without_mfmt_the_time_is_set_with_site_utime() {
    let server = Server::start(Config { mfmt: false, site_utime: true, ..Config::default() });
    let ftp = server.connect();
    upload(&ftp, "/dated.txt", b"x");
    let when_ms = 1_577_836_800_000;
    ftp.set_times("/dated.txt", Some(when_ms), None).unwrap();
    let on_server = fs::metadata(server.path("dated.txt")).unwrap().modified().unwrap();
    assert_eq!(on_server, UNIX_EPOCH + Duration::from_millis(when_ms as u64));
    assert!(server.log.lock().unwrap().iter().any(|l| l.starts_with("SITE UTIME /dated.txt 20200101000000")));
}

#[test]
fn a_stalled_data_connection_fails_within_the_data_timeout() {
    let server = Server::start(Config { stall_retr: true, ..Config::default() });
    fs::write(server.path("a.txt"), "never arrives").unwrap();
    let ftp = connect(FtpParams { data_timeout: Some(Duration::from_secs(1)), ..server.params() }).unwrap();
    let started = std::time::Instant::now();
    let result = ftp.open_read("/a.txt").and_then(|mut r| {
        let mut bytes = Vec::new();
        r.read_to_end(&mut bytes).map_err(|e| VfsError::Io { path: "/a.txt".into(), message: e.to_string() })
    });
    assert!(result.is_err(), "a download from a silent data connection must fail");
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
}

#[test]
fn idle_sessions_get_a_noop_keep_alive() {
    let server = Server::start(Config::default());
    let ftp = connect(FtpParams { keepalive: Some(Duration::from_millis(200)), ..server.params() }).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(server.sent("NOOP"));
    // And the session still works afterwards.
    assert!(names(&ftp, "/").is_empty());
}

// ---------------------------------------------------------------- Resume

/// Stops after `0` bytes; answers Resume.
struct StopAt(u64, Mutex<Progress>);

impl OpContext for StopAt {
    fn checkpoint(&self) -> bool {
        self.1.lock().unwrap().bytes_done < self.0
    }
    fn progress(&self, progress: &Progress) {
        *self.1.lock().unwrap() = progress.clone();
    }
    fn conflict(&self, info: &ConflictInfo) -> ConflictAnswer {
        assert!(info.resumable, "the partial file should be resumable");
        ConflictAnswer { choice: ConflictChoice::Resume, apply_to_all: false }
    }
    fn error(&self, info: &ErrorInfo) -> ErrorChoice {
        panic!("unexpected error on {}: {}", info.path, info.message);
    }
}

fn engine<'a>(ctx: &'a StopAt, from: &Arc<dyn Vfs>, to: &Arc<dyn Vfs>, partials: &Arc<Partials>) -> Engine<'a> {
    Engine::new(ctx, Arc::clone(from), Arc::clone(to), false, Policies::default())
        .with_partials(Some(Arc::clone(partials)))
}

#[test]
fn ft06_a_stopped_download_resumes_with_rest_and_matches() {
    let server = Server::start(Config::default());
    let data: Vec<u8> = (0..3 * 1024 * 1024u32).map(|i| (i % 253) as u8).collect();
    fs::write(server.path("big.bin"), &data).unwrap();
    let remote: Arc<dyn Vfs> = Arc::new(server.connect());
    let local: Arc<dyn Vfs> = Arc::new(rustyfinder_core::LocalFs);
    let partials = Arc::new(Partials::in_memory());
    let here = tempfile::tempdir().unwrap();
    let dest = here.path().to_str().unwrap().to_string();

    let stop = StopAt(data.len() as u64 / 2, Mutex::default());
    assert!(
        engine(&stop, &remote, &local, &partials)
            .transfer(TransferMode::Copy, &["/big.bin".into()], &dest, None)
            .unwrap()
            .stopped
    );
    let kept = fs::metadata(here.path().join("big.bin")).unwrap().len();
    assert!(kept > 0 && kept < data.len() as u64);

    let go = StopAt(u64::MAX, Mutex::default());
    let out = engine(&go, &remote, &local, &partials)
        .transfer(TransferMode::Copy, &["/big.bin".into()], &dest, None)
        .unwrap();
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert_eq!(fs::read(here.path().join("big.bin")).unwrap(), data, "hashes match");
    assert!(server.log.lock().unwrap().iter().any(|l| *l == format!("REST {kept}")), "REST {kept} was sent");
}

#[test]
fn a_stopped_upload_resumes_with_rest_or_appe() {
    for rest in [true, false] {
        let server = Server::start(Config { rest, ..Config::default() });
        let data: Vec<u8> = (0..3 * 1024 * 1024u32).map(|i| (i % 241) as u8).collect();
        let here = tempfile::tempdir().unwrap();
        fs::write(here.path().join("up.bin"), &data).unwrap();
        let source = here.path().join("up.bin").to_str().unwrap().to_string();
        let remote: Arc<dyn Vfs> = Arc::new(server.connect());
        let local: Arc<dyn Vfs> = Arc::new(rustyfinder_core::LocalFs);
        let partials = Arc::new(Partials::in_memory());

        let stop = StopAt(data.len() as u64 / 2, Mutex::default());
        let out = engine(&stop, &local, &remote, &partials).transfer(
            TransferMode::Copy,
            std::slice::from_ref(&source),
            "/",
            None,
        );
        assert!(out.unwrap().stopped);
        assert!(fs::metadata(server.path("up.bin")).unwrap().len() < data.len() as u64);

        let go = StopAt(u64::MAX, Mutex::default());
        let out = engine(&go, &local, &remote, &partials).transfer(
            TransferMode::Copy,
            std::slice::from_ref(&source),
            "/",
            None,
        );
        assert!(out.unwrap().failures.is_empty());
        assert_eq!(fs::read(server.path("up.bin")).unwrap(), data, "rest={rest}");
        assert_eq!(server.sent("APPE"), !rest, "APPE only without REST");
    }
}
