//! Transfer behaviour around failures: automatic retries, "retry after"
//! answers, bandwidth limits, partial files and Resume, folder Replace
//! confirmations and NFC name matching on servers. Test names carry spec
//! IDs (FO = file-ops, CF = conflicts).

use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::engine::*;
use super::partials::Partials;
use super::throttle::RateLimiter;
use crate::entry::Entry;
use crate::error::{VfsError, VfsResult};
use crate::local::LocalFs;
use crate::vfs::Vfs;

#[derive(Default)]
struct Script {
    conflicts: Mutex<VecDeque<ConflictChoice>>,
    errors: Mutex<VecDeque<ErrorChoice>>,
    asked: Mutex<Vec<ConflictInfo>>,
    errors_asked: Mutex<Vec<ErrorInfo>>,
    /// Stop once this many bytes have been reported (negative: never).
    stop_at_bytes: AtomicI64,
    last: Mutex<Progress>,
}

impl Script {
    fn new() -> Self {
        let s = Self::default();
        s.stop_at_bytes.store(-1, Ordering::SeqCst);
        s
    }

    fn answering(choices: &[ConflictChoice]) -> Self {
        let s = Self::new();
        s.conflicts.lock().unwrap().extend(choices.iter().copied());
        s
    }
}

impl OpContext for Script {
    fn checkpoint(&self) -> bool {
        let stop = self.stop_at_bytes.load(Ordering::SeqCst);
        stop < 0 || (self.last.lock().unwrap().bytes_done as i64) < stop
    }
    fn progress(&self, progress: &Progress) {
        *self.last.lock().unwrap() = progress.clone();
    }
    fn conflict(&self, info: &ConflictInfo) -> ConflictAnswer {
        self.asked.lock().unwrap().push(info.clone());
        let choice = self.conflicts.lock().unwrap().pop_front().expect("unexpected conflict prompt");
        ConflictAnswer { choice, apply_to_all: false }
    }
    fn error(&self, info: &ErrorInfo) -> ErrorChoice {
        self.errors_asked.lock().unwrap().push(info.clone());
        self.errors.lock().unwrap().pop_front().expect("unexpected error prompt")
    }
}

/// The local disk dressed up as a server whose failures are scripted.
#[derive(Default)]
struct Server {
    /// `open_read` fails with a network-style error this many times.
    fail_opens: AtomicU32,
    /// `open_read` answers "retry after 0 s" this many times.
    throttle_opens: AtomicU32,
    /// The next download breaks after this many bytes (0: never).
    break_after: AtomicU64,
    /// Bytes handed out by readers so far.
    bytes_read: Arc<AtomicU64>,
    /// Compare names after NFC normalisation, like a real server.
    nfc: bool,
}

struct CountingReader {
    inner: Box<dyn Read + Send>,
    count: Arc<AtomicU64>,
    /// Bytes left before the connection "drops".
    left: Option<u64>,
}

impl Read for CountingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let want = match self.left {
            Some(0) => return Err(std::io::Error::other("connection reset by peer")),
            Some(left) => buf.len().min(left as usize),
            None => buf.len(),
        };
        let n = self.inner.read(&mut buf[..want])?;
        if let Some(left) = self.left.as_mut() {
            *left -= n as u64;
        }
        self.count.fetch_add(n as u64, Ordering::SeqCst);
        Ok(n)
    }
}

impl Server {
    fn reader(&self, inner: Box<dyn Read + Send>) -> Box<dyn Read + Send> {
        let cut = self.break_after.swap(0, Ordering::SeqCst);
        Box::new(CountingReader { inner, count: Arc::clone(&self.bytes_read), left: (cut > 0).then_some(cut) })
    }
}

