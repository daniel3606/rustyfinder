//! Activities: every file operation runs as one, with a title, state,
//! progress, and Pause / Resume / Stop.
//!
//! Copies wait in a transfer queue (at most N run at once). Deletes,
//! renames, new items and same-volume moves start at once. Finished
//! operations are logged, and those that can be reversed go on a per-window
//! undo stack.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::error::VfsError;
use crate::ops::engine::{
    ConflictAnswer, ConflictChoice, ConflictInfo, Effect, Engine, ErrorChoice, ErrorInfo, OpContext, Outcome, Policies,
    Progress, TransferMode,
};
use crate::ops::partials::Partials;
use crate::ops::privileged::Privileged;
use crate::ops::simple;
use crate::ops::throttle::RateLimiter;
use crate::paths;
use crate::registry::{ConnId, VfsRegistry, LOCAL};
use crate::store::Store;
use crate::vfs::Vfs;

pub type ActivityId = u64;

/// Most log entries kept across launches.
pub const MAX_LOG_ENTRIES: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityState {
    Queued,
    Preparing,
    Running,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ActivityResult {
    Success,
    /// Finished, but some items failed or were skipped.
    Partial,
    Failed,
    Stopped,
}

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Transfer {
        mode: TransferMode,
        sources: Vec<String>,
        dest_dir: String,
        /// New name for a single source ("Copy to as…").
        rename_to: Option<String>,
    },
    Duplicate {
        paths: Vec<String>,
    },
    /// Permanent delete. Cannot be undone.
    Delete {
        paths: Vec<String>,
    },
    Trash {
        paths: Vec<String>,
    },
    Rename {
        path: String,
        new_name: String,
    },
    NewFolder {
        dir: String,
    },
    NewFile {
        dir: String,
    },
    /// Make a folder in `dir` and move `paths` into it.
    NewFolderWithSelection {
        dir: String,
        paths: Vec<String>,
    },
    /// Symlinks to `paths`, created in `dest_dir`.
    Symlink {
        paths: Vec<String>,
        dest_dir: String,
    },
    /// Finder aliases to `paths` (this Mac only), created in `dest_dir`.
    Alias {
        paths: Vec<String>,
        dest_dir: String,
    },
    /// Set permission bits; `recursive` applies them to enclosed items too.
    SetMode {
        paths: Vec<String>,
        mode: u32,
        recursive: bool,
    },
    /// Copy one file to an exact path, replacing what is there without
    /// asking. Used to send an edited file back to its server.
    Put {
        source: String,
        target: String,
    },
    /// Rename many items at once: (current path, new path) pairs. They
    /// are ordered so swaps and chains work, and nothing is overwritten.
    MultiRename {
        pairs: Vec<(String, String)>,
    },
    /// Carry out a sync plan between `left_root` (on the source
    /// connection) and `right_root` (on the target connection).
    Sync {
        left_root: String,
        right_root: String,
        steps: Vec<SyncStep>,
    },
    /// Copy items out of an opened archive (the source connection).
    /// `blocked` lists entries the archive left out as unsafe, for the log.
    Extract {
        archive_name: String,
        sources: Vec<String>,
        dest_dir: String,
        rename_to: Option<String>,
        blocked: Vec<String>,
    },
    /// Run shell command lines one after the other in `cwd` (a folder on
    /// this Mac), through the user's login shell. `title` names the
    /// activity ("Running “Lint”"), `summary` the log entry.
    Shell {
        /// What failed, in messages: the tool's name.
        name: String,
        title: String,
        summary: String,
        commands: Vec<String>,
        cwd: String,
    },
    /// Build an archive named `name` (with extension; made unique if
    /// taken) in `dest_dir` from `sources`.
    Compress {
        sources: Vec<String>,
        dest_dir: String,
        name: String,
        options: crate::archive::CompressOptions,
    },
}

/// One row of a sync plan to carry out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncStep {
    /// Path relative to both roots.
    pub path: String,
    pub action: crate::sync::Action,
    /// True if the right side changes (copy left to right, or delete on
    /// the right); false for the left side.
    pub to_right: bool,
    pub is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityInfo {
    pub id: ActivityId,
    /// Window that started it.
    pub window: u64,
    pub title: String,
    pub state: ActivityState,
    pub progress: Progress,
    /// Bytes per second, smoothed.
    pub speed: u64,
    /// Seconds left, when it can be estimated.
    pub eta_secs: Option<u64>,
    /// Waiting for an answer to a conflict or error prompt.
    pub waiting_for_user: bool,
}

#[derive(Debug, Clone)]
pub enum Prompt {
    Conflict(Box<ConflictInfo>),
    Error(ErrorInfo),
}

/// Sent once when an activity ends, however it ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishReport {
    pub id: ActivityId,
    pub window: u64,
    pub title: String,
    pub result: ActivityResult,
    /// Why it could not start or what failed, for showing to the user.
    pub message: Option<String>,
    /// Top-level items to select in the pane afterwards.
    pub results: Vec<String>,
    /// The new item should go straight into inline rename.
    pub rename_after: bool,
    /// A transfer, delete, trash or undo: worth a completion sound. Quick
    /// edits such as rename or new folder are not.
    pub notable: bool,
    /// What a shell command printed (custom tools, git).
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    /// Milliseconds since the Unix epoch.
    pub time_ms: i64,
    /// What happened, in the past tense: "Copied 12 items".
    pub title: String,
    pub result: ActivityResult,
    /// Renames (`a → b`), conflict resolutions, failed paths with messages.
    pub lines: Vec<String>,
}

/// Called on worker threads.
pub trait ActivityObserver: Send + Sync {
    /// The list of running and queued activities changed (throttled).
    fn activities_changed(&self, activities: Vec<ActivityInfo>);
    /// An activity needs a decision; answer with `ActivityManager::answer_*`.
    fn prompt(&self, activity: ActivityId, prompt: Prompt);
    fn finished(&self, report: FinishReport);
}

/// Things only the platform shell can do.
pub trait Platform: Send + Sync {
    /// Move an item to the Trash. Returns where it now is.
    fn trash(&self, path: &str) -> Result<String, String>;

    /// Make a Finder alias file at the new path `alias` that points to
    /// `target`.
    fn make_alias(&self, _target: &str, _alias: &str) -> Result<(), String> {
        Err("Aliases can’t be made here.".into())
    }

    /// Replace the Finder tags of `path`.
    fn set_tags(&self, _path: &str, _tags: &[String]) -> Result<(), String> {
        Err("Tags can’t be changed here.".into())
    }

    /// Replace the Finder comment of `path`.
    fn set_comment(&self, _path: &str, _comment: &str) -> Result<(), String> {
        Err("Comments can’t be changed here.".into())
    }

    /// Run one step with administrator rights, through the system's own
    /// authorization prompt. Returns the new path of a trashed item.
    fn elevate(&self, _action: &Privileged) -> Result<String, String> {
        Err("Administrator rights are not available here.".into())
    }
}

