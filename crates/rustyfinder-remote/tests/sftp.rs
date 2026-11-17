//! SFTP against a real `sshd`.
//!
//! The fixture is the one the spec describes: a throwaway OpenSSH server on
//! 127.0.0.1 and a free high port, run as the current user, with its own
//! host key, config and authorized key in a temp folder. The client is
//! pointed at a temp ssh config and known_hosts, so the real `~/.ssh` is
//! never read or written. If `sshd` cannot start here, every test skips
//! itself and says so.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustyfinder_core::ops::{
    ConflictAnswer, ConflictChoice, ConflictInfo, Engine, ErrorChoice, ErrorInfo, OpContext, Policies, Progress,
    TransferMode,
};
use rustyfinder_core::{EntryKind, LocalFs, Vfs, VfsError};
use rustyfinder_remote::{connect, ConnectError, PromptKind, Prompter, SftpFs, SftpParams};

struct Server {
    dir: tempfile::TempDir,
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn keygen(path: &Path, passphrase: &str) {
    let ok = Command::new("/usr/bin/ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", passphrase, "-f"])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(ok, "ssh-keygen failed");
}

impl Server {
    /// Start sshd, or return `None` if this machine will not run one.
    fn start() -> Option<Self> {
        let dir = tempfile::tempdir().ok()?;
        let p = |name: &str| dir.path().join(name);
        fs::create_dir(p("root")).ok()?;
        keygen(&p("host_key"), "");
        keygen(&p("client_key"), "");
        keygen(&p("locked_key"), "open sesame");
        let mut authorized = fs::read_to_string(p("client_key.pub")).ok()?;
        authorized.push_str(&fs::read_to_string(p("locked_key.pub")).ok()?);
        fs::write(p("authorized_keys"), authorized).ok()?;

        // Tests run in parallel and each picks a free port, so two can
        // pick the same one. The loser's sshd exits at once; a connection
        // to that port would then reach the *other* test's server. So a
        // server only counts as started if its own process is still alive
        // after the port answers. Otherwise try again on another port.
        for _ in 0..8 {
            let port = TcpListener::bind("127.0.0.1:0").ok()?.local_addr().ok()?.port();
            let config = format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {host}\nAuthorizedKeysFile {auth}\n\
                 PidFile {pid}\nUsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n\
                 StrictModes no\nSubsystem sftp /usr/libexec/sftp-server\nLogLevel ERROR\n",
                host = p("host_key").display(),
                auth = p("authorized_keys").display(),
                pid = p("sshd.pid").display(),
            );
            fs::write(p("sshd_config"), config).ok()?;
            let mut child = Command::new("/usr/sbin/sshd")
                .args(["-D", "-e", "-f"])
                .arg(p("sshd_config"))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut answered = false;
            while Instant::now() < deadline && child.try_wait().ok().flatten().is_none() {
                if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    answered = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            // Give a losing sshd a moment to notice the port is taken.
            std::thread::sleep(Duration::from_millis(200));
            if answered && child.try_wait().ok().flatten().is_none() {
                let mut server = Server { dir, child, port };
                server.write_client_config("client_key");
                return Some(server);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        None
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// Remote paths are local paths here: the server is this machine.
    fn root(&self) -> String {
        fs::canonicalize(self.path("root")).unwrap().to_str().unwrap().to_string()
    }

    /// `Host t` with everything in the config, so the Connect panel can
    /// leave user and port empty (SF-01).
    fn write_client_config(&mut self, key: &str) {
        let user = std::env::var("USER").unwrap_or_default();
        let config = format!(
            "Host t\n  HostName 127.0.0.1\n  Port {}\n  User {user}\n  IdentityFile {}\n  IdentitiesOnly yes\n",
            self.port,
            self.path(key).display()
        );
        fs::write(self.path("ssh_config"), config).unwrap();
    }

    fn params(&self) -> SftpParams {
        SftpParams {
            host: "t".into(),
            remote_path: Some(self.root()),
            askpass_program: env!("CARGO_BIN_EXE_rustyfinder-askpass").into(),
            config_file: Some(self.path("ssh_config").to_str().unwrap().into()),
            known_hosts_file: Some(self.path("known_hosts").to_str().unwrap().into()),
            ..Default::default()
        }
    }

    /// Connect, accepting the unknown host key.
    fn connect(&self) -> SftpFs {
        connect(&self.params(), Arc::new(Answers::accepting())).expect("connect")
    }
}

/// Scripted user: records every question and answers by kind.
struct Answers {
    asked: Mutex<Vec<(String, PromptKind)>>,
    host_key: Option<String>,
    passphrase: Option<String>,
}

impl Answers {
    fn accepting() -> Self {
        Self { asked: Mutex::new(Vec::new()), host_key: Some("yes".into()), passphrase: None }
    }

    fn kinds(&self) -> Vec<PromptKind> {
        self.asked.lock().unwrap().iter().map(|(_, k)| *k).collect()
    }
}

impl Prompter for Answers {
    fn prompt(&self, text: &str, kind: PromptKind) -> Option<String> {
        self.asked.lock().unwrap().push((text.to_string(), kind));
        match kind {
            PromptKind::HostKey => self.host_key.clone(),
            PromptKind::Passphrase => self.passphrase.clone(),
            _ => None,
        }
    }
}

/// Runs `test` with a server, or reports that it was skipped.
fn with_server(test: impl FnOnce(&mut Server)) {
    match Server::start() {
        Some(mut server) => test(&mut server),
        None => eprintln!("SKIPPED: sshd could not be started on this machine"),
    }
}

// ------------------------------------------------------------ Connecting

#[test]
fn sf01_sf06_host_alias_from_config_and_home_as_first_folder() {
    with_server(|server| {
        // No user, no port, no remote path: all from the ssh config.
        let params = SftpParams { remote_path: None, ..server.params() };
        let fs = connect(&params, Arc::new(Answers::accepting())).expect("connect");
        let home = std::env::var("HOME").unwrap();
        assert_eq!(fs::canonicalize(fs.home()).unwrap(), fs::canonicalize(home).unwrap());
        assert_eq!(fs.label(), "t");
        assert!(fs.id().starts_with("sftp://"));
    });
}

#[test]
fn sf02_unknown_host_shows_a_fingerprint_and_is_remembered_once_accepted() {
    with_server(|server| {
        let user = Arc::new(Answers::accepting());
        let fs = connect(&server.params(), user.clone()).expect("connect");
        drop(fs);
        {
            let asked = user.asked.lock().unwrap();
            assert_eq!(asked.len(), 1);
            assert_eq!(asked[0].1, PromptKind::HostKey);
            assert!(asked[0].0.contains("SHA256:"), "no fingerprint in: {}", asked[0].0);
        }
        let known = fs::read_to_string(server.path("known_hosts")).unwrap();
        assert!(known.contains("ssh-ed25519"));

        // Second time: known host, nothing asked.
        let again = Arc::new(Answers::accepting());
        connect(&server.params(), again.clone()).expect("reconnect");
        assert!(again.kinds().is_empty());
    });
}

#[test]
fn refusing_an_unknown_host_key_connects_nothing() {
    with_server(|server| {
        let user = Arc::new(Answers { host_key: Some("no".into()), ..Answers::accepting() });
        let result = connect(&server.params(), user);
        assert!(matches!(result, Err(ConnectError::HostKeyRejected)), "{:?}", result.err());
        assert!(
            !server.path("known_hosts").exists() || fs::read_to_string(server.path("known_hosts")).unwrap().is_empty()
        );
    });
}

#[test]
fn sf03_a_changed_host_key_is_refused_with_a_warning() {
    with_server(|server| {
        drop(server.connect());
        // Swap the recorded key for a different one: the server now
        // presents a key that does not match the record.
        keygen(&server.path("other_key"), "");
        let other = fs::read_to_string(server.path("other_key.pub")).unwrap();
        let key = other.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        fs::write(server.path("known_hosts"), format!("[127.0.0.1]:{} {key}\n", server.port)).unwrap();

        let user = Arc::new(Answers::accepting());
        match connect(&server.params(), user.clone()) {
            Err(ConnectError::HostKeyChanged(text)) => assert!(text.contains("SHA256:")),
            other => panic!("expected HostKeyChanged, got {:?}", other.err()),
        }
        // ssh refuses outright: the user is never offered a "yes".
        assert!(user.kinds().is_empty());
    });
}

#[test]
fn sf04_a_passphrase_protected_key_asks_once() {
    with_server(|server| {
        server.write_client_config("locked_key");
        let user = Arc::new(Answers { passphrase: Some("open sesame".into()), ..Answers::accepting() });
        let fs = connect(&server.params(), user.clone()).expect("connect");
        assert!(!fs.list(&server.root()).unwrap().iter().any(|e| e.name == "nope"));
        let passphrases = user.kinds().iter().filter(|k| **k == PromptKind::Passphrase).count();
        assert_eq!(passphrases, 1);

        // A wrong passphrase fails as an authorization failure.
        let wrong = Arc::new(Answers { passphrase: Some("wrong".into()), ..Answers::accepting() });
        assert!(matches!(connect(&server.params(), wrong), Err(ConnectError::AuthFailed)));
    });
}

#[test]
fn connection_errors_are_explained() {
    with_server(|server| {
        // Nothing listens on this port.
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let refused = SftpParams {
            host: "127.0.0.1".into(),
            port: Some(port),
            config_file: Some("/dev/null".into()),
            ..server.params()
        };
        assert!(matches!(connect(&refused, Arc::new(Answers::accepting())), Err(ConnectError::Refused)));

        let unknown = SftpParams {
            host: "no-such-host.invalid".into(),
            config_file: Some("/dev/null".into()),
            ..server.params()
        };
        assert!(matches!(
            connect(&unknown, Arc::new(Answers::accepting())),
            Err(ConnectError::CannotResolve(h)) if h == "no-such-host.invalid"
        ));

        let missing_folder = SftpParams { remote_path: Some("/definitely/not/here".into()), ..server.params() };
        assert!(matches!(connect(&missing_folder, Arc::new(Answers::accepting())), Err(ConnectError::Other(_))));
    });
}

#[test]
fn sf11_disconnecting_leaves_no_ssh_process() {
    with_server(|server| {
        let fs = server.connect();
        let pid = fs.ssh_pid().expect("ssh is running") as i32;
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "ssh should be alive while connected");
        drop(fs);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && unsafe { libc::kill(pid, 0) } == 0 {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "ssh process {pid} is still running");
    });
}

// ------------------------------------------------------------ Operations

#[test]
fn listing_shows_kinds_sizes_dates_modes_and_link_targets() {
    with_server(|server| {
        let root = server.root();
        fs::create_dir(format!("{root}/dir")).unwrap();
        fs::write(format!("{root}/file.txt"), b"12345").unwrap();
        fs::write(format!("{root}/.hidden"), b"").unwrap();
        fs::set_permissions(format!("{root}/file.txt"), fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink("dir", format!("{root}/to_dir")).unwrap();
        std::os::unix::fs::symlink("nowhere", format!("{root}/broken")).unwrap();
        // A folder with a dot is still a folder (EC-04).
        fs::create_dir(format!("{root}/a.b")).unwrap();

        let fs = server.connect();
        let mut entries = fs.list(&root).unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec![".hidden", "a.b", "broken", "dir", "file.txt", "to_dir"]);
        let get = |n: &str| entries.iter().find(|e| e.name == n).unwrap();

        assert!(get(".hidden").is_hidden);
        assert_eq!(get(".hidden").size, 0);
        assert_eq!(get("a.b").kind, EntryKind::Directory);
        assert!(get("a.b").is_dir);
        assert_eq!(get("file.txt").kind, EntryKind::File);
        assert_eq!(get("file.txt").size, 5);
        assert_eq!(get("file.txt").mode, Some(0o640));
        let age = get("file.txt").modified_ms.unwrap();
        assert!(age > 1_600_000_000_000 && age % 1000 == 0, "whole seconds: {age}");
        assert_eq!(get("to_dir").kind, EntryKind::Symlink);
        assert!(get("to_dir").is_dir && !get("to_dir").is_broken_link);
        assert_eq!(get("to_dir").link_target.as_deref(), Some("dir"));
        assert!(get("broken").is_broken_link);

        assert_eq!(fs.stat(&format!("{root}/file.txt")).unwrap().size, 5);
        assert!(matches!(fs.stat(&format!("{root}/nope")), Err(VfsError::NotFound(_))));
        assert!(matches!(fs.list(&format!("{root}/nope")), Err(VfsError::NotFound(_))));
        assert!(fs.space(&root).is_some_and(|s| s.total > 0));
    });
}

#[test]
fn make_rename_delete_and_their_conflicts() {
    with_server(|server| {
        let root = server.root();
        let fs = server.connect();
        let p = |n: &str| format!("{root}/{n}");

        fs.mkdir(&p("d")).unwrap();
        assert!(matches!(fs.mkdir(&p("d")), Err(VfsError::AlreadyExists(_))));
        fs.create(&p("d/a")).unwrap().write_all(b"A").unwrap();
        assert!(matches!(fs.create(&p("d/a")), Err(VfsError::AlreadyExists(_))));
        fs.create(&p("d/b")).unwrap().write_all(b"B").unwrap();

        // Rename never overwrites unless asked to.
        assert!(matches!(fs.rename(&p("d/a"), &p("d/b"), false), Err(VfsError::AlreadyExists(_))));
        assert_eq!(std::fs::read(p("d/b")).unwrap(), b"B");
        fs.rename(&p("d/a"), &p("d/b"), true).unwrap();
        assert_eq!(std::fs::read(p("d/b")).unwrap(), b"A");
        // A case-only rename is an ordinary rename on a case-sensitive
        // server, and must work either way.
        fs.rename(&p("d/b"), &p("d/c"), false).unwrap();

        assert!(matches!(fs.remove_dir(&p("d")), Err(VfsError::NotEmpty(_))));
        fs.remove_file(&p("d/c")).unwrap();
        fs.remove_dir(&p("d")).unwrap();
        assert!(!Path::new(&p("d")).exists());
        assert!(matches!(fs.remove_file(&p("gone")), Err(VfsError::NotFound(_))));
    });
}

#[test]
fn sf13_mode_dates_and_symlinks_can_be_set() {
    with_server(|server| {
        let root = server.root();
        let fs = server.connect();
        let file = format!("{root}/f");
        fs.create(&file).unwrap().write_all(b"x").unwrap();

        fs.set_mode(&file, 0o600).unwrap();
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);

        fs.set_times(&file, Some(1_000_000_000_000), None).unwrap();
        assert_eq!(fs.stat(&file).unwrap().modified_ms, Some(1_000_000_000_000));

        let link = format!("{root}/link");
        fs.symlink("f", &link).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("f"));
        assert_eq!(fs.stat(&link).unwrap().link_target.as_deref(), Some("f"));
    });
}