impl Vfs for Server {
    fn id(&self) -> String {
        "test://server".into()
    }
    fn list(&self, path: &str) -> VfsResult<Vec<Entry>> {
        LocalFs.list(path)
    }
    fn stat(&self, path: &str) -> VfsResult<Entry> {
        if self.nfc {
            // A server matches names byte for byte (APFS would not).
            let name = crate::paths::file_name(path);
            let dir = crate::paths::parent(path).unwrap_or_else(|| "/".into());
            if !LocalFs.list(&dir)?.iter().any(|e| e.name == name) {
                return Err(VfsError::NotFound(path.into()));
            }
        }
        LocalFs.stat(path)
    }
    fn mkdir(&self, path: &str) -> VfsResult<()> {
        LocalFs.mkdir(path)
    }
    fn rename(&self, from: &str, to: &str, replace: bool) -> VfsResult<()> {
        LocalFs.rename(from, to, replace)
    }
    fn remove_file(&self, path: &str) -> VfsResult<()> {
        LocalFs.remove_file(path)
    }
    fn remove_dir(&self, path: &str) -> VfsResult<()> {
        LocalFs.remove_dir(path)
    }
    fn open_read(&self, path: &str) -> VfsResult<Box<dyn Read + Send>> {
        if self.fail_opens.load(Ordering::SeqCst) > 0 {
            self.fail_opens.fetch_sub(1, Ordering::SeqCst);
            return Err(VfsError::Io { path: path.into(), message: "The server did not answer in time.".into() });
        }
        if self.throttle_opens.load(Ordering::SeqCst) > 0 {
            self.throttle_opens.fetch_sub(1, Ordering::SeqCst);
            return Err(VfsError::Throttled { path: path.into(), message: "Slow down".into(), retry_after_secs: 0 });
        }
        Ok(self.reader(LocalFs.open_read(path)?))
    }
    fn create(&self, path: &str) -> VfsResult<Box<dyn Write + Send>> {
        LocalFs.create(path)
    }
    fn set_times(&self, path: &str, m: Option<i64>, c: Option<i64>) -> VfsResult<()> {
        LocalFs.set_times(path, m, c)
    }
    fn same_volume(&self, _a: &str, _b: &str) -> bool {
        false
    }
    fn resume_support(&self) -> (bool, bool) {
        (true, true)
    }
    fn open_read_at(&self, path: &str, offset: u64) -> VfsResult<Box<dyn Read + Send>> {
        Ok(self.reader(LocalFs.open_read_at(path, offset)?))
    }
    fn open_write_at(&self, path: &str, offset: u64) -> VfsResult<Box<dyn Write + Send>> {
        LocalFs.open_write_at(path, offset)
    }
    fn nfc_names(&self) -> bool {
        self.nfc
    }
}

struct Fx {
    tmp: tempfile::TempDir,
    server: Arc<Server>,
    partials: Arc<Partials>,
}

impl Fx {
    fn new() -> Self {
        Self::with(Server::default())
    }

    fn with(server: Server) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("remote")).unwrap();
        fs::create_dir(tmp.path().join("local")).unwrap();
        Self { tmp, server: Arc::new(server), partials: Arc::new(Partials::in_memory()) }
    }

    fn p(&self, rel: &str) -> String {
        self.tmp.path().join(rel).to_str().unwrap().to_string()
    }

    fn write(&self, rel: &str, bytes: &[u8]) {
        let path = self.tmp.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn read(&self, rel: &str) -> Vec<u8> {
        fs::read(self.tmp.path().join(rel)).unwrap()
    }

    /// Download from the "server" to the local disk.
    fn download<'a>(&self, ctx: &'a Script, policies: Policies) -> Engine<'a> {
        let server: Arc<dyn Vfs> = self.server.clone();
        Engine::new(ctx, server, Arc::new(LocalFs), false, policies).with_partials(Some(Arc::clone(&self.partials)))
    }

    /// Upload from the local disk to the "server".
    fn upload<'a>(&self, ctx: &'a Script, policies: Policies) -> Engine<'a> {
        let server: Arc<dyn Vfs> = self.server.clone();
        Engine::new(ctx, Arc::new(LocalFs), server, false, policies).with_partials(Some(Arc::clone(&self.partials)))
    }
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 % 251) as u8).collect()
}

fn quick_retries(retries: u32) -> Policies {
    Policies { retries, retry_delay: Duration::from_millis(1), ..Default::default() }
}

// ------------------------------------------------------------- Retries