/// One undoable step: the changes to reverse, newest last.
#[derive(Debug, Clone, PartialEq, Eq)]
struct UndoRecord {
    /// Names the operation: "Move to Trash of 3 items".
    name: String,
    effects: Vec<Effect>,
    /// Connections the operation ran between (source side, target side).
    src: ConnId,
    dst: ConnId,
}

#[derive(Default)]
struct UndoStacks {
    undo: Vec<UndoRecord>,
    redo: Vec<UndoRecord>,
}

enum Answer {
    Conflict(ConflictAnswer),
    Error(ErrorChoice),
}

struct Control {
    stop: AtomicBool,
    paused: Mutex<bool>,
    wake: Condvar,
    answer: Mutex<Option<Answer>>,
    answered: Condvar,
}

enum Job {
    /// Run `request` from connection `src` to connection `dst` (the same
    /// one for operations that stay on one file system).
    Request { request: Request, src: ConnId, dst: ConnId },
    /// Reverse these effects (an undo or a redo).
    Revert { record: UndoRecord, is_redo: bool },
}

struct Entry {
    info: ActivityInfo,
    control: Arc<Control>,
    /// The work, until a thread takes it.
    job: Option<Job>,
    uses_queue: bool,
    /// (time, bytes done) of the last speed sample.
    sample: Option<(Instant, u64)>,
}

struct State {
    entries: Vec<Entry>,
    next_id: ActivityId,
    undo: HashMap<u64, UndoStacks>,
    log: Vec<LogEntry>,
    last_emit: Option<Instant>,
}

struct Inner {
    registry: Arc<VfsRegistry>,
    store: Arc<Store>,
    platform: Arc<dyn Platform>,
    observer: Arc<dyn ActivityObserver>,
    state: Mutex<State>,
    /// Bandwidth limits, each shared by all transfers of its direction.
    downloads: Arc<RateLimiter>,
    uploads: Arc<RateLimiter>,
    /// Partial files left by stopped transfers.
    partials: Arc<Partials>,
}

#[derive(Clone)]
pub struct ActivityManager {
    inner: Arc<Inner>,
}

/// Progress updates reach the UI at most this often.
const EMIT_INTERVAL: Duration = Duration::from_millis(100);

impl ActivityManager {
    pub fn new(
        registry: Arc<VfsRegistry>,
        store: Arc<Store>,
        platform: Arc<dyn Platform>,
        observer: Arc<dyn ActivityObserver>,
    ) -> Self {
        let log = store.load_log();
        let partials = Arc::new(Partials::open(store.partials_file()));
        Self {
            inner: Arc::new(Inner {
                downloads: Arc::new(RateLimiter::default()),
                uploads: Arc::new(RateLimiter::default()),
                partials,
                registry,
                store,
                platform,
                observer,
                state: Mutex::new(State {
                    entries: Vec::new(),
                    next_id: 1,
                    undo: HashMap::new(),
                    log,
                    last_emit: None,
                }),
            }),
        }
    }

    /// Start (or queue) an operation on the local disk for `window`.
    pub fn submit(&self, window: u64, request: Request) -> ActivityId {
        self.submit_on(window, request, LOCAL, LOCAL)
    }

    /// Start (or queue) an operation whose source paths are on `src` and
    /// whose target is on `dst`. Operations that touch one file system
    /// (delete, rename, new folder...) use `src` for it.
    pub fn submit_on(&self, window: u64, request: Request, src: ConnId, dst: ConnId) -> ActivityId {
        let title = title_for(&request);
        let uses_queue = self.uses_queue(&request, src, dst);
        self.add(window, title, Job::Request { request, src, dst }, uses_queue)
    }

    fn add(&self, window: u64, title: String, job: Job, uses_queue: bool) -> ActivityId {
        let id = {
            let mut st = self.inner.state.lock().unwrap();
            let id = st.next_id;
            st.next_id += 1;
            st.entries.push(Entry {
                info: ActivityInfo {
                    id,
                    window,
                    title,
                    state: ActivityState::Queued,
                    progress: Progress::default(),
                    speed: 0,
                    eta_secs: None,
                    waiting_for_user: false,
                },
                control: Arc::new(Control {
                    stop: AtomicBool::new(false),
                    paused: Mutex::new(false),
                    wake: Condvar::new(),
                    answer: Mutex::new(None),
                    answered: Condvar::new(),
                }),
                job: Some(job),
                uses_queue,
                sample: None,
            });
            id
        };
        pump(&self.inner);
        emit(&self.inner, true);
        id
    }

    /// Copies go through the transfer queue. Everything quick (deletes,
    /// renames, new items, moves within one volume) starts at once.
    fn uses_queue(&self, request: &Request, src: ConnId, dst: ConnId) -> bool {
        match request {
            Request::Transfer { mode: TransferMode::Copy, .. }
            | Request::Duplicate { .. }
            | Request::Put { .. }
            | Request::Extract { .. }
            | Request::Compress { .. }
            | Request::Sync { .. } => true,
            // A move is a quick rename only within one volume of one
            // file system; anything else has to copy.
            Request::Transfer { mode: TransferMode::Move, sources, dest_dir, .. } => {
                src != dst
                    || self
                        .inner
                        .registry
                        .get(src)
                        .is_none_or(|vfs| !sources.iter().all(|s| vfs.same_volume(s, dest_dir)))
            }
            _ => false,
        }
    }

    pub fn activities(&self) -> Vec<ActivityInfo> {
        let st = self.inner.state.lock().unwrap();
        st.entries.iter().map(|e| e.info.clone()).collect()
    }

    /// True if anything is running or queued (for `window`, or anywhere).
    pub fn has_active(&self, window: Option<u64>) -> bool {
        let st = self.inner.state.lock().unwrap();
        st.entries.iter().any(|e| window.is_none_or(|w| e.info.window == w))
    }

    // --- Controls

    pub fn pause(&self, id: ActivityId) {
        self.set_paused(id, true);
    }

    pub fn resume(&self, id: ActivityId) {
        self.set_paused(id, false);
    }

    fn set_paused(&self, id: ActivityId, paused: bool) {
        {
            let mut st = self.inner.state.lock().unwrap();
            let Some(entry) = st.entries.iter_mut().find(|e| e.info.id == id) else {
                return;
            };
            // A queued activity has nothing to pause; it simply waits.
            if entry.info.state == ActivityState::Queued {
                return;
            }
            *entry.control.paused.lock().unwrap() = paused;
            entry.control.wake.notify_all();
            entry.info.state = if paused {
                ActivityState::Paused
            } else if entry.info.progress.preparing {
                ActivityState::Preparing
            } else {
                ActivityState::Running
            };
            entry.sample = None;
            entry.info.speed = 0;
            entry.info.eta_secs = None;
        }
        emit(&self.inner, true);
    }