#[test]
fn reading_and_writing_stream_including_empty_and_larger_files() {
    with_server(|server| {
        let root = server.root();
        let fs = server.connect();

        // Zero bytes up and down stays zero bytes.
        drop(fs.create(&format!("{root}/empty")).unwrap());
        assert_eq!(std::fs::metadata(format!("{root}/empty")).unwrap().len(), 0);
        let mut got = Vec::new();
        fs.open_read(&format!("{root}/empty")).unwrap().read_to_end(&mut got).unwrap();
        assert!(got.is_empty());

        // 3 MB with a recognisable pattern, written in odd-sized pieces.
        let data: Vec<u8> = (0..3 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
        let mut writer = fs.create(&format!("{root}/big")).unwrap();
        for chunk in data.chunks(70_001) {
            writer.write_all(chunk).unwrap();
        }
        writer.flush().unwrap();
        drop(writer);
        assert_eq!(std::fs::read(format!("{root}/big")).unwrap(), data);

        let mut back = Vec::new();
        fs.open_read(&format!("{root}/big")).unwrap().read_to_end(&mut back).unwrap();
        assert_eq!(back, data, "a non-empty download must never come back empty or altered");
    });
}

// -------------------------------------------- Through the transfer engine

/// Answers nothing: any prompt fails the test.
struct Quiet(Mutex<Progress>);

impl OpContext for Quiet {
    fn checkpoint(&self) -> bool {
        true
    }
    fn progress(&self, progress: &Progress) {
        *self.0.lock().unwrap() = progress.clone();
    }
    fn conflict(&self, info: &ConflictInfo) -> ConflictAnswer {
        panic!("unexpected conflict on {}", info.target_path);
    }
    fn error(&self, info: &ErrorInfo) -> ErrorChoice {
        panic!("unexpected error on {}: {}", info.path, info.message);
    }
}

#[test]
fn upload_and_download_whole_trees_through_the_engine() {
    with_server(|server| {
        let root = server.root();
        let remote: Arc<dyn Vfs> = Arc::new(server.connect());
        let local: Arc<dyn Vfs> = Arc::new(LocalFs);
        let scratch = tempfile::tempdir().unwrap();
        let here = scratch.path().to_str().unwrap().to_string();
        fs::create_dir_all(format!("{here}/site/css")).unwrap();
        fs::write(format!("{here}/site/index.html"), "<html>").unwrap();
        fs::write(format!("{here}/site/css/main.css"), "body{}").unwrap();
        fs::write(format!("{here}/site/empty"), "").unwrap();
        std::os::unix::fs::symlink("index.html", format!("{here}/site/home")).unwrap();
        LocalFs.set_times(&format!("{here}/site/index.html"), Some(1_000_000_000_000), None).unwrap();

        // Upload.
        let ctx = Quiet(Mutex::new(Progress::default()));
        let out = Engine::new(&ctx, Arc::clone(&local), Arc::clone(&remote), false, Policies::default())
            .transfer(TransferMode::Copy, &[format!("{here}/site")], &root, None)
            .unwrap();
        assert!(out.failures.is_empty(), "{:?}", out.failures);
        assert_eq!(fs::read_to_string(format!("{root}/site/css/main.css")).unwrap(), "body{}");
        assert_eq!(fs::metadata(format!("{root}/site/empty")).unwrap().len(), 0);
        assert_eq!(fs::read_link(format!("{root}/site/home")).unwrap(), Path::new("index.html"));
        // The modification time is set on the server after the upload.
        assert_eq!(remote.stat(&format!("{root}/site/index.html")).unwrap().modified_ms, Some(1_000_000_000_000));
        let p = ctx.0.lock().unwrap().clone();
        assert_eq!((p.items_done, p.items_total), (6, 6));

        // Download into a fresh folder.
        fs::create_dir(format!("{here}/back")).unwrap();
        let ctx = Quiet(Mutex::new(Progress::default()));
        let out = Engine::new(&ctx, Arc::clone(&remote), Arc::clone(&local), false, Policies::default())
            .transfer(TransferMode::Copy, &[format!("{root}/site")], &format!("{here}/back"), None)
            .unwrap();
        assert!(out.failures.is_empty(), "{:?}", out.failures);
        assert_eq!(fs::read_to_string(format!("{here}/back/site/index.html")).unwrap(), "<html>");
        assert_eq!(LocalFs.stat(&format!("{here}/back/site/index.html")).unwrap().modified_ms, Some(1_000_000_000_000));
        assert_eq!(fs::read_link(format!("{here}/back/site/home")).unwrap(), Path::new("index.html"));
    });
}

#[test]
fn replace_upload_goes_through_a_temporary_name() {
    with_server(|server| {
        let root = server.root();
        fs::write(format!("{root}/index.html"), "old").unwrap();
        let remote: Arc<dyn Vfs> = Arc::new(server.connect());
        let scratch = tempfile::tempdir().unwrap();
        let source = scratch.path().join("index.html");
        fs::write(&source, "new").unwrap();

        struct Replace;
        impl OpContext for Replace {
            fn checkpoint(&self) -> bool {
                true
            }
            fn progress(&self, _: &Progress) {}
            fn conflict(&self, _: &ConflictInfo) -> ConflictAnswer {
                ConflictAnswer { choice: ConflictChoice::Replace, apply_to_all: false }
            }
            fn error(&self, info: &ErrorInfo) -> ErrorChoice {
                panic!("unexpected error: {}", info.message);
            }
        }
        let out = Engine::new(&Replace, Arc::new(LocalFs), remote, false, Policies::default())
            .transfer(TransferMode::Copy, &[source.to_str().unwrap().to_string()], &root, None)
            .unwrap();
        assert!(out.failures.is_empty(), "{:?}", out.failures);
        assert_eq!(fs::read_to_string(format!("{root}/index.html")).unwrap(), "new");
        // No staging file is left behind on the server.
        let names: Vec<String> =
            fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["index.html"]);
    });
}