#[test]
fn fo15_auto_retry_rides_out_two_failures_without_asking() {
    let f = Fx::new();
    f.write("remote/a.bin", b"payload");
    f.server.fail_opens.store(2, Ordering::SeqCst);
    let ctx = Script::new(); // any error prompt would panic
    let out =
        f.download(&ctx, quick_retries(3)).transfer(TransferMode::Copy, &[f.p("remote/a.bin")], &f.p("local"), None);
    let out = out.unwrap();
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert_eq!(f.read("local/a.bin"), b"payload");
    assert!(out.notes.iter().any(|n| n.starts_with("Retry 2 of 3")), "{:?}", out.notes);
}

#[test]
fn when_the_retries_run_out_the_error_policy_applies() {
    let f = Fx::new();
    f.write("remote/a.bin", b"payload");
    f.server.fail_opens.store(5, Ordering::SeqCst);
    let ctx = Script::new();
    ctx.errors.lock().unwrap().push_back(ErrorChoice::Skip);
    let out =
        f.download(&ctx, quick_retries(2)).transfer(TransferMode::Copy, &[f.p("remote/a.bin")], &f.p("local"), None);
    assert_eq!(out.unwrap().failures.len(), 1);
    assert_eq!(ctx.errors_asked.lock().unwrap().len(), 1, "asked once, after two automatic retries");
    // Two automatic tries plus the first: three opens failed.
    assert_eq!(f.server.fail_opens.load(Ordering::SeqCst), 2);
}

#[test]
fn back_off_doubles_and_a_stop_ends_the_wait() {
    let f = Fx::new();
    f.write("remote/a.bin", b"x");
    f.server.fail_opens.store(1, Ordering::SeqCst);
    let ctx = Script::new();
    let policies = Policies { retries: 1, retry_delay: Duration::from_millis(300), ..Default::default() };
    let started = Instant::now();
    f.download(&ctx, policies).transfer(TransferMode::Copy, &[f.p("remote/a.bin")], &f.p("local"), None).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(300), "the back-off was waited out");

    // Stopped while waiting: the wait ends at once.
    f.server.fail_opens.store(1, Ordering::SeqCst);
    fs::remove_file(f.p("local/a.bin")).unwrap();
    let ctx = Script::new();
    ctx.stop_at_bytes.store(0, Ordering::SeqCst);
    let policies = Policies { retries: 1, retry_delay: Duration::from_secs(20), ..Default::default() };
    let started = Instant::now();
    let out = f.download(&ctx, policies).transfer(TransferMode::Copy, &[f.p("remote/a.bin")], &f.p("local"), None);
    assert!(out.unwrap().stopped);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn retry_after_answers_are_honoured_without_counting_as_retries() {
    let f = Fx::new();
    f.write("remote/a.bin", b"payload");
    f.server.throttle_opens.store(3, Ordering::SeqCst);
    let ctx = Script::new(); // automatic retries are off, yet nobody is asked
    let out = f
        .download(&ctx, Policies::default())
        .transfer(TransferMode::Copy, &[f.p("remote/a.bin")], &f.p("local"), None)
        .unwrap();
    assert!(out.failures.is_empty());
    assert_eq!(f.read("local/a.bin"), b"payload");
    assert_eq!(out.notes.iter().filter(|n| n.starts_with("Server busy")).count(), 3);
}

// ------------------------------------------------------------ Bandwidth

#[test]
fn fo14_a_limited_transfer_keeps_to_the_limit() {
    let f = Fx::new();
    f.write("local/big.bin", &pattern(300_000));
    let ctx = Script::new();
    let limiter = Arc::new(RateLimiter::new(200_000));
    let started = Instant::now();
    let out = f
        .upload(&ctx, Policies::default())
        .with_limiter(Some(limiter))
        .transfer(TransferMode::Copy, &[f.p("local/big.bin")], &f.p("remote"), None)
        .unwrap();
    assert!(out.failures.is_empty());
    // 300 KB at 200 KB/s, less the small burst allowance: over a second.
    assert!(started.elapsed() >= Duration::from_millis(1100), "{:?}", started.elapsed());
    assert_eq!(f.read("remote/big.bin"), pattern(300_000));
}

// ------------------------------------------------------- Partial files