    /// Stop one activity. A queued one is removed without ever starting.
    pub fn stop(&self, id: ActivityId) {
        let never_started = {
            let mut st = self.inner.state.lock().unwrap();
            let Some(pos) = st.entries.iter().position(|e| e.info.id == id) else {
                return;
            };
            if st.entries[pos].job.is_some() {
                Some(st.entries.remove(pos))
            } else {
                let control = &st.entries[pos].control;
                control.stop.store(true, Ordering::SeqCst);
                control.wake.notify_all();
                control.answered.notify_all();
                None
            }
        };
        if let Some(entry) = never_started {
            let report = FinishReport {
                id,
                window: entry.info.window,
                title: entry.info.title.clone(),
                result: ActivityResult::Stopped,
                message: None,
                results: Vec::new(),
                rename_after: false,
                notable: true,
                output: None,
            };
            log(&self.inner, &entry.info.title, ActivityResult::Stopped, Vec::new());
            self.inner.observer.finished(report);
            emit(&self.inner, true);
        }
    }

    /// Stop every activity of `window`.
    pub fn stop_all(&self, window: u64) {
        let ids: Vec<ActivityId> = {
            let st = self.inner.state.lock().unwrap();
            st.entries.iter().filter(|e| e.info.window == window).map(|e| e.info.id).collect()
        };
        for id in ids {
            self.stop(id);
        }
    }

    /// Move a queued activity to `index` among the queued ones, so it
    /// starts earlier or later.
    pub fn reorder(&self, id: ActivityId, index: usize) {
        {
            let mut st = self.inner.state.lock().unwrap();
            let queued: Vec<usize> =
                st.entries.iter().enumerate().filter(|(_, e)| e.job.is_some()).map(|(i, _)| i).collect();
            let Some(from) = queued.iter().position(|i| st.entries[*i].info.id == id) else {
                return;
            };
            let to = index.min(queued.len() - 1);
            if from == to {
                return;
            }
            let entry = st.entries.remove(queued[from]);
            // Positions shifted by the removal; recompute the slot.
            let queued: Vec<usize> =
                st.entries.iter().enumerate().filter(|(_, e)| e.job.is_some()).map(|(i, _)| i).collect();
            let at = queued.get(to).copied().unwrap_or(st.entries.len());
            st.entries.insert(at, entry);
        }
        emit(&self.inner, true);
    }

    pub fn answer_conflict(&self, id: ActivityId, answer: ConflictAnswer) {
        self.answer(id, Answer::Conflict(answer));
    }

    pub fn answer_error(&self, id: ActivityId, choice: ErrorChoice) {
        self.answer(id, Answer::Error(choice));
    }

    fn answer(&self, id: ActivityId, answer: Answer) {
        let control = {
            let mut st = self.inner.state.lock().unwrap();
            let Some(entry) = st.entries.iter_mut().find(|e| e.info.id == id) else {
                return;
            };
            entry.info.waiting_for_user = false;
            Arc::clone(&entry.control)
        };
        *control.answer.lock().unwrap() = Some(answer);
        control.answered.notify_all();
        emit(&self.inner, true);
    }

    // --- Undo

    /// Menu title for Undo ("Undo Move to Trash"), if there is one.
    pub fn undo_title(&self, window: u64) -> Option<String> {
        let st = self.inner.state.lock().unwrap();
        st.undo.get(&window)?.undo.last().map(|r| format!("Undo {}", short_name(&r.name)))
    }

    pub fn redo_title(&self, window: u64) -> Option<String> {
        let st = self.inner.state.lock().unwrap();
        st.undo.get(&window)?.redo.last().map(|r| format!("Redo {}", short_name(&r.name)))
    }

    /// Reverse the window's most recent undoable operation.
    pub fn undo(&self, window: u64) -> Option<ActivityId> {
        self.revert(window, false)
    }

    pub fn redo(&self, window: u64) -> Option<ActivityId> {
        self.revert(window, true)
    }

    fn revert(&self, window: u64, is_redo: bool) -> Option<ActivityId> {
        let record = {
            let mut st = self.inner.state.lock().unwrap();
            let stacks = st.undo.get_mut(&window)?;
            if is_redo {
                stacks.redo.pop()
            } else {
                stacks.undo.pop()
            }
        }?;
        let title = format!("{} {}", if is_redo { "Redo" } else { "Undo" }, record.name);
        Some(self.add(window, title, Job::Revert { record, is_redo }, false))
    }

    /// Record a change the shell made itself (tags, a comment), so it can
    /// be undone like a file operation. `summary` is the log line.
    pub fn record(&self, window: u64, name: &str, summary: &str, effects: Vec<Effect>) {
        if effects.is_empty() {
            return;
        }
        {
            let mut st = self.inner.state.lock().unwrap();
            let stacks = st.undo.entry(window).or_default();
            stacks.undo.push(UndoRecord { name: name.to_string(), effects, src: LOCAL, dst: LOCAL });
            stacks.redo.clear();
        }
        log(&self.inner, summary, ActivityResult::Success, Vec::new());
    }

    // --- Log

    /// Finished operations, oldest first.
    pub fn log(&self) -> Vec<LogEntry> {
        self.inner.state.lock().unwrap().log.clone()
    }
}

/// Start queued activities while there is room.
fn pump(inner: &Arc<Inner>) {
    let max = inner.store.settings().max_transfers.clamp(1, 10) as usize;
    let mut to_start = Vec::new();
    {
        let mut st = inner.state.lock().unwrap();
        let mut running = st.entries.iter().filter(|e| e.uses_queue && e.job.is_none()).count();
        for entry in st.entries.iter_mut().filter(|e| e.job.is_some()) {
            if entry.uses_queue {
                if running >= max {
                    continue;
                }
                running += 1;
            }
            let job = entry.job.take().expect("filtered on is_some");
            entry.info.state = ActivityState::Preparing;
            to_start.push((
                entry.info.id,
                entry.info.window,
                entry.info.title.clone(),
                Arc::clone(&entry.control),
                job,
            ));
        }
    }
    for (id, window, title, control, job) in to_start {
        let inner = Arc::clone(inner);
        thread::Builder::new()
            .name("rustyfinder-activity".into())
            .spawn(move || run(inner, id, window, title, control, job))
            .expect("failed to spawn activity thread");
    }
}

/// Tell the observer about the current list. Progress ticks are throttled;
/// structural changes pass `force`.
fn emit(inner: &Arc<Inner>, force: bool) {
    let list = {
        let mut st = inner.state.lock().unwrap();
        let now = Instant::now();
        if !force && st.last_emit.is_some_and(|t| now.duration_since(t) < EMIT_INTERVAL) {
            return;
        }
        st.last_emit = Some(now);
        st.entries.iter().map(|e| e.info.clone()).collect::<Vec<_>>()
    };
    inner.observer.activities_changed(list);
}

fn log(inner: &Arc<Inner>, title: &str, result: ActivityResult, lines: Vec<String>) {
    let mut st = inner.state.lock().unwrap();
    st.log.push(LogEntry {
        time_ms: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0),
        title: title.to_string(),
        result,
        lines,
    });
    let extra = st.log.len().saturating_sub(MAX_LOG_ENTRIES);
    st.log.drain(..extra);
    inner.store.save_log(&st.log);
}

/// The engine's view of one running activity.
struct Worker {
    inner: Arc<Inner>,
    id: ActivityId,
    control: Arc<Control>,
}

impl Worker {
    fn stopped(&self) -> bool {
        self.control.stop.load(Ordering::SeqCst)
    }