#[test]
fn moving_within_the_server_is_a_rename_and_deleting_a_tree_works() {
    with_server(|server| {
        let root = server.root();
        fs::create_dir_all(format!("{root}/a/sub")).unwrap();
        fs::write(format!("{root}/a/sub/f"), "x").unwrap();
        fs::create_dir(format!("{root}/b")).unwrap();
        let remote: Arc<dyn Vfs> = Arc::new(server.connect());

        let ctx = Quiet(Mutex::new(Progress::default()));
        let out = Engine::new(&ctx, Arc::clone(&remote), Arc::clone(&remote), true, Policies::default())
            .transfer(TransferMode::Move, &[format!("{root}/a")], &format!("{root}/b"), None)
            .unwrap();
        assert!(out.failures.is_empty(), "{:?}", out.failures);
        assert_eq!(fs::read_to_string(format!("{root}/b/a/sub/f")).unwrap(), "x");
        assert!(!Path::new(&format!("{root}/a")).exists());

        let ctx = Quiet(Mutex::new(Progress::default()));
        let out = Engine::new(&ctx, Arc::clone(&remote), Arc::clone(&remote), true, Policies::default())
            .delete(&[format!("{root}/b")]);
        assert!(out.failures.is_empty(), "{:?}", out.failures);
        assert!(!Path::new(&format!("{root}/b")).exists());
    });
}

