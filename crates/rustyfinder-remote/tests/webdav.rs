//! The WebDAV backend against a scripted server running in this process
//! (see `support/davd.rs`).

#[path = "support/davd.rs"]
mod davd;

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use davd::{AuthMode, Config};
use rustyfinder_core::{Vfs, VfsError};
use rustyfinder_remote::webdav::{self, connect, WebDavFs, WebDavParams};
use rustyfinder_remote::RemoteError;

struct Server {
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
    root: tempfile::TempDir,
}

impl Server {
    fn start(config: Config) -> Server {
        let log = Arc::new(Mutex::new(Vec::new()));
        let root = tempfile::tempdir().unwrap();
        let port = davd::start(root.path().to_path_buf(), config, Arc::clone(&log));
        Server { port, log, root }
    }

    fn params(&self) -> WebDavParams {
        WebDavParams {
            host: "127.0.0.1".into(),
            port: Some(self.port),
            user: "bob".into(),
            password: "secret".into(),
            // The test server speaks plain HTTP on this Mac only.
            allow_insecure_basic: true,
            ..WebDavParams::default()
        }
    }

    fn connect(&self) -> WebDavFs {
        connect(self.params()).expect("connect")
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.path().join(rel)
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
    fs::write(server.path("hello.txt"), "remote hello").unwrap();
    let dav = server.connect();
    assert_eq!((dav.home(), dav.label()), ("/", "bob@127.0.0.1"));
    assert_eq!(names(&dav, "/"), ["docs", "hello.txt"]);

    let entry = dav.stat("/hello.txt").unwrap();
    assert_eq!((entry.name.as_str(), entry.size, entry.is_dir), ("hello.txt", 12, false));
    assert!(entry.modified_ms.is_some() && entry.created_ms.is_some());
    assert!(dav.stat("/docs").unwrap().is_dir);
    assert!(matches!(dav.stat("/nope"), Err(VfsError::NotFound(_))));
    assert!(matches!(dav.list("/nope"), Err(VfsError::NotFound(_))));

    assert_eq!(download(&dav, "/hello.txt"), b"remote hello");
    upload(&dav, "/docs/new.bin", &[0, 1, 2, 255]);
    assert_eq!(fs::read(server.path("docs/new.bin")).unwrap(), [0, 1, 2, 255]);
    assert!(matches!(dav.create("/docs/new.bin"), Err(VfsError::AlreadyExists(_))));

    dav.mkdir("/made").unwrap();
    assert!(matches!(dav.mkdir("/made"), Err(VfsError::AlreadyExists(_))));
    dav.rename("/made", "/renamed", false).unwrap();
    assert_eq!(names(&dav, "/"), ["docs", "hello.txt", "renamed"]);
    dav.remove_dir("/renamed").unwrap();
    dav.remove_file("/hello.txt").unwrap();
    assert_eq!(names(&dav, "/"), ["docs"]);

    // Listings never ask for the whole tree at once.
    let log = server.log.lock().unwrap();
    assert!(log.iter().any(|l| l.starts_with("PROPFIND") && l.ends_with("depth=1")));
    assert!(!log.iter().any(|l| l.contains("depth=infinity")));
}

#[test]
fn wd02_an_anonymous_share_needs_no_user_name() {
    let server = Server::start(Config { auth: AuthMode::None, ..Config::default() });
    fs::write(server.path("public.txt"), "open").unwrap();
    let params = WebDavParams { user: String::new(), password: String::new(), ..server.params() };
    let dav = connect(params).unwrap();
    assert_eq!(dav.label(), "127.0.0.1");
    assert_eq!(names(&dav, "/"), ["public.txt"]);
}

#[test]
fn wd03_names_with_spaces_hashes_and_percent_signs_work() {
    let server = Server::start(Config::default());
    fs::create_dir(server.path("My Docs #1")).unwrap();
    fs::write(server.path("My Docs #1/50% off? résumé.txt"), "odd").unwrap();
    let dav = server.connect();
    assert_eq!(names(&dav, "/"), ["My Docs #1"]);
    assert_eq!(names(&dav, "/My Docs #1"), ["50% off? résumé.txt"]);
    assert_eq!(download(&dav, "/My Docs #1/50% off? résumé.txt"), b"odd");
    upload(&dav, "/My Docs #1/new #2.txt", b"n");
    dav.rename("/My Docs #1/new #2.txt", "/My Docs #1/moved 100%.txt", false).unwrap();
    assert_eq!(fs::read(server.path("My Docs #1/moved 100%.txt")).unwrap(), b"n");
}

#[test]
fn wd04_renaming_onto_a_taken_name_is_refused_then_replaces_on_request() {
    let server = Server::start(Config::default());
    fs::write(server.path("a.txt"), "from a").unwrap();
    fs::write(server.path("b.txt"), "old b").unwrap();
    let dav = server.connect();
    // "Overwrite: F": the server says the name exists, nothing changes.
    assert!(matches!(dav.rename("/a.txt", "/b.txt", false), Err(VfsError::AlreadyExists(p)) if p == "/b.txt"));
    assert_eq!(fs::read_to_string(server.path("b.txt")).unwrap(), "old b");
    // After the user chose Replace.
    dav.rename("/a.txt", "/b.txt", true).unwrap();
    assert_eq!(fs::read_to_string(server.path("b.txt")).unwrap(), "from a");
    assert!(!server.path("a.txt").exists());
}

#[test]
fn wd05_a_large_upload_is_streamed_and_arrives_whole() {
    let server = Server::start(Config::default());
    let dav = server.connect();
    let block = vec![0xABu8; 1 << 16];
    let mut writer = dav.create("/big.bin").unwrap();
    for _ in 0..128 {
        writer.write_all(&block).unwrap();
    }
    writer.flush().unwrap();
    assert_eq!(fs::metadata(server.path("big.bin")).unwrap().len(), 128 << 16);
    assert_eq!(dav.stat("/big.bin").unwrap().size, 128 << 16);
    assert_eq!(download(&dav, "/big.bin").len(), 128 << 16);
}

#[test]
fn wd06_a_server_without_creation_dates_still_lists() {
    let server = Server::start(Config { omit_creationdate: true, ..Config::default() });
    fs::write(server.path("a.txt"), "a").unwrap();
    let dav = server.connect();
    let entries = dav.list("/").unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].created_ms.is_none() && entries[0].modified_ms.is_some());
}