    /// Show a prompt and block until it is answered or the activity stops.
    fn ask(&self, prompt: Prompt) -> Option<Answer> {
        {
            let mut st = self.inner.state.lock().unwrap();
            if let Some(entry) = st.entries.iter_mut().find(|e| e.info.id == self.id) {
                entry.info.waiting_for_user = true;
                entry.sample = None;
                entry.info.speed = 0;
                entry.info.eta_secs = None;
            }
        }
        emit(&self.inner, true);
        self.inner.observer.prompt(self.id, prompt);

        let mut answer = self.control.answer.lock().unwrap();
        loop {
            if let Some(a) = answer.take() {
                return Some(a);
            }
            if self.stopped() {
                return None;
            }
            // Timed wait so a stop is noticed even if no answer ever comes.
            answer = self.control.answered.wait_timeout(answer, Duration::from_millis(200)).unwrap().0;
        }
    }
}

impl OpContext for Worker {
    fn checkpoint(&self) -> bool {
        let mut paused = self.control.paused.lock().unwrap();
        while *paused && !self.stopped() {
            paused = self.control.wake.wait_timeout(paused, Duration::from_millis(200)).unwrap().0;
        }
        !self.stopped()
    }

    fn progress(&self, progress: &Progress) {
        {
            let mut st = self.inner.state.lock().unwrap();
            let Some(entry) = st.entries.iter_mut().find(|e| e.info.id == self.id) else {
                return;
            };
            let now = Instant::now();
            match entry.sample {
                None => entry.sample = Some((now, progress.bytes_done)),
                Some((then, bytes)) => {
                    let elapsed = now.duration_since(then).as_secs_f64();
                    // Sample about once a second so the figure is steady.
                    if elapsed >= 1.0 {
                        let rate = progress.bytes_done.saturating_sub(bytes) as f64 / elapsed;
                        let smoothed =
                            if entry.info.speed == 0 { rate } else { entry.info.speed as f64 * 0.6 + rate * 0.4 };
                        entry.info.speed = smoothed as u64;
                        entry.sample = Some((now, progress.bytes_done));
                        let left = progress.bytes_total.saturating_sub(progress.bytes_done);
                        entry.info.eta_secs = (entry.info.speed > 0).then(|| left / entry.info.speed);
                    }
                }
            }
            entry.info.progress = progress.clone();
            if entry.info.state != ActivityState::Paused {
                entry.info.state = if progress.preparing { ActivityState::Preparing } else { ActivityState::Running };
            }
        }
        emit(&self.inner, false);
    }

    fn conflict(&self, info: &ConflictInfo) -> ConflictAnswer {
        match self.ask(Prompt::Conflict(Box::new(info.clone()))) {
            Some(Answer::Conflict(answer)) => answer,
            // Stopped (or answered with the wrong kind): end the activity.
            _ => ConflictAnswer { choice: ConflictChoice::Stop, apply_to_all: false },
        }
    }

    fn error(&self, info: &ErrorInfo) -> ErrorChoice {
        match self.ask(Prompt::Error(info.clone())) {
            Some(Answer::Error(choice)) => choice,
            _ => ErrorChoice::Stop,
        }
    }

    fn elevate(&self, action: &Privileged) -> Result<String, String> {
        self.inner.platform.elevate(action)
    }
}

/// What a finished job leaves behind.
struct Done {
    outcome: Outcome,
    /// Why nothing could be done, if a precondition failed.
    refused: Option<String>,
    /// Past-tense line for the log: "Copied 12 items".
    summary: String,
    /// Name for the undo menu and log: "Copy of 12 items".
    undo_name: String,
    rename_after: bool,
}

fn run(inner: Arc<Inner>, id: ActivityId, window: u64, title: String, control: Arc<Control>, job: Job) {
    let worker = Worker { inner: Arc::clone(&inner), id, control };
    emit(&inner, true);

    let notable = match &job {
        Job::Request { request, .. } => matches!(
            request,
            Request::Transfer { .. }
                | Request::Duplicate { .. }
                | Request::Delete { .. }
                | Request::Trash { .. }
                | Request::Extract { .. }
                | Request::Compress { .. }
                | Request::Sync { .. }
        ),
        Job::Revert { .. } => true,
    };
    // Undoing swaps the two sides: what was the target is now the source.
    let (done, revert, conns) = match job {
        Job::Request { request, src, dst } => (perform(&worker, &request, src, dst), None, (src, dst)),
        Job::Revert { record, is_redo } => {
            let done = revert_record(&worker, &record, is_redo);
            (done, Some(is_redo), (record.dst, record.src))
        }
    };

    let Done { outcome, refused, summary, undo_name, rename_after } = done;
    let result = if refused.is_some() {
        ActivityResult::Failed
    } else if outcome.stopped {
        ActivityResult::Stopped
    } else if !outcome.failures.is_empty() {
        if outcome.done == 0 {
            ActivityResult::Failed
        } else {
            ActivityResult::Partial
        }
    } else {
        ActivityResult::Success
    };

    let mut lines = outcome.notes.clone();
    lines.extend(outcome.failures.iter().map(|(path, message)| format!("Failed: {path}: {message}")));
    if let Some(reason) = &refused {
        lines.push(reason.clone());
    }
    let log_title =
        if result == ActivityResult::Success || result == ActivityResult::Partial { summary } else { title.clone() };
    log(&inner, &log_title, result, lines);

    {
        let mut st = inner.state.lock().unwrap();
        st.entries.retain(|e| e.info.id != id);
        // Undoing a creation means moving it to the Trash, and servers have
        // none. So only what can truly be put back is recorded.
        let undoable = conns.1 == LOCAL || outcome.effects.iter().all(|e| matches!(e, Effect::Moved { .. }));
        if !outcome.effects.is_empty() && undoable {
            let stacks = st.undo.entry(window).or_default();
            let record = UndoRecord { name: undo_name, effects: outcome.effects.clone(), src: conns.0, dst: conns.1 };
            match revert {
                // A fresh operation: it becomes the next thing to undo and
                // whatever could be redone is no longer valid.
                None => {
                    stacks.undo.push(record);
                    stacks.redo.clear();
                }
                Some(false) => stacks.redo.push(record),
                Some(true) => stacks.undo.push(record),
            }
        }
    }

    let message = refused.or_else(|| {
        outcome.failures.first().map(|(path, message)| {
            let more = outcome.failures.len() - 1;
            let name = paths::file_name(path);
            if more == 0 {
                format!("“{name}”: {message}")
            } else {
                format!("“{name}”: {message} ({more} more failed)")
            }
        })
    });
    inner.observer.finished(FinishReport {
        id,
        window,
        title,
        result,
        message,
        results: outcome.results,
        rename_after: rename_after && result == ActivityResult::Success,
        notable,
        output: outcome.output,
    });
    pump(&inner);
    emit(&inner, true);
}

fn policies(inner: &Inner) -> Policies {
    let s = inner.store.settings();
    Policies {
        file: s.replace_policy,
        folder: s.merge_policy,
        error: s.error_policy,
        resume: s.resume_policy,
        retries: if s.auto_retry { s.max_retries.clamp(1, 10) } else { 0 },
        ..Policies::default()
    }
}