// ----------------------------------------- Host keys, reconnects, resume

#[test]
fn sf03_connecting_anyway_accepts_only_the_new_key_the_user_saw() {
    with_server(|server| {
        drop(server.connect());
        keygen(&server.path("other_key"), "");
        let other = fs::read_to_string(server.path("other_key.pub")).unwrap();
        let key = other.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        fs::write(server.path("known_hosts"), format!("[127.0.0.1]:{} {key}\n", server.port)).unwrap();
        let text = match connect(&server.params(), Arc::new(Answers::accepting())) {
            Err(ConnectError::HostKeyChanged(text)) => text,
            other => panic!("expected HostKeyChanged, got {:?}", other.err()),
        };
        let fingerprint = rustyfinder_remote::sftp::changed_host_key_fingerprint(&text).expect("fingerprint");

        // A different fingerprint than the one shown: the user is asked,
        // and saying no connects nothing.
        let wary = Arc::new(Answers { host_key: Some("no".into()), ..Answers::accepting() });
        let wrong = SftpParams { accept_changed_host_key: Some("SHA256:not-it".into()), ..server.params() };
        assert!(matches!(connect(&wrong, wary.clone()), Err(ConnectError::HostKeyRejected)));
        assert_eq!(wary.kinds(), vec![PromptKind::HostKey]);

        // The fingerprint that was shown: connects without another question.
        let user = Arc::new(Answers { host_key: Some("no".into()), ..Answers::accepting() });
        let anyway = SftpParams { accept_changed_host_key: Some(fingerprint), ..server.params() };
        let fs = connect(&anyway, user.clone()).expect("override");
        assert!(user.kinds().is_empty());
        drop(fs);
        // The new key is on record now; the next connect is quiet.
        let again = Arc::new(Answers { host_key: Some("no".into()), ..Answers::accepting() });
        connect(&server.params(), again.clone()).expect("plain connect");
        assert!(again.kinds().is_empty());
    });
}