#[test]
fn a_copy_on_the_same_server_is_done_by_the_server() {
    let server = Server::start(Config::default());
    fs::write(server.path("a.txt"), "copy me").unwrap();
    fs::write(server.path("taken.txt"), "t").unwrap();
    let dav = server.connect();
    assert!(dav.copy_within("/a.txt", "/a copy.txt", &mut |_| true).unwrap());
    assert_eq!(fs::read_to_string(server.path("a copy.txt")).unwrap(), "copy me");
    assert!(matches!(dav.copy_within("/a.txt", "/taken.txt", &mut |_| true), Err(VfsError::AlreadyExists(_))));
    assert!(server.log.lock().unwrap().iter().any(|l| l.starts_with("COPY /a.txt")));
}

#[test]
fn digest_authentication_works_for_every_kind_of_request() {
    let server = Server::start(Config { auth: AuthMode::Digest, ..Config::default() });
    fs::write(server.path("a.txt"), "digest").unwrap();
    let dav = server.connect();
    assert_eq!(names(&dav, "/"), ["a.txt"]);
    assert_eq!(download(&dav, "/a.txt"), b"digest");
    upload(&dav, "/up.txt", b"sent with digest");
    assert_eq!(fs::read(server.path("up.txt")).unwrap(), b"sent with digest");
    dav.mkdir("/dir with space").unwrap();
    dav.rename("/up.txt", "/dir with space/up.txt", false).unwrap();
    assert!(server.path("dir with space/up.txt").exists());
}

#[test]
fn wrong_or_missing_credentials_are_authorization_errors() {
    for auth in [AuthMode::Basic, AuthMode::Digest] {
        let server = Server::start(Config { auth, ..Config::default() });
        let wrong = WebDavParams { password: "wrong".into(), ..server.params() };
        assert!(matches!(connect(wrong), Err(RemoteError::Auth(_))));
        let nobody = WebDavParams { user: String::new(), ..server.params() };
        assert!(matches!(connect(nobody), Err(RemoteError::Auth(_))));
    }
}

#[test]
fn the_remote_path_is_the_connections_top_and_must_exist() {
    let server = Server::start(Config::default());
    fs::create_dir_all(server.path("remote.php/dav/files/bob")).unwrap();
    fs::write(server.path("remote.php/dav/files/bob/mine.txt"), "m").unwrap();
    let params = WebDavParams { remote_path: Some("/remote.php/dav/files/bob/".into()), ..server.params() };
    let dav = connect(params).unwrap();
    assert_eq!(dav.home(), "/remote.php/dav/files/bob");
    assert_eq!(names(&dav, dav.home()), ["mine.txt"]);

    let missing = WebDavParams { remote_path: Some("/no/such/place".into()), ..server.params() };
    match connect(missing) {
        Err(RemoteError::Other(message)) => assert!(message.contains("/no/such/place"), "{message}"),
        other => panic!("expected an error naming the folder, got {:?}", other.err()),
    }
}

// ------------------------------------------------------ Security