/// An engine reading from `src` and writing to `dst`.
fn engine<'a>(worker: &'a Worker, src: &Arc<dyn Vfs>, dst: &Arc<dyn Vfs>, same: bool) -> Engine<'a> {
    engine_with(worker, src, dst, same, policies(&worker.inner))
}

/// Like `engine`, with these policies. Every engine of an activity is made
/// here, so all of them keep partial files and honour the bandwidth limit
/// of their direction (uploads when the target is a server, downloads when
/// only the source is).
fn engine_with<'a>(
    worker: &'a Worker,
    src: &Arc<dyn Vfs>,
    dst: &Arc<dyn Vfs>,
    same: bool,
    policies: Policies,
) -> Engine<'a> {
    let inner = &worker.inner;
    let s = inner.store.settings();
    let kbps = |on: bool, value: u32| if on { u64::from(value.max(1)) * 1024 } else { 0 };
    inner.downloads.set_rate(kbps(s.limit_downloads, s.download_limit_kbps));
    inner.uploads.set_rate(kbps(s.limit_uploads, s.upload_limit_kbps));
    let limiter = if inner.registry.is_server(dst) {
        Some(Arc::clone(&inner.uploads))
    } else if inner.registry.is_server(src) {
        Some(Arc::clone(&inner.downloads))
    } else {
        None
    };
    Engine::new(worker, Arc::clone(src), Arc::clone(dst), same, policies)
        .with_limiter(limiter)
        .with_partials(Some(Arc::clone(&inner.partials)))
}