#[test]
fn a_dead_session_is_opened_again_once_on_the_next_request() {
    with_server(|server| {
        let root = server.root();
        fs::write(format!("{root}/here.txt"), "x").unwrap();
        let fs = server.connect();
        let pid = fs.ssh_pid().unwrap() as i32;
        // What sleep or a network change does to the connection.
        unsafe { libc::kill(pid, libc::SIGKILL) };
        std::thread::sleep(Duration::from_millis(200));
        let names: Vec<String> = fs.list(&root).expect("reconnected").into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["here.txt"]);
        assert_eq!(fs.reconnects(), 1);
        assert_ne!(fs.ssh_pid().unwrap() as i32, pid);
        // And it keeps working without reconnecting again.
        fs.stat(&format!("{root}/here.txt")).unwrap();
        assert_eq!(fs.reconnects(), 1);
    });
}

#[test]
fn listings_carry_owner_and_group_names() {
    with_server(|server| {
        let root = server.root();
        fs::write(format!("{root}/mine"), "x").unwrap();
        let fs = server.connect();
        let entry = fs.list(&root).unwrap().into_iter().find(|e| e.name == "mine").unwrap();
        let me = std::env::var("USER").unwrap();
        assert_eq!(entry.owner_name().as_deref(), Some(me.as_str()));
        assert!(entry.group_name().is_some_and(|g| !g.is_empty()));
    });
}