#[test]
fn cf11_a_stopped_download_is_resumed_with_only_the_rest_transferred() {
    let f = Fx::new();
    let content = pattern(3 * 1024 * 1024 + 123);
    f.write("remote/big.iso", &content);

    // Stopped after the first megabyte: the partial file stays.
    let ctx = Script::new();
    ctx.stop_at_bytes.store(1024 * 1024, Ordering::SeqCst);
    let out = f.download(&ctx, Policies::default()).transfer(
        TransferMode::Copy,
        &[f.p("remote/big.iso")],
        &f.p("local"),
        None,
    );
    assert!(out.unwrap().stopped);
    let kept = f.read("local/big.iso").len();
    assert!(kept > 0 && kept < content.len(), "a partial file is left: {kept}");

    // Started again: the conflict offers Resume.
    let before = f.server.bytes_read.load(Ordering::SeqCst);
    let ctx = Script::answering(&[ConflictChoice::Resume]);
    let out = f
        .download(&ctx, Policies::default())
        .transfer(TransferMode::Copy, &[f.p("remote/big.iso")], &f.p("local"), None)
        .unwrap();
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert!(ctx.asked.lock().unwrap()[0].resumable);
    assert_eq!(f.read("local/big.iso"), content, "byte for byte");
    let transferred = f.server.bytes_read.load(Ordering::SeqCst) - before;
    assert_eq!(transferred, (content.len() - kept) as u64, "only the rest was read");
    assert!(f.partials.find("local", &f.p("local/big.iso")).is_none(), "complete now");
}

#[test]
fn resume_policy_resume_needs_no_prompt_and_works_for_uploads() {
    let f = Fx::new();
    let content = pattern(2 * 1024 * 1024 + 7);
    f.write("local/up.bin", &content);
    let ctx = Script::new();
    ctx.stop_at_bytes.store(1024 * 1024, Ordering::SeqCst);
    let out =
        f.upload(&ctx, Policies::default()).transfer(TransferMode::Copy, &[f.p("local/up.bin")], &f.p("remote"), None);
    assert!(out.unwrap().stopped);

    let ctx = Script::new();
    let policies = Policies { resume: ResumePolicy::Resume, ..Default::default() };
    let out = f.upload(&ctx, policies).transfer(TransferMode::Copy, &[f.p("local/up.bin")], &f.p("remote"), None);
    assert!(out.unwrap().failures.is_empty());
    assert_eq!(f.read("remote/up.bin"), content);
    assert!(ctx.asked.lock().unwrap().is_empty());
}

#[test]
fn a_smaller_file_nobody_recorded_is_an_ordinary_conflict() {
    let f = Fx::new();
    f.write("remote/a.txt", b"the whole content");
    f.write("local/a.txt", b"the");
    let ctx = Script::answering(&[ConflictChoice::Skip]);
    f.download(&ctx, Policies { resume: ResumePolicy::Resume, ..Default::default() })
        .transfer(TransferMode::Copy, &[f.p("remote/a.txt")], &f.p("local"), None)
        .unwrap();
    assert!(!ctx.asked.lock().unwrap()[0].resumable);
    assert_eq!(f.read("local/a.txt"), b"the");
}

#[test]
fn a_changed_source_is_not_resumed() {
    let f = Fx::new();
    f.write("remote/a.bin", &pattern(2 * 1024 * 1024));
    let ctx = Script::new();
    ctx.stop_at_bytes.store(1024 * 1024, Ordering::SeqCst);
    f.download(&ctx, Policies::default())
        .transfer(TransferMode::Copy, &[f.p("remote/a.bin")], &f.p("local"), None)
        .unwrap();
    // The source changed meanwhile: appending its tail would mix versions.
    f.write("remote/a.bin", &pattern(2 * 1024 * 1024 + 1));
    let ctx = Script::answering(&[ConflictChoice::Replace]);
    f.download(&ctx, Policies { resume: ResumePolicy::Resume, ..Default::default() })
        .transfer(TransferMode::Copy, &[f.p("remote/a.bin")], &f.p("local"), None)
        .unwrap();
    assert!(!ctx.asked.lock().unwrap()[0].resumable);
    assert_eq!(f.read("local/a.bin"), pattern(2 * 1024 * 1024 + 1));
}