fn perform(worker: &Worker, request: &Request, src: ConnId, dst: ConnId) -> Done {
    let mut done = Done {
        outcome: Outcome { undoable: true, ..Default::default() },
        refused: None,
        summary: String::new(),
        undo_name: String::new(),
        rename_after: false,
    };
    let registry = &worker.inner.registry;
    let (Some(src_vfs), Some(dst_vfs)) = (registry.get(src), registry.get(dst)) else {
        done.refused = Some("The connection is closed.".into());
        return done;
    };
    let same = src == dst;
    let vfs = &*src_vfs;
    let engine = || engine(worker, &src_vfs, &dst_vfs, same);
    let engine_here = || self::engine(worker, &src_vfs, &src_vfs, true);
    // One-step operations share this shape: do it, record the effect.
    let mut single = |result: Result<(String, Effect), VfsError>, verb: &str| match result {
        Ok((path, effect)) => {
            done.outcome.done = 1;
            done.outcome.results.push(path);
            done.outcome.effects.push(effect);
        }
        Err(e) => done.refused = Some(format!("{verb}: {e}")),
    };

    match request {
        Request::Transfer { mode, sources, dest_dir, rename_to } => {
            let (verb, past) = match mode {
                TransferMode::Copy => ("Copy", "Copied"),
                TransferMode::Move => ("Move", "Moved"),
            };
            match engine().transfer(*mode, sources, dest_dir, rename_to.as_deref()) {
                Ok(outcome) => done.outcome = outcome,
                Err(e) => done.refused = Some(e.to_string()),
            }
            done.summary = format!("{past} {}", count(done.outcome.done));
            done.undo_name = format!("{verb} of {}", items(sources));
        }
        Request::Duplicate { paths } => {
            done.outcome = engine_here().duplicate(paths);
            done.summary = format!("Duplicated {}", items(paths));
            done.undo_name = format!("Duplicate of {}", items(paths));
        }
        Request::Delete { paths } => {
            done.outcome = engine_here().delete(paths);
            done.summary = format!("Deleted {}", items(paths));
        }
        Request::Trash { .. } if src != LOCAL => {
            done.refused = Some("There is no Trash on a server. Use Delete instead.".into());
        }
        Request::Trash { paths } => {
            for path in paths {
                if !worker.checkpoint() {
                    done.outcome.stopped = true;
                    break;
                }
                match trash_one(worker, path) {
                    Ok(Some(trashed)) => {
                        done.outcome.done += 1;
                        done.outcome.effects.push(Effect::Trashed { original: path.clone(), trashed });
                    }
                    Ok(None) => {
                        done.outcome.stopped = true;
                        break;
                    }
                    Err(message) => done.outcome.failures.push((path.clone(), message)),
                }
            }
            done.summary = format!("Moved {} to the Trash", count(done.outcome.done));
            done.undo_name = format!("Move to Trash of {}", items(paths));
        }
        Request::Rename { path, new_name } => {
            let old = paths::file_name(path).to_string();
            let renamed = simple::rename(vfs, path, new_name, src == LOCAL);
            if let Ok(new_path) = &renamed {
                done.outcome.notes.push(format!("{old} → {}", paths::file_name(new_path)));
            }
            single(renamed.map(|to| (to.clone(), Effect::Moved { from: path.clone(), to })), "Rename");
            done.summary = "Renamed 1 item".into();
            done.undo_name = "Rename".into();
        }
        Request::NewFolder { dir } => {
            single(
                simple::new_folder(vfs, dir, "untitled folder").map(|p| (p.clone(), Effect::Created { path: p })),
                "New Folder",
            );
            done.summary = "Created a folder".into();
            done.undo_name = "New Folder".into();
            done.rename_after = true;
        }
        Request::NewFile { dir } => {
            single(
                simple::new_file(vfs, dir, "untitled file.txt").map(|p| (p.clone(), Effect::Created { path: p })),
                "New File",
            );
            done.summary = "Created a file".into();
            done.undo_name = "New File".into();
            done.rename_after = true;
        }
        Request::NewFolderWithSelection { dir, paths } => {
            match simple::new_folder(vfs, dir, "New Folder With Items") {
                Ok(folder) => {
                    match engine_here().transfer(TransferMode::Move, paths, &folder, None) {
                        Ok(mut outcome) => {
                            // Recorded first so undo moves the items back
                            // out before the folder goes to the Trash.
                            outcome.effects.insert(0, Effect::Created { path: folder.clone() });
                            outcome.results = vec![folder];
                            done.outcome = outcome;
                        }
                        Err(e) => {
                            let _ = vfs.remove_dir(&folder);
                            done.refused = Some(e.to_string());
                        }
                    }
                }
                Err(e) => done.refused = Some(format!("New Folder: {e}")),
            }
            done.summary = format!("Created a folder with {}", items(paths));
            done.undo_name = "New Folder with Selection".into();
            done.rename_after = true;
        }
        Request::SetMode { paths, mode, recursive } => {
            done.outcome = engine_here().set_mode(paths, *mode, *recursive);
            done.summary = format!("Changed permissions of {} to {}", items(paths), crate::perm::octal(*mode));
        }
        Request::Put { source, target } => {
            // The user edited this file and saved: the old remote copy is
            // replaced without a prompt, staged so it is never half-written.
            // A failure is not asked about in the activity: the app shows
            // its own "Remote Editing Error" with Retry once it has failed.
            let always_replace = Policies {
                file: crate::ops::engine::ConflictPolicy::Replace,
                error: crate::ops::engine::ErrorPolicy::Skip,
                ..policies(&worker.inner)
            };
            let dir = paths::parent(target).unwrap_or_else(|| "/".to_string());
            let name = paths::file_name(target).to_string();
            let run = engine_with(worker, &src_vfs, &dst_vfs, same, always_replace).transfer(
                TransferMode::Copy,
                std::slice::from_ref(source),
                &dir,
                Some(&name),
            );
            match run {
                Ok(outcome) => done.outcome = outcome,
                Err(e) => done.refused = Some(e.to_string()),
            }
            done.outcome.effects.clear();
            done.summary = format!("Uploaded “{name}”");
        }
        Request::MultiRename { pairs } => {
            // Case matters only where the file system keeps it apart.
            let case_insensitive = src == LOCAL;
            let steps = crate::rename::plan_steps(pairs, case_insensitive);
            let mut failed_pairs = std::collections::HashSet::new();
            for step in &steps {
                if !worker.checkpoint() {
                    done.outcome.stopped = true;
                    break;
                }
                // A pair whose earlier hop failed is not continued.
                if failed_pairs.contains(&step.pair) {
                    continue;
                }
                // Never overwrites: a name taken in the meantime fails
                // this item and leaves the others to proceed.
                match simple::rename_to_path(vfs, &step.from, &step.to) {
                    Ok(()) => done.outcome.effects.push(Effect::Moved { from: step.from.clone(), to: step.to.clone() }),
                    Err(e) => {
                        failed_pairs.insert(step.pair);
                        done.outcome.failures.push((pairs[step.pair].0.clone(), e.to_string()));
                    }
                }
            }
            // The log lists what the user asked for, not the temporary hops.
            if !done.outcome.stopped {
                for (index, (from, to)) in pairs.iter().enumerate() {
                    if from != to && !failed_pairs.contains(&index) {
                        done.outcome.done += 1;
                        done.outcome.results.push(to.clone());
                        done.outcome.notes.push(format!("{} → {}", paths::file_name(from), paths::file_name(to)));
                    }
                }
            }
            done.summary = format!("Renamed {}", count(done.outcome.done));
            done.undo_name = format!("Rename of {} items", pairs.len());
        }
        Request::Sync { left_root, right_root, steps } => {
            use crate::sync::Action;
            // The plan already decided every conflict: replace, no prompts.
            let decided = Policies {
                file: crate::ops::engine::ConflictPolicy::Replace,
                folder: crate::ops::engine::ConflictPolicy::Merge,
                ..policies(&worker.inner)
            };
            let (mut added, mut updated, mut deleted) = (0u64, 0u64, 0u64);
            let root = |right: bool| if right { right_root } else { left_root };
            // Copies first; deletions only after every copy has finished.
            let copies = steps.iter().filter(|s| s.action != Action::Delete);
            let deletions = steps.iter().filter(|s| s.action == Action::Delete);
            for (index, step) in copies.chain(deletions).enumerate() {
                if !worker.checkpoint() {
                    done.outcome.stopped = true;
                    break;
                }
                worker.progress(&Progress {
                    items_done: index as u64,
                    items_total: steps.len() as u64,
                    current: step.path.clone(),
                    ..Default::default()
                });
                let (to_vfs, from_vfs) = if step.to_right { (&dst_vfs, &src_vfs) } else { (&src_vfs, &dst_vfs) };
                let target = paths::join(root(step.to_right), &step.path);
                let source = paths::join(root(!step.to_right), &step.path);
                let outcome = match step.action {
                    Action::Delete => engine_with(worker, to_vfs, to_vfs, true, decided).delete(&[target]),
                    // A folder to add is just created; its contents are
                    // their own rows.
                    Action::Add if step.is_dir => {
                        let mut made = Outcome::default();
                        match to_vfs.mkdir(&target) {
                            Ok(()) | Err(VfsError::AlreadyExists(_)) => made.done = 1,
                            Err(e) => made.failures.push((target.clone(), e.to_string())),
                        }
                        made
                    }
                    Action::Add | Action::Update => {
                        let dir = paths::parent(&target).unwrap_or_else(|| "/".to_string());
                        engine_with(worker, from_vfs, to_vfs, same, decided)
                            .transfer(TransferMode::Copy, &[source], &dir, None)
                            .unwrap_or_else(|e| Outcome {
                                failures: vec![(target.clone(), e.to_string())],
                                ..Default::default()
                            })
                    }
                };
                if outcome.stopped {
                    done.outcome.stopped = true;
                    break;
                }
                if outcome.failures.is_empty() {
                    done.outcome.done += 1;
                    match step.action {
                        Action::Add => added += 1,
                        Action::Update => updated += 1,
                        Action::Delete => deleted += 1,
                    }
                }
                done.outcome.failures.extend(outcome.failures);
            }
            // The count is what was really synchronized, with its breakdown.
            done.summary = format!("Synchronized {}", count(done.outcome.done));
            done.outcome.notes.push(format!("Added {added}, updated {updated}, deleted {deleted}"));
            done.outcome.undoable = false;
        }
        Request::Extract { archive_name, sources, dest_dir, rename_to, blocked } => {
            // Extracting is copying out of the archive: conflicts, progress
            // and Stop are the copy engine's.
            match engine().transfer(TransferMode::Copy, sources, dest_dir, rename_to.as_deref()) {
                Ok(outcome) => done.outcome = outcome,
                Err(e) => done.refused = Some(format!("The archive “{archive_name}” can’t be extracted: {e}")),
            }
            for entry in blocked {
                done.outcome.notes.push(format!("Blocked “{entry}”: its path leads outside the destination."));
            }
            done.summary = format!("Extracted “{archive_name}”");
            done.undo_name = format!("Extract of “{archive_name}”");
        }
        Request::Shell { name, title: _, summary, commands, cwd } => {
            let mut output = String::new();
            for (index, command) in commands.iter().enumerate() {
                if worker.stopped() {
                    done.outcome.stopped = true;
                    break;
                }
                worker.progress(&Progress {
                    items_done: index as u64,
                    items_total: commands.len() as u64,
                    ..Default::default()
                });
                match crate::tools::run(command, cwd, &|| !worker.stopped()) {
                    Ok(result) => {
                        output.push_str(&result.output);
                        if result.stopped {
                            done.outcome.stopped = true;
                            break;
                        }
                        match result.exit_code {
                            Some(0) => done.outcome.done += 1,
                            code => {
                                let line = match code {
                                    Some(code) => format!("Task exit code: {code}"),
                                    None => "The task was ended by a signal.".to_string(),
                                };
                                if !output.is_empty() && !output.ends_with('\n') {
                                    output.push('\n');
                                }
                                // The alert shows the code and what the
                                // command said last, which is usually why.
                                let tail: Vec<&str> =
                                    result.output.lines().rev().filter(|l| !l.trim().is_empty()).take(5).collect();
                                let said: Vec<&str> = tail.into_iter().rev().collect();
                                let message =
                                    if said.is_empty() { line.clone() } else { format!("{line}\n{}", said.join("\n")) };
                                output.push_str(&line);
                                output.push('\n');
                                done.outcome.failures.push((name.clone(), message));
                            }
                        }
                    }
                    Err(e) => {
                        done.outcome.failures.push((name.clone(), format!("The command could not be started: {e}")))
                    }
                }
            }
            // The log keeps what was printed (the start of it, if long).
            done.outcome.notes.extend(output.lines().take(200).map(str::to_string));
            done.outcome.output = Some(output);
            done.outcome.undoable = false;
            done.summary = summary.clone();
        }
        Request::Compress { sources, dest_dir, name, options } => {
            done.summary = format!("Compressed {}", items(sources));
            done.undo_name = format!("Compress of {}", items(sources));
            match compress(worker, &src_vfs, &dst_vfs, src, dst, sources, dest_dir, name, options) {
                Ok(Some(path)) => {
                    done.outcome.done = 1;
                    done.outcome.results.push(path.clone());
                    done.outcome.effects.push(Effect::Created { path });
                }
                Ok(None) => done.outcome.stopped = true,
                Err(message) => done.refused = Some(format!("The archive can’t be created: {message}")),
            }
        }
        Request::Symlink { paths, dest_dir } => {
            for path in paths {
                match simple::make_symlink(vfs, path, dest_dir) {
                    Ok(link) => {
                        done.outcome.done += 1;
                        done.outcome.results.push(link.clone());
                        done.outcome.effects.push(Effect::Created { path: link });
                    }
                    Err(e) => done.outcome.failures.push((path.clone(), e.to_string())),
                }
            }
            done.summary = format!(
                "Created {}",
                if paths.len() == 1 { "a symlink".into() } else { format!("{} symlinks", paths.len()) }
            );
            done.undo_name = "Make Symlink".into();
        }
        Request::Alias { .. } if src != LOCAL => {
            done.refused = Some("Aliases can only be made on this Mac. Use Make Symlink instead.".into());
        }
        Request::Alias { paths, dest_dir } => {
            for path in paths {
                let taken = |name: &str| vfs.stat(&paths::join(dest_dir, name)).is_ok();
                let alias =
                    paths::join(dest_dir, &crate::ops::names::suffixed_name(paths::file_name(path), "alias", &taken));
                match worker.inner.platform.make_alias(path, &alias) {
                    Ok(()) => {
                        done.outcome.done += 1;
                        done.outcome.results.push(alias.clone());
                        done.outcome.effects.push(Effect::Created { path: alias });
                    }
                    Err(message) => done.outcome.failures.push((path.clone(), message)),
                }
            }
            done.summary = format!(
                "Created {}",
                if paths.len() == 1 { "an alias".into() } else { format!("{} aliases", paths.len()) }
            );
            done.undo_name = "Make Alias".into();
        }
    }
    done
}