#[test]
fn sf10_a_replacing_upload_never_leaves_the_target_missing() {
    with_server(|server| {
        let root = server.root();
        fs::write(format!("{root}/index.html"), "v0").unwrap();
        let remote: Arc<dyn Vfs> = Arc::new(server.connect());
        let scratch = tempfile::tempdir().unwrap();
        let source = scratch.path().join("index.html").to_str().unwrap().to_string();

        // Watch the folder the whole time the uploads run.
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = {
            let (stop, root) = (Arc::clone(&stop), root.clone());
            std::thread::spawn(move || {
                let mut missing = 0;
                while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                    if !Path::new(&format!("{root}/index.html")).exists() {
                        missing += 1;
                    }
                }
                missing
            })
        };
        let policies = Policies { file: rustyfinder_core::ConflictPolicy::Replace, ..Policies::default() };
        for round in 1..=20 {
            fs::write(&source, format!("v{round}")).unwrap();
            let ctx = Quiet(Mutex::new(Progress::default()));
            let out = Engine::new(&ctx, Arc::new(LocalFs), Arc::clone(&remote), false, policies)
                .transfer(TransferMode::Copy, std::slice::from_ref(&source), &root, None)
                .unwrap();
            assert!(out.failures.is_empty(), "{:?}", out.failures);
        }
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(watcher.join().unwrap(), 0, "index.html was missing at some point");
        assert_eq!(fs::read_to_string(format!("{root}/index.html")).unwrap(), "v20");
    });
}