#[test]
fn a_retry_after_a_dropped_connection_continues_where_it_broke() {
    let f = Fx::new();
    let content = pattern(3 * 1024 * 1024);
    f.write("remote/a.bin", &content);
    f.server.break_after.store(1_500_000, Ordering::SeqCst);
    let ctx = Script::new();
    let out =
        f.download(&ctx, quick_retries(1)).transfer(TransferMode::Copy, &[f.p("remote/a.bin")], &f.p("local"), None);
    assert!(out.unwrap().failures.is_empty());
    assert_eq!(f.read("local/a.bin"), content);
    // 1.5 MB before the break, then only the remaining 1.5 MB.
    assert_eq!(f.server.bytes_read.load(Ordering::SeqCst), content.len() as u64);
}

#[test]
fn without_a_partials_record_a_stopped_copy_leaves_nothing() {
    let f = Fx::new();
    f.write("remote/a.bin", &pattern(2 * 1024 * 1024));
    let ctx = Script::new();
    ctx.stop_at_bytes.store(1024 * 1024, Ordering::SeqCst);
    let server: Arc<dyn Vfs> = f.server.clone();
    let out = Engine::new(&ctx, server, Arc::new(LocalFs), false, Policies::default()).transfer(
        TransferMode::Copy,
        &[f.p("remote/a.bin")],
        &f.p("local"),
        None,
    );
    assert!(out.unwrap().stopped);
    assert!(fs::read_dir(f.p("local")).unwrap().next().is_none());
}

// ----------------------------------------------------- Folder Replace

#[test]
fn replacing_a_folder_that_holds_extra_items_is_confirmed_even_under_a_policy() {
    let f = Fx::new();
    f.write("local/d/same.txt", b"new");
    f.write("remote/d/same.txt", b"old");
    f.write("remote/d/only-there.txt", b"keep me?");
    let policies = Policies { folder: ConflictPolicy::Replace, ..Default::default() };

    // The prompt names what would be lost; Skip keeps it.
    let ctx = Script::answering(&[ConflictChoice::Skip]);
    f.upload(&ctx, policies).transfer(TransferMode::Copy, &[f.p("local/d")], &f.p("remote"), None).unwrap();
    assert_eq!(ctx.asked.lock().unwrap()[0].target_extras, vec!["only-there.txt"]);
    assert_eq!(f.read("remote/d/only-there.txt"), b"keep me?");

    // Confirmed: replaced.
    let ctx = Script::answering(&[ConflictChoice::Replace]);
    f.upload(&ctx, policies).transfer(TransferMode::Copy, &[f.p("local/d")], &f.p("remote"), None).unwrap();
    assert!(!f.tmp.path().join("remote/d/only-there.txt").exists());
    assert_eq!(f.read("remote/d/same.txt"), b"new");

    // Nothing extra in the target: the policy acts without asking.
    let ctx = Script::new();
    f.upload(&ctx, policies).transfer(TransferMode::Copy, &[f.p("local/d")], &f.p("remote"), None).unwrap();
    assert!(ctx.asked.lock().unwrap().is_empty());
}

// --------------------------------------------------------- Unicode names

#[test]
fn names_on_a_server_are_compared_after_nfc_normalisation() {
    let f = Fx::with(Server { nfc: true, ..Default::default() });
    // The server keeps the precomposed name; this Mac sends decomposed.
    let nfc = "caf\u{e9}.txt";
    let nfd = "cafe\u{301}.txt";
    let local = tempfile::tempdir().unwrap();
    fs::write(local.path().join(nfd), b"new").unwrap();
    // A folder outside APFS's own normalisation: plain bytes on the "server".
    f.write(&format!("remote/{nfc}"), b"old");
    let source = local.path().join(nfd).to_str().unwrap().to_string();
    let ctx = Script::answering(&[ConflictChoice::Skip]);
    f.upload(&ctx, Policies::default()).transfer(TransferMode::Copy, &[source], &f.p("remote"), None).unwrap();
    assert_eq!(ctx.asked.lock().unwrap().len(), 1, "the two spellings are one name");
}