/// Move one item to the Trash. Where this user may not remove it (its
/// folder is not writable, as with apps installed by an administrator),
/// the user is asked whether to do it as an administrator. `Ok(None)`
/// means the activity was stopped.
fn trash_one(worker: &Worker, path: &str) -> Result<Option<String>, String> {
    loop {
        let message = match worker.inner.platform.trash(path) {
            Ok(trashed) => return Ok(Some(trashed)),
            Err(message) => message,
        };
        if folder_writable(path) {
            return Err(message);
        }
        let info = ErrorInfo { path: path.to_string(), message: message.clone(), can_elevate: true };
        match worker.error(&info) {
            ErrorChoice::Retry => continue,
            ErrorChoice::Stop => return Ok(None),
            ErrorChoice::Elevate => {
                return worker
                    .elevate(&Privileged::Trash { path: path.to_string() })
                    .map(Some)
                    .map_err(|why| format!("Not done as administrator: {why}"));
            }
            ErrorChoice::Skip | ErrorChoice::SkipAll => return Err(message),
        }
    }
}

/// True if this user may add and remove items in the folder holding `path`.
fn folder_writable(path: &str) -> bool {
    let Some(dir) = paths::parent(path) else { return true };
    let Ok(c) = std::ffi::CString::new(dir) else { return true };
    // SAFETY: a valid NUL-terminated path; access() only reads it.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Reverse a record's effects, newest first. The reversals themselves are
/// collected so the step can be redone (or undone again).
fn revert_record(worker: &Worker, record: &UndoRecord, is_redo: bool) -> Done {
    let mut outcome = Outcome { undoable: true, ..Default::default() };
    let registry = &worker.inner.registry;
    // The items are on the operation's target side now; they go back to
    // its source side.
    let (Some(here), Some(back)) = (registry.get(record.dst), registry.get(record.src)) else {
        return Done {
            outcome,
            refused: Some("The connection is closed.".into()),
            summary: String::new(),
            undo_name: record.name.clone(),
            rename_after: false,
        };
    };
    let same = record.src == record.dst;
    for effect in record.effects.iter().rev() {
        if !worker.checkpoint() {
            outcome.stopped = true;
            break;
        }
        match effect {
            // Something we made goes to the Trash, never straight to nothing.
            Effect::Created { path } => match worker.inner.platform.trash(path) {
                Ok(trashed) => {
                    outcome.done += 1;
                    outcome.effects.push(Effect::Trashed { original: path.clone(), trashed });
                }
                Err(message) => outcome.failures.push((path.clone(), message)),
            },
            Effect::Moved { from, to } => match put_back(worker, &here, &back, same, to, from) {
                Ok(true) => {
                    outcome.done += 1;
                    outcome.results.push(from.clone());
                    outcome.effects.push(Effect::Moved { from: to.clone(), to: from.clone() });
                }
                Ok(false) => outcome.skipped += 1,
                Err(message) => outcome.failures.push((to.clone(), message)),
            },
            // The Trash is local, so both sides are the local disk.
            Effect::Trashed { original, trashed } => match put_back(worker, &here, &back, same, trashed, original) {
                Ok(true) => {
                    outcome.done += 1;
                    outcome.results.push(original.clone());
                    outcome.effects.push(Effect::Created { path: original.clone() });
                }
                Ok(false) => outcome.skipped += 1,
                Err(message) => outcome.failures.push((trashed.clone(), message)),
            },
            Effect::Tags { path, before, after } => match worker.inner.platform.set_tags(path, before) {
                Ok(()) => {
                    outcome.done += 1;
                    outcome.results.push(path.clone());
                    outcome.effects.push(Effect::Tags {
                        path: path.clone(),
                        before: after.clone(),
                        after: before.clone(),
                    });
                }
                Err(message) => outcome.failures.push((path.clone(), message)),
            },
            Effect::Comment { path, before, after } => match worker.inner.platform.set_comment(path, before) {
                Ok(()) => {
                    outcome.done += 1;
                    outcome.results.push(path.clone());
                    outcome.effects.push(Effect::Comment {
                        path: path.clone(),
                        before: after.clone(),
                        after: before.clone(),
                    });
                }
                Err(message) => outcome.failures.push((path.clone(), message)),
            },
        }
    }
    Done {
        outcome,
        refused: None,
        // Logged as what it is: "Undo Move to Trash of 3 items".
        summary: format!("{} {}", if is_redo { "Redo" } else { "Undo" }, record.name),
        undo_name: record.name.clone(),
        rename_after: false,
    }
}

/// Move `current` (on `here`) back to exactly `wanted` (on `back`). Uses
/// the engine, so a name that has since been taken raises the normal
/// conflict prompt instead of overwriting. Returns false if the user
/// skipped it.
fn put_back(
    worker: &Worker,
    here: &Arc<dyn Vfs>,
    back: &Arc<dyn Vfs>,
    same: bool,
    current: &str,
    wanted: &str,
) -> Result<bool, String> {
    let Some(dir) = paths::parent(wanted) else {
        return Err("The original location is not valid.".into());
    };
    if back.stat(&dir).is_err() {
        return Err(format!("The original folder “{}” no longer exists.", paths::file_name(&dir)));
    }
    let name = paths::file_name(wanted).to_string();
    // Undoing a rename: same folder, other name. A pure spelling change
    // ("Photo.jpg" back to "photo.jpg") is the same item, not a conflict.
    let in_place = same && paths::parent(current).as_deref() == Some(dir.as_str());
    if in_place && (here.stat(wanted).is_err() || here.same_item(current, wanted)) {
        return simple::rename_to_path(&**here, current, wanted).map(|()| true).map_err(|e| e.to_string());
    }
    let outcome = engine(worker, here, back, same)
        .transfer(TransferMode::Move, &[current.to_string()], &dir, Some(&name))
        .map_err(|e| e.to_string())?;
    if let Some((_, message)) = outcome.failures.first() {
        return Err(message.clone());
    }
    Ok(outcome.done > 0)
}

fn count(n: u64) -> String {
    if n == 1 {
        "1 item".into()
    } else {
        format!("{n} items")
    }
}

/// `"report.pdf"` for one path, `12 items` for several.
fn items(paths_: &[String]) -> String {
    match paths_ {
        [one] => format!("“{}”", paths::file_name(one)),
        many => format!("{} items", many.len()),
    }
}

/// Build an archive. The tool works on this Mac's disk, so items on a
/// server are downloaded to a scratch folder first, and an archive for a
/// server is built in one and uploaded. Returns the new archive's path,
/// or `None` if stopped.
#[allow(clippy::too_many_arguments)]
fn compress(
    worker: &Worker,
    src_vfs: &Arc<dyn Vfs>,
    dst_vfs: &Arc<dyn Vfs>,
    src: ConnId,
    dst: ConnId,
    sources: &[String],
    dest_dir: &str,
    name: &str,
    options: &crate::archive::CompressOptions,
) -> Result<Option<String>, String> {
    use crate::archive;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let local = worker.inner.registry.local();
    let scratch = std::env::temp_dir().join(format!(
        "rustyfinder-compress-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let scratch = scratch.to_string_lossy().into_owned();
    let staged = src != LOCAL || dst != LOCAL;
    if staged {
        std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    }
    let result = (|| -> Result<Option<String>, String> {
        // 1. The items, on this disk.
        let local_sources: Vec<String> = if src == LOCAL {
            sources.to_vec()
        } else {
            let items_dir = format!("{scratch}/items");
            std::fs::create_dir_all(&items_dir).map_err(|e| e.to_string())?;
            let fetched = engine(worker, src_vfs, &local, false)
                .transfer(TransferMode::Copy, sources, &items_dir, None)
                .map_err(|e| e.to_string())?;
            if fetched.stopped {
                return Ok(None);
            }
            if let Some((path, why)) = fetched.failures.first() {
                return Err(format!("“{}” could not be downloaded: {why}", paths::file_name(path)));
            }
            sources.iter().map(|s| paths::join(&items_dir, paths::file_name(s))).collect()
        };
        // 2. A name nothing else has, where the archive will end up.
        let ext = options.format.extension();
        let base = name.strip_suffix(&format!(".{ext}")).unwrap_or(name);
        let unique = archive::free_name(dest_dir, base, ext, &|path| dst_vfs.stat(path).is_ok());
        let build_at = if dst == LOCAL { paths::join(dest_dir, &unique) } else { format!("{scratch}/{unique}") };
        // 3. Build it.
        let total: u64 = local_sources
            .iter()
            .map(|s| crate::perm::measure(&*local, s, &|| true).map_or(1, |(items, _)| items + 1))
            .sum();
        let built =
            archive::compress(&local_sources, &build_at, options, &|| worker.checkpoint(), &mut |written, current| {
                worker.progress(&Progress {
                    items_done: written.min(total),
                    items_total: total.max(written),
                    current: current.to_string(),
                    ..Default::default()
                });
            })?;
        if !built {
            return Ok(None);
        }
        // 4. Send it to the server it is for.
        if dst != LOCAL {
            let sent = engine(worker, &local, dst_vfs, false)
                .transfer(TransferMode::Copy, std::slice::from_ref(&build_at), dest_dir, None)
                .map_err(|e| e.to_string())?;
            if sent.stopped {
                return Ok(None);
            }
            if let Some((_, why)) = sent.failures.first() {
                return Err(format!("The archive could not be uploaded: {why}"));
            }
        }
        Ok(Some(paths::join(dest_dir, &unique)))
    })();
    if staged {
        let _ = std::fs::remove_dir_all(&scratch);
    }
    result
}

fn title_for(request: &Request) -> String {
    match request {
        Request::Transfer { mode: TransferMode::Copy, sources, .. } => format!("Copying {}", items(sources)),
        Request::Transfer { mode: TransferMode::Move, sources, .. } => format!("Moving {}", items(sources)),
        Request::Duplicate { paths } => format!("Duplicating {}", items(paths)),
        Request::Delete { paths } => format!("Deleting {}", items(paths)),
        Request::Trash { paths } => format!("Moving {} to the Trash", items(paths)),
        Request::Rename { path, .. } => format!("Renaming {}", items(std::slice::from_ref(path))),
        Request::NewFolder { .. } => "Creating a folder".into(),
        Request::NewFile { .. } => "Creating a file".into(),
        Request::NewFolderWithSelection { paths, .. } => format!("Creating a folder with {}", items(paths)),
        Request::Symlink { paths, .. } => format!("Making a symlink to {}", items(paths)),
        Request::Alias { paths, .. } => format!("Making an alias to {}", items(paths)),
        Request::SetMode { paths, .. } => format!("Changing permissions of {}", items(paths)),
        Request::Put { target, .. } => format!("Uploading {}", items(std::slice::from_ref(target))),
        Request::MultiRename { pairs } => format!("Renaming {} items", pairs.len()),
        Request::Sync { steps, .. } => format!("Synchronizing {} items", steps.len()),
        Request::Extract { archive_name, .. } => format!("Extracting “{archive_name}”"),
        Request::Shell { title, .. } => title.clone(),
        Request::Compress { sources, .. } => format!("Compressing {}", items(sources)),
    }
}

/// "Move to Trash of 3 items" -> "Move to Trash", for the Edit menu.
fn short_name(name: &str) -> &str {
    name.split(" of ").next().unwrap_or(name)
}