/// Stops once this many bytes went through; answers Resume when asked.
struct StopAt(u64, Mutex<Progress>, Mutex<Vec<ConflictInfo>>);

impl OpContext for StopAt {
    fn checkpoint(&self) -> bool {
        self.1.lock().unwrap().bytes_done < self.0
    }
    fn progress(&self, progress: &Progress) {
        *self.1.lock().unwrap() = progress.clone();
    }
    fn conflict(&self, info: &ConflictInfo) -> ConflictAnswer {
        self.2.lock().unwrap().push(info.clone());
        ConflictAnswer { choice: ConflictChoice::Resume, apply_to_all: false }
    }
    fn error(&self, info: &ErrorInfo) -> ErrorChoice {
        panic!("unexpected error on {}: {}", info.path, info.message);
    }
}

#[test]
fn sf08_a_stopped_download_and_upload_resume_at_their_offsets() {
    use rustyfinder_core::ops::Partials;
    with_server(|server| {
        let root = server.root();
        let data: Vec<u8> = (0..6 * 1024 * 1024u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        fs::write(format!("{root}/big.iso"), &data).unwrap();
        let remote: Arc<dyn Vfs> = Arc::new(server.connect());
        let local: Arc<dyn Vfs> = Arc::new(LocalFs);
        let partials = Arc::new(Partials::in_memory());
        let scratch = tempfile::tempdir().unwrap();
        let here = scratch.path().to_str().unwrap().to_string();
        fn with<'a>(ctx: &'a StopAt, from: &Arc<dyn Vfs>, to: &Arc<dyn Vfs>, partials: &Arc<Partials>) -> Engine<'a> {
            Engine::new(ctx, Arc::clone(from), Arc::clone(to), false, Policies::default())
                .with_partials(Some(Arc::clone(partials)))
        }
        let engine = |ctx, from, to| with(ctx, from, to, &partials);

        // Download, stopped at about 30 %, then resumed.
        let stop = StopAt(2 * 1024 * 1024, Mutex::default(), Mutex::default());
        let out =
            engine(&stop, &remote, &local).transfer(TransferMode::Copy, &[format!("{root}/big.iso")], &here, None);
        assert!(out.unwrap().stopped);
        let kept = fs::metadata(format!("{here}/big.iso")).unwrap().len();
        assert!(kept > 0 && kept < data.len() as u64);
        let resume = StopAt(u64::MAX, Mutex::default(), Mutex::default());
        let out = engine(&resume, &remote, &local)
            .transfer(TransferMode::Copy, &[format!("{root}/big.iso")], &here, None)
            .unwrap();
        assert!(out.failures.is_empty(), "{:?}", out.failures);
        assert!(resume.2.lock().unwrap()[0].resumable);
        assert_eq!(fs::read(format!("{here}/big.iso")).unwrap(), data, "byte for byte");

        // Upload, the same way round.
        fs::create_dir(format!("{root}/up")).unwrap();
        let up = format!("{root}/up");
        let stop = StopAt(2 * 1024 * 1024, Mutex::default(), Mutex::default());
        let out = engine(&stop, &local, &remote).transfer(TransferMode::Copy, &[format!("{here}/big.iso")], &up, None);
        assert!(out.unwrap().stopped);
        assert!(fs::metadata(format!("{up}/big.iso")).unwrap().len() < data.len() as u64);
        let resume = StopAt(u64::MAX, Mutex::default(), Mutex::default());
        engine(&resume, &local, &remote).transfer(TransferMode::Copy, &[format!("{here}/big.iso")], &up, None).unwrap();
        assert!(resume.2.lock().unwrap()[0].resumable);
        assert_eq!(fs::read(format!("{up}/big.iso")).unwrap(), data);
    });
}

#[test]
fn copy_url_names_host_and_path_without_a_password() {
    with_server(|server| {
        let fs = server.connect();
        let url = fs.url_for("/var/www/my file.html").unwrap();
        assert_eq!(url, "sftp://t/var/www/my%20file.html");
    });
}

#[test]
fn sf09_names_that_are_not_utf8_survive_the_protocol_layer() {
    // APFS refuses such names, so no file here can carry one; check the
    // wire format instead: the bytes go out exactly as they came in.
    use russh_sftp::protocol::{File as WireFile, FileAttributes};
    let name = russh_sftp::escape_bytes(&[0xff, b'A']);
    assert_eq!(name.chars().count(), 2, "one character per byte, the A kept as it is");
    let file = WireFile { filename: name.clone(), longname: String::new(), attrs: FileAttributes::empty() };
    let bytes = russh_sftp::ser::to_bytes(&file).unwrap();
    assert_eq!(&bytes[..6], &[0, 0, 0, 2, 0xff, b'A'], "length 2, then the raw bytes");
    let back: WireFile = russh_sftp::de::from_bytes(&mut bytes.clone()).unwrap();
    assert_eq!(back.filename, name);
}