#[test]
fn basic_authentication_over_plain_http_needs_the_users_consent() {
    let server = Server::start(Config::default());
    let unasked = WebDavParams { allow_insecure_basic: false, ..server.params() };
    match connect(unasked) {
        Err(RemoteError::Auth(message)) => assert!(message.starts_with(webdav::INSECURE_BASIC), "{message}"),
        other => panic!("expected a warning, got {:?}", other.err()),
    }
    // No password went out: only the unauthenticated first request.
    assert!(server.log.lock().unwrap().iter().all(|l| l.starts_with("PROPFIND")));
    assert_eq!(server.log.lock().unwrap().len(), 1);
    // Digest never sends the password itself, so it needs no warning.
    let digest = Server::start(Config { auth: AuthMode::Digest, ..Config::default() });
    assert!(connect(WebDavParams { allow_insecure_basic: false, ..digest.params() }).is_ok());
}

#[test]
fn https_names_an_unverifiable_certificate_and_a_trusted_one_works() {
    let server = Server::start(Config { tls: true, ..Config::default() });
    fs::write(server.path("a.txt"), "over https").unwrap();
    let params = WebDavParams { host: "localhost".into(), https: true, allow_insecure_basic: false, ..server.params() };
    let fingerprint = rustyfinder_remote::trust::fingerprint(&davd::certificate());
    match connect(params.clone()) {
        Err(RemoteError::Certificate(message)) => {
            assert_eq!(rustyfinder_remote::trust::fingerprint_in(&message), Some(fingerprint));
        }
        other => panic!("expected a certificate error, got {:?}", other.err()),
    }
    let dav = connect(WebDavParams {
        trusted_fingerprint: Some(rustyfinder_remote::trust::fingerprint(&davd::certificate())),
        ..params
    })
    .expect("trusted");
    assert_eq!(names(&dav, "/"), ["a.txt"]);
    assert_eq!(download(&dav, "/a.txt"), b"over https");
    upload(&dav, "/b.txt", b"sent over https");
    assert_eq!(fs::read(server.path("b.txt")).unwrap(), b"sent over https");
    assert_eq!(dav.url_for("/a b.txt").unwrap(), format!("https://localhost:{}/a%20b.txt", server.port));
}

// ------------------------------------------------- Dates, resume, busy

#[test]
fn the_modification_time_is_set_with_proppatch() {
    let server = Server::start(Config::default());
    let dav = server.connect();
    upload(&dav, "/dated.txt", b"x");
    dav.set_times("/dated.txt", Some(1_577_836_800_000), None).unwrap();
    let on_server = fs::metadata(server.path("dated.txt")).unwrap().modified().unwrap();
    assert_eq!(on_server, std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_577_836_800));
    assert_eq!(dav.stat("/dated.txt").unwrap().modified_ms, Some(1_577_836_800_000));
}

#[test]
fn a_download_resumes_with_a_range_request_or_skips_ahead_without_one() {
    for ranges in [true, false] {
        let server = Server::start(Config { ranges, ..Config::default() });
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(server.path("big.bin"), &data).unwrap();
        let dav = server.connect();
        let mut rest = Vec::new();
        dav.open_read_at("/big.bin", 40_000).unwrap().read_to_end(&mut rest).unwrap();
        assert_eq!(rest, &data[40_000..], "ranges={ranges}");
        assert!(server.log.lock().unwrap().iter().any(|l| l.ends_with("range=bytes=40000-")));
    }
}

#[test]
fn a_busy_server_is_asked_again_after_its_retry_after() {
    use rustyfinder_core::ops::{Engine, Policies, TransferMode};
    let server = Server::start(Config { busy_gets: 2, ..Config::default() });
    fs::write(server.path("a.txt"), "eventually").unwrap();
    let dav: Arc<dyn Vfs> = Arc::new(server.connect());
    assert!(matches!(dav.open_read("/a.txt"), Err(VfsError::Throttled { retry_after_secs: 0, .. })));
    // The engine waits as asked and tries again, without a prompt.
    let here = tempfile::tempdir().unwrap();
    let ctx = NoPrompts;
    let out = Engine::new(&ctx, Arc::clone(&dav), Arc::new(rustyfinder_core::LocalFs), false, Policies::default())
        .transfer(TransferMode::Copy, &["/a.txt".into()], here.path().to_str().unwrap(), None)
        .unwrap();
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert_eq!(fs::read(here.path().join("a.txt")).unwrap(), b"eventually");
}

struct NoPrompts;

impl rustyfinder_core::ops::OpContext for NoPrompts {
    fn checkpoint(&self) -> bool {
        true
    }
    fn progress(&self, _: &rustyfinder_core::ops::Progress) {}
    fn conflict(&self, info: &rustyfinder_core::ops::ConflictInfo) -> rustyfinder_core::ops::ConflictAnswer {
        panic!("unexpected conflict on {}", info.target_path);
    }
    fn error(&self, info: &rustyfinder_core::ops::ErrorInfo) -> rustyfinder_core::ops::ErrorChoice {
        panic!("unexpected error on {}: {}", info.path, info.message);
    }
}

#[test]
fn nothing_listening_is_an_unreachable_error() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let params = WebDavParams { host: "127.0.0.1".into(), port: Some(port), ..WebDavParams::default() };
    assert!(matches!(connect(params), Err(RemoteError::Unreachable(_))));
}
