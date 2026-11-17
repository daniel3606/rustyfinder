//! The copy / move / delete engine.
//!
//! It runs on a worker thread and talks to the outside only through
//! `OpContext`: progress out, and decisions (conflicts, errors, pause, stop)
//! in. That keeps every rule here testable without threads or UI.
//!
//! Safety rules it enforces (file-ops and conflicts specs):
//! - nothing is overwritten unless the user or a policy said Replace
//! - a replaced target stays intact until its replacement is fully written
//! - symlinks are copied and deleted as links, never followed
//! - a moved item's source is removed only after it copied successfully

use std::collections::HashSet;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use unicode_normalization::UnicodeNormalization;

use crate::entry::{Entry, EntryKind};
use crate::error::{VfsError, VfsResult};
use crate::ops::names;
use crate::ops::partials::{PartialRecord, Partials};
pub use crate::ops::privileged::Privileged;
use crate::ops::throttle::RateLimiter;
use crate::paths;
use crate::sort::fold;
use crate::vfs::Vfs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferMode {
    Copy,
    Move,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConflictKind {
    FileFile,
    DirDir,
    /// File vs folder or folder vs file: Merge is not possible.
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    Replace,
    /// Replace only if the source is newer; otherwise skip.
    ReplaceOlder,
    KeepBoth,
    Skip,
    Stop,
    /// Folders only: combine contents.
    Merge,
    /// A partial file left by a stopped transfer: continue writing it.
    Resume,
}

#[derive(Debug, Clone)]
pub struct ConflictInfo {
    pub kind: ConflictKind,
    pub source_path: String,
    pub source: Entry,
    pub target_path: String,
    pub target: Entry,
    /// The target is what a stopped transfer of this source left behind,
    /// so Resume is on offer.
    pub resumable: bool,
    /// Folder vs folder: names in the target that the source lacks. A
    /// Replace would delete them, so it needs a confirmation.
    pub target_extras: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConflictAnswer {
    pub choice: ConflictChoice,
    /// Reuse this choice for later conflicts of the same kind.
    pub apply_to_all: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorChoice {
    Retry,
    Skip,
    SkipAll,
    Stop,
    /// Retry with administrator rights, through the system's prompt. Only
    /// offered when `ErrorInfo::can_elevate` says so. Once one works, later
    /// permission problems of the same activity are retried that way too.
    Elevate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorInfo {
    pub path: String,
    pub message: String,
    /// A permission problem on this Mac that an administrator could get
    /// past: the prompt may offer `ErrorChoice::Elevate`.
    pub can_elevate: bool,
}

/// What to do about a conflict without asking; `Ask` asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ConflictPolicy {
    #[default]
    Ask,
    Replace,
    ReplaceOlder,
    KeepBoth,
    Skip,
    Merge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ErrorPolicy {
    #[default]
    Ask,
    Skip,
    Stop,
}

/// What to do about a partial target a stopped transfer left behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ResumePolicy {
    #[default]
    Ask,
    Resume,
    Replace,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policies {
    /// File conflicts (and file-vs-folder ones).
    pub file: ConflictPolicy,
    /// Folder-vs-folder conflicts.
    pub folder: ConflictPolicy,
    pub error: ErrorPolicy,
    /// Partial targets (conflicts spec §6).
    pub resume: ResumePolicy,
    /// A failed item is tried again this many times on its own, after a
    /// back-off, before the error policy applies. 0 turns this off.
    pub retries: u32,
    /// The first back-off; it doubles with each retry, up to 30 s.
    pub retry_delay: Duration,
}

impl Default for Policies {
    fn default() -> Self {
        Self {
            file: ConflictPolicy::Ask,
            folder: ConflictPolicy::Ask,
            error: ErrorPolicy::Ask,
            resume: ResumePolicy::Ask,
            retries: 0,
            retry_delay: Duration::from_secs(2),
        }
    }
}

/// Back-offs never grow past this.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

/// A server's "retry after" is honoured up to this long...
const MAX_THROTTLE_WAIT: Duration = Duration::from_secs(300);
/// ...and this many times in a row for one item, so a server that always
/// says "later" cannot hold an activity forever.
const MAX_THROTTLED_RETRIES: u32 = 10;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// Still counting what there is to do.
    pub preparing: bool,
    pub items_done: u64,
    pub items_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Name of the item being worked on.
    pub current: String,
}

/// The engine's only link to the outside world.
pub trait OpContext: Send + Sync {
    /// Called between items and chunks. Blocks while paused; returns false
    /// once the operation should stop.
    fn checkpoint(&self) -> bool;
    fn progress(&self, progress: &Progress);
    /// Ask how to resolve a name conflict. May block for the user.
    fn conflict(&self, info: &ConflictInfo) -> ConflictAnswer;
    /// Ask what to do about a failed item. May block for the user.
    fn error(&self, info: &ErrorInfo) -> ErrorChoice;
    /// Carry out `action` with administrator rights (the system asks for
    /// the password). Returns the item's new path for a `Trash` step.
    fn elevate(&self, _action: &Privileged) -> Result<String, String> {
        Err("Administrator rights are not available here.".into())
    }

    /// Wait `duration` (a retry back-off, a bandwidth limit), staying
    /// responsive to Pause and Stop. False if the operation was stopped.
    fn wait(&self, duration: Duration) -> bool {
        let deadline = Instant::now() + duration;
        loop {
            if !self.checkpoint() {
                return false;
            }
            let now = Instant::now();
            if now >= deadline {
                return true;
            }
            std::thread::sleep((deadline - now).min(Duration::from_millis(100)));
        }
    }
}

/// A change an operation made, recorded so it can be undone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Created {
        path: String,
    },
    Moved {
        from: String,
        to: String,
    },
    Trashed {
        original: String,
        trashed: String,
    },
    /// Finder tags changed (undone by setting `before` again).
    Tags {
        path: String,
        before: Vec<String>,
        after: Vec<String>,
    },
    /// Finder comment changed.
    Comment {
        path: String,
        before: String,
        after: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Stopped by the user (or a Stop policy) before finishing.
    pub stopped: bool,
    /// Items transferred or deleted.
    pub done: u64,
    pub skipped: u64,
    /// (path, message) for every item that failed and was skipped.
    pub failures: Vec<(String, String)>,
    /// Human-readable notes for the log (conflict resolutions and the like).
    pub notes: Vec<String>,
    /// Top-level changes, for undo. Empty when `undoable` is false.
    pub effects: Vec<Effect>,
    /// False once something was replaced or merged: those cannot be put back.
    pub undoable: bool,
    /// Final paths of the top-level items, for selecting them afterwards.
    pub results: Vec<String>,
    /// What a shell command printed, when the operation was one.
    pub output: Option<String>,
}

/// The engine unwinds with this when the operation is stopped.
struct Stop;
type Flow<T = ()> = Result<T, Stop>;

/// How a single root is being transferred.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Copy,
    /// Same volume: a rename does it.
    MoveByRename,
    /// Across volumes or backends: copy, then delete the source.
    MoveByCopy,
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A name no user would pick, in the same folder as `path`, used to stage
/// a replacement next to its target.
fn temp_sibling(path: &str) -> String {
    let dir = paths::parent(path).unwrap_or_else(|| "/".to_string());
    let n = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    paths::join(&dir, &format!(".rustyfinder-tmp-{}-{n}", std::process::id()))
}

pub struct Engine<'a> {
    ctx: &'a dyn OpContext,
    src: Arc<dyn Vfs>,
    dst: Arc<dyn Vfs>,
    /// Source and target are the same backend, so rename and fast copy apply.
    same_backend: bool,
    policies: Policies,
    /// Sticky "apply to all" answers: file, folder, mixed, partial.
    sticky: [Option<ConflictChoice>; 4],
    skip_all_errors: bool,
    /// The user retried a step as administrator: do so again without asking.
    elevate_all: bool,
    progress: Progress,
    out: Outcome,
    /// Shared bandwidth limit for this transfer's direction, if any.
    limiter: Option<Arc<RateLimiter>>,
    /// Where stopped transfers record the partial files they leave.
    partials: Option<Arc<Partials>>,
    /// The item being retried automatically, and how often it was.
    retrying: (String, u32),
    /// The same for "retry after" answers.
    throttled: (String, u32),
}

impl<'a> Engine<'a> {
    pub fn new(
        ctx: &'a dyn OpContext,
        src: Arc<dyn Vfs>,
        dst: Arc<dyn Vfs>,
        same_backend: bool,
        policies: Policies,
    ) -> Self {
        Self {
            ctx,
            src,
            dst,
            same_backend,
            policies,
            sticky: [None; 4],
            skip_all_errors: false,
            elevate_all: false,
            progress: Progress::default(),
            out: Outcome { undoable: true, ..Default::default() },
            limiter: None,
            partials: None,
            retrying: (String::new(), 0),
            throttled: (String::new(), 0),
        }
    }

    /// Hold byte transfers to a shared bandwidth limit.
    pub fn with_limiter(mut self, limiter: Option<Arc<RateLimiter>>) -> Self {
        self.limiter = limiter;
        self
    }

    /// Keep the partial file of a stopped transfer (where both sides can
    /// resume) and record it here, so it can be resumed later.
    pub fn with_partials(mut self, partials: Option<Arc<Partials>>) -> Self {
        self.partials = partials;
        self
    }

    // ------------------------------------------------------------ Transfer

    /// Copy or move `sources` into `dest_dir`. `rename_to` gives a single
    /// source a new name ("Copy to as…"). An `Err` means a precondition
    /// failed and nothing was touched.
    pub fn transfer(
        mut self,
        mode: TransferMode,
        sources: &[String],
        dest_dir: &str,
        rename_to: Option<&str>,
    ) -> VfsResult<Outcome> {
        self.preflight(sources, dest_dir, rename_to)?;

        let steps: Vec<Step> = sources
            .iter()
            .map(|s| match mode {
                TransferMode::Copy => Step::Copy,
                TransferMode::Move if self.same_backend && self.src.same_volume(s, dest_dir) => Step::MoveByRename,
                TransferMode::Move => Step::MoveByCopy,
            })
            .collect();

        // Renames need no counting: each root is one step. Everything else
        // is measured first so progress and the free-space check are real.
        let all_renames = steps.iter().all(|s| *s == Step::MoveByRename);
        if all_renames {
            self.progress.items_total = sources.len() as u64;
        } else if self.prepare(sources).is_err() {
            self.out.stopped = true;
            return Ok(self.finish());
        }
        if !all_renames {
            self.check_space(sources, dest_dir, &steps)?;
        }

        for (source, step) in sources.iter().zip(steps) {
            let name = rename_to.unwrap_or_else(|| paths::file_name(source));
            let target = paths::join(dest_dir, name);
            if self.transfer_item(source, &target, step, true, all_renames).is_err() {
                self.out.stopped = true;
                break;
            }
        }
        Ok(self.finish())
    }

    /// Checks that must pass before a single byte is written.
    fn preflight(&self, sources: &[String], dest_dir: &str, rename_to: Option<&str>) -> VfsResult<()> {
        if sources.is_empty() {
            return Err(VfsError::Invalid("Nothing is selected.".into()));
        }
        if rename_to.is_some() && sources.len() != 1 {
            return Err(VfsError::Invalid("Only one item can be given a new name.".into()));
        }
        // Two sources that would land on the same name: one would silently
        // overwrite the other, so the whole operation is refused.
        let mut seen = HashSet::new();
        let mut clashes: Vec<&str> = Vec::new();
        for s in sources {
            let name = paths::file_name(s);
            if !seen.insert(fold(name)) && !clashes.contains(&name) {
                clashes.push(name);
            }
        }
        if !clashes.is_empty() {
            return Err(VfsError::Invalid(format!(
                "Several selected items are named {}. They can’t go into the same folder.",
                clashes.iter().map(|n| format!("“{n}”")).collect::<Vec<_>>().join(", ")
            )));
        }
        if self.same_backend {
            for s in sources {
                let name = rename_to.unwrap_or_else(|| paths::file_name(s));
                if s == dest_dir || is_inside(dest_dir, s) {
                    return Err(VfsError::Invalid(format!(
                        "“{}” can’t be copied or moved into itself.",
                        paths::file_name(s)
                    )));
                }
                // Replacing /a with /a/x/a would destroy the source mid-way.
                if is_inside(s, &paths::join(dest_dir, name)) {
                    return Err(VfsError::Invalid("An item can’t be replaced by an item it contains.".into()));
                }
            }
        }
        if self.dst.read_only(dest_dir) {
            return Err(VfsError::ReadOnly);
        }
        match self.dst.stat(dest_dir) {
            Ok(e) if e.is_dir => Ok(()),
            Ok(_) => Err(VfsError::NotADirectory(dest_dir.to_string())),
            Err(e) => Err(e),
        }
    }

    /// Count items and bytes. Stops early if the user stops the operation.
    fn prepare(&mut self, sources: &[String]) -> Flow {
        self.progress.preparing = true;
        for s in sources {
            let (items, bytes) = self.measure(&Arc::clone(&self.src), s, true)?;
            self.progress.items_total += items;
            self.progress.bytes_total += bytes;
        }
        self.progress.preparing = false;
        self.ctx.progress(&self.progress);
        Ok(())
    }

    /// Items and bytes under `path` (inclusive), not following symlinks.
    /// Unreadable parts count as far as they could be read.
    fn measure(&mut self, vfs: &Arc<dyn Vfs>, path: &str, report: bool) -> Flow<(u64, u64)> {
        if !self.ctx.checkpoint() {
            return Err(Stop);
        }
        let Ok(entry) = vfs.stat(path) else {
            return Ok((1, 0));
        };
        if entry.kind != EntryKind::Directory {
            return Ok((1, weight(&entry)));
        }
        let (mut items, mut bytes) = (1, 0);
        for child in vfs.list(path).unwrap_or_default() {
            let child_path = paths::join(path, &child.name);
            if child.kind == EntryKind::Directory {
                let (i, b) = self.measure(vfs, &child_path, report)?;
                items += i;
                bytes += b;
            } else {
                items += 1;
                bytes += weight(&child);
            }
        }
        if report {
            self.progress.items_done = 0;
            self.progress.current = paths::file_name(path).to_string();
            let mut shown = self.progress.clone();
            shown.items_total += items;
            self.ctx.progress(&shown);
        }
        Ok((items, bytes))
    }

    /// Refuse a copy that cannot fit. Only for local targets on another
    /// volume: a same-volume copy on APFS is a clone and needs no space.
    fn check_space(&self, sources: &[String], dest_dir: &str, steps: &[Step]) -> VfsResult<()> {
        let crosses = sources
            .iter()
            .zip(steps)
            .any(|(s, step)| *step != Step::MoveByRename && !(self.same_backend && self.src.same_volume(s, dest_dir)));
        if !crosses {
            return Ok(());
        }
        match self.dst.space(dest_dir) {
            Some(space) if self.progress.bytes_total > space.free => Err(VfsError::NoSpace),
            _ => Ok(()),
        }
    }

    /// Transfer one item (a root or a child during recursion) to `target`.
    fn transfer_item(&mut self, source: &str, target: &str, step: Step, top: bool, one_per_root: bool) -> Flow {
        if !self.ctx.checkpoint() {
            return Err(Stop);
        }
        let entry = loop {
            match self.src.stat(source) {
                Ok(e) => break e,
                Err(e) => match self.on_error(source, &e)? {
                    ErrorChoice::Retry => continue,
                    _ => return Ok(()),
                },
            }
        };
        self.progress.current = entry.name.clone();
        let is_dir = entry.kind == EntryKind::Directory;
        let mut target = target.to_string();
        let mut replace = false;
        let mut existing_is_dir = false;

        if self.same_backend && self.src.same_item(source, &target) {
            // An item pasted into the folder it is already in.
            if step != Step::Copy {
                self.account_skipped(source, &entry, one_per_root)?;
                return Ok(());
            }
            target = self.free_sibling(&target, |name, taken| names::duplicate_name(name, is_dir, taken));
        } else if let Some((stored, existing)) = self.existing_target(&target) {
            // On a server the stored spelling may differ (NFC vs NFD).
            target = stored;
            existing_is_dir = existing.kind == EntryKind::Directory;
            let kind = match (is_dir, existing_is_dir) {
                (true, true) => ConflictKind::DirDir,
                (false, false) => ConflictKind::FileFile,
                _ => ConflictKind::Mixed,
            };
            let resume_at = match kind {
                ConflictKind::FileFile if entry.kind == EntryKind::File => self.partial_offset(&entry, source, &target),
                _ => None,
            };
            let target_extras = match kind {
                ConflictKind::DirDir => self.target_extras(source, &target),
                _ => Vec::new(),
            };
            let info = ConflictInfo {
                kind,
                source_path: source.to_string(),
                source: entry.clone(),
                target_path: target.clone(),
                target: existing.clone(),
                resumable: resume_at.is_some(),
                target_extras,
            };
            match self.decide(&info)? {
                ConflictChoice::Resume => match resume_at {
                    Some(offset) => {
                        self.out.undoable = false;
                        self.out.notes.push(format!("Resumed: {target}"));
                        let done = self.copy_file(&entry, source, &target, false, false, Some(offset))?;
                        if done && step != Step::Copy {
                            if let Err(e) = self.src.remove_file(source) {
                                self.out.failures.push((source.to_string(), e.to_string()));
                            }
                        }
                        if done && top {
                            self.out.results.push(target);
                        }
                        return Ok(());
                    }
                    // Nothing to resume: leave the target alone.
                    None => {
                        self.account_skipped(source, &entry, one_per_root)?;
                        return Ok(());
                    }
                },
                ConflictChoice::Skip => {
                    self.out.notes.push(format!("Skipped (already exists): {target}"));
                    self.account_skipped(source, &entry, one_per_root)?;
                    return Ok(());
                }
                ConflictChoice::KeepBoth => {
                    target = self.free_sibling(&target, |name, taken| names::keep_both_name(name, is_dir, taken));
                    self.out.notes.push(format!("Kept both: {target}"));
                }
                ConflictChoice::ReplaceOlder => {
                    // An item without a date is never treated as newer.
                    let newer = matches!((entry.modified_ms, existing.modified_ms), (Some(s), Some(t)) if s > t);
                    if !newer {
                        self.out.notes.push(format!("Skipped (not newer): {target}"));
                        self.account_skipped(source, &entry, one_per_root)?;
                        return Ok(());
                    }
                    replace = true;
                }
                ConflictChoice::Replace => replace = true,
                ConflictChoice::Merge if kind == ConflictKind::DirDir => {
                    self.out.undoable = false;
                    self.out.notes.push(format!("Merged: {target}"));
                    return self.merge(source, &target, step, top, one_per_root);
                }
                // Merge makes no sense for a file; treat it as "leave it".
                ConflictChoice::Merge => {
                    self.account_skipped(source, &entry, one_per_root)?;
                    return Ok(());
                }
                ConflictChoice::Stop => return Err(Stop),
            }
            if replace {
                self.out.undoable = false;
                self.out.notes.push(format!("Replaced: {target}"));
            }
        }

        let transferred = match step {
            Step::MoveByRename => {
                self.move_by_rename(source, &target, replace, is_dir || existing_is_dir, one_per_root)?
            }
            _ if is_dir => self.copy_dir(source, &target, replace, step)?,
            _ => self.copy_file(&entry, source, &target, replace, existing_is_dir, None)?,
        };
        if !transferred {
            return Ok(());
        }
        if step == Step::MoveByCopy && !is_dir {
            // The copy is complete; only now may the source go.
            if let Err(e) = self.src.remove_file(source) {
                self.out.failures.push((source.to_string(), e.to_string()));
            }
        }
        if top {
            self.out.results.push(target.clone());
            self.out.effects.push(match step {
                Step::Copy => Effect::Created { path: target },
                _ => Effect::Moved { from: source.to_string(), to: target },
            });
        }
        Ok(())
    }

    /// Combine `source` folder into the existing `target` folder. Children
    /// that clash raise their own conflicts.
    fn merge(&mut self, source: &str, target: &str, step: Step, top: bool, one_per_root: bool) -> Flow {
        let Some(children) = self.list_source(source)? else {
            return Ok(());
        };
        if !one_per_root {
            self.progress.items_done += 1; // the folder itself
        }
        let failures_before = self.out.failures.len() + self.out.skipped as usize;
        for child in children {
            self.transfer_item(
                &paths::join(source, &child.name),
                &paths::join(target, &child.name),
                step,
                false,
                true,
            )?;
            if !one_per_root && child.kind != EntryKind::Directory {
                // Children moved by rename are not counted by transfer_item.
                if step == Step::MoveByRename {
                    self.progress.items_done += 1;
                }
            }
        }
        if step != Step::Copy && self.out.failures.len() + self.out.skipped as usize == failures_before {
            // Everything inside moved on; the emptied source folder can go.
            let _ = self.src.remove_dir(source);
        }
        if one_per_root && top {
            self.progress.items_done += 1;
        }
        if top {
            self.out.results.push(target.to_string());
        }
        self.out.done += 1;
        self.ctx.progress(&self.progress);
        Ok(())
    }

    /// Move within one volume. Returns false if the item was skipped.
    fn move_by_rename(
        &mut self,
        source: &str,
        target: &str,
        replace: bool,
        involves_dir: bool,
        one_per_root: bool,
    ) -> Flow<bool> {
        loop {
            let result = if !replace {
                self.src.rename(source, target, false)
            } else if !involves_dir {
                // File over file: one atomic step.
                self.src.rename(source, target, true)
            } else {
                // A folder is involved, so rename cannot replace directly.
                // Park the old target, move the new one in, then discard it.
                let aside = temp_sibling(target);
                self.src.rename(target, &aside, false).and_then(|()| {
                    match self.src.rename(source, target, false) {
                        Ok(()) => {
                            if let Err(e) = remove_tree(&*self.src, &aside) {
                                self.out.notes.push(format!("Could not remove replaced item {aside}: {e}"));
                            }
                            Ok(())
                        }
                        Err(e) => {
                            // Put the original back exactly as it was.
                            let _ = self.src.rename(&aside, target, false);
                            Err(e)
                        }
                    }
                })
            };
            if let Err(e) = result {
                let action = (!replace && self.is_local(&self.src))
                    .then(|| Privileged::Move { from: source.to_string(), to: target.to_string() });
                match self.on_failure(source, &e, action)? {
                    ErrorChoice::Retry => continue,
                    ErrorChoice::Elevate => {}
                    _ => return Ok(false),
                }
            }
            if one_per_root {
                self.progress.items_done += 1;
            } else {
                let (items, bytes) = self.measure(&Arc::clone(&self.src), target, false)?;
                self.progress.items_done += items;
                self.progress.bytes_done += bytes;
            }
            self.out.done += 1;
            self.ctx.progress(&self.progress);
            return Ok(true);
        }
    }

    /// Copy a folder and everything in it. Returns false if it was skipped.
    fn copy_dir(&mut self, source: &str, target: &str, replace: bool, step: Step) -> Flow<bool> {
        // A replacement is built beside the target and swapped in at the
        // end, so a stop or failure leaves the original untouched.
        let work = if replace { temp_sibling(target) } else { target.to_string() };
        loop {
            match self.dst.mkdir(&work) {
                Ok(()) => break,
                Err(e) => {
                    let action =
                        (!replace && self.is_local(&self.dst)).then(|| Privileged::MakeDir { path: work.clone() });
                    match self.on_failure(&work, &e, action)? {
                        ErrorChoice::Retry => continue,
                        ErrorChoice::Elevate => break,
                        _ => return Ok(false),
                    }
                }
            }
        }
        self.progress.items_done += 1;
        self.out.done += 1;

        let Some(children) = self.list_source(source)? else {
            return Ok(true);
        };
        let problems_before = self.out.failures.len() + self.out.skipped as usize;
        for child in children {
            let result = self.transfer_item(
                &paths::join(source, &child.name),
                &paths::join(&work, &child.name),
                step,
                false,
                false,
            );
            if result.is_err() {
                if replace {
                    // Stopped mid-replacement: discard the half-built copy.
                    let _ = remove_tree(&*self.dst, &work);
                }
                return Err(Stop);
            }
        }
        // Dates and attributes go on last: adding children touched them.
        let metadata = if self.same_backend {
            self.dst.copy_dir_metadata(source, &work)
        } else {
            self.src.stat(source).and_then(|e| self.dst.set_times(&work, e.modified_ms, e.created_ms))
        };
        if let Err(e) = metadata {
            self.out.notes.push(format!("Could not copy attributes of {source}: {e}"));
        }

        if replace {
            let aside = temp_sibling(target);
            let swapped = self.dst.rename(target, &aside, false).and_then(|()| {
                self.dst.rename(&work, target, false).inspect_err(|_| {
                    let _ = self.dst.rename(&aside, target, false);
                })
            });
            match swapped {
                Ok(()) => {
                    if let Err(e) = remove_tree(&*self.dst, &aside) {
                        self.out.notes.push(format!("Could not remove replaced item {aside}: {e}"));
                    }
                }
                Err(e) => {
                    let _ = remove_tree(&*self.dst, &work);
                    self.out.failures.push((target.to_string(), e.to_string()));
                    return Ok(false);
                }
            }
        }
        if step == Step::MoveByCopy && self.out.failures.len() + self.out.skipped as usize == problems_before {
            // Every child copied and was removed, so the folder is empty.
            if let Err(e) = self.src.remove_dir(source) {
                self.out.failures.push((source.to_string(), e.to_string()));
            }
        }
        Ok(true)
    }

    /// Copy one file or symlink. Returns false if it was skipped. With
    /// `resume_at`, the target already holds that many bytes of it.
    fn copy_file(
        &mut self,
        entry: &Entry,
        source: &str,
        target: &str,
        replace: bool,
        target_is_dir: bool,
        resume_at: Option<u64>,
    ) -> Flow<bool> {
        let work = if replace { temp_sibling(target) } else { target.to_string() };
        let base = self.progress.bytes_done;
        let mut resume_at = resume_at;
        loop {
            match self.copy_bytes(entry, source, &work, base, resume_at, !replace) {
                Ok(()) => break,
                Err(VfsError::Cancelled) => return Err(Stop),
                Err(e) => {
                    self.progress.bytes_done = base;
                    let action = (!replace && self.same_backend && self.is_local(&self.dst))
                        .then(|| Privileged::Copy { from: source.to_string(), to: work.clone() });
                    match self.on_failure(source, &e, action)? {
                        ErrorChoice::Retry => {
                            // A retry carries on from what the failed try
                            // left, instead of starting over.
                            resume_at = self.partial_offset(entry, source, &work);
                            continue;
                        }
                        ErrorChoice::Elevate => break,
                        _ => {
                            self.progress.items_done += 1;
                            self.progress.bytes_done = base + weight(entry);
                            return Ok(false);
                        }
                    }
                }
            }
        }
        if replace {
            let swapped = if target_is_dir {
                let aside = temp_sibling(target);
                self.dst.rename(target, &aside, false).and_then(|()| match self.dst.rename(&work, target, false) {
                    Ok(()) => {
                        let _ = remove_tree(&*self.dst, &aside);
                        Ok(())
                    }
                    Err(e) => {
                        let _ = self.dst.rename(&aside, target, false);
                        Err(e)
                    }
                })
            } else {
                // One atomic step: the target is either the old file or the
                // complete new one, never missing.
                let stored = self.dst.stored_name(target);
                self.dst.rename(&work, target, true).map(|()| {
                    // Where the volume ignores case the entry keeps its old
                    // spelling ("report.txt"); give it the source's
                    // ("Report.txt"). Same item, so nothing can be lost.
                    let wanted = paths::file_name(target);
                    if let Some(old) = stored.filter(|old| old != wanted) {
                        let dir = paths::parent(target).unwrap_or_else(|| "/".to_string());
                        let _ = self.dst.rename(&paths::join(&dir, &old), target, true);
                    }
                })
            };
            if let Err(e) = swapped {
                let _ = self.dst.remove_file(&work);
                self.out.failures.push((target.to_string(), e.to_string()));
                self.progress.items_done += 1;
                self.progress.bytes_done = base + weight(entry);
                return Ok(false);
            }
            // Whatever partial file was there has been replaced.
            self.forget_partial(target);
        }
        self.progress.items_done += 1;
        self.progress.bytes_done = base + weight(entry);
        self.out.done += 1;
        self.ctx.progress(&self.progress);
        Ok(true)
    }

    /// Write `source`'s content and metadata to the new path `work` (or,
    /// with `resume_at`, the rest of it into the partial file there). On a
    /// failure the partial file is removed, unless `may_keep` and it can be
    /// resumed later.
    fn copy_bytes(
        &mut self,
        entry: &Entry,
        source: &str,
        work: &str,
        base: u64,
        resume_at: Option<u64>,
        may_keep: bool,
    ) -> VfsResult<()> {
        let ctx = self.ctx;
        if self.same_backend && resume_at.is_none() {
            let progress = &mut self.progress;
            let fast = self.src.copy_within(source, work, &mut |copied| {
                progress.bytes_done = base + copied;
                ctx.progress(progress);
                ctx.checkpoint()
            })?;
            if fast {
                return Ok(());
            }
        }
        if entry.kind == EntryKind::Symlink {
            let link = entry.link_target.clone().unwrap_or_default();
            return self.dst.symlink(&link, work);
        }

        let offset = resume_at.unwrap_or(0);
        let mut reader = match resume_at {
            Some(at) if at > 0 => self.src.open_read_at(source, at)?,
            _ => self.src.open_read(source)?,
        };
        let mut writer = match resume_at {
            Some(at) => self.dst.open_write_at(work, at)?,
            None => self.dst.create(work)?,
        };
        let unlimited = 1024 * 1024;
        let limiter = self.limiter.clone();
        let mut buffer = vec![0u8; limiter.as_ref().map_or(unlimited, |l| l.chunk_size(unlimited))];
        let mut copied = offset;
        self.progress.bytes_done = base + offset;
        let result = (|| -> VfsResult<()> {
            loop {
                let n = reader.read(&mut buffer).map_err(|e| VfsError::from_io(source, &e))?;
                if n == 0 {
                    break;
                }
                writer.write_all(&buffer[..n]).map_err(|e| VfsError::from_io(work, &e))?;
                copied += n as u64;
                self.progress.bytes_done = base + copied;
                ctx.progress(&self.progress);
                if !ctx.checkpoint() {
                    return Err(VfsError::Cancelled);
                }
                if let Some(wait) = limiter.as_ref().map(|l| l.take(n as u64)).filter(|w| !w.is_zero()) {
                    if !ctx.wait(wait) {
                        return Err(VfsError::Cancelled);
                    }
                }
            }
            writer.flush().map_err(|e| VfsError::from_io(work, &e))
        })();
        drop(writer);
        if let Err(e) = result {
            if may_keep && copied > 0 && self.keeps_partial(entry, &e) {
                self.record_partial(entry, source, work);
            } else {
                let _ = self.dst.remove_file(work);
                self.forget_partial(work);
            }
            return Err(e);
        }
        if resume_at.is_some() {
            // A resumed file has to end up exactly as long as its source.
            let size = self.dst.stat(work)?.size;
            if size != entry.size {
                self.forget_partial(work);
                return Err(VfsError::Io {
                    path: work.to_string(),
                    message: format!(
                        "The resumed file has {size} bytes, but the original has {}. Replace it to copy it again.",
                        entry.size
                    ),
                });
            }
        }
        self.forget_partial(work);
        // Best effort: not every backend can store dates or permissions.
        let _ = self.dst.set_times(work, entry.modified_ms, entry.created_ms);
        if let Some(mode) = entry.mode {
            let _ = self.dst.set_mode(work, mode);
        }
        Ok(())
    }

    fn list_source(&mut self, source: &str) -> Flow<Option<Vec<Entry>>> {
        loop {
            match self.src.list(source) {
                Ok(children) => return Ok(Some(children)),
                Err(e) => match self.on_error(source, &e)? {
                    ErrorChoice::Retry => continue,
                    _ => return Ok(None),
                },
            }
        }
    }

    /// A free name next to `target`, picked by `rule`.
    fn free_sibling(&self, target: &str, rule: impl Fn(&str, &dyn Fn(&str) -> bool) -> String) -> String {
        let dir = paths::parent(target).unwrap_or_else(|| "/".to_string());
        let taken = |name: &str| self.dst.stat(&paths::join(&dir, name)).is_ok();
        paths::join(&dir, &rule(paths::file_name(target), &taken))
    }

    /// The item already at `target`, with the path it is stored under. On
    /// a server, a name differing only in Unicode normalisation (NFD from
    /// this Mac, NFC on the server) is the same name (conflicts spec §3).
    fn existing_target(&self, target: &str) -> Option<(String, Entry)> {
        if let Ok(entry) = self.dst.stat(target) {
            return Some((target.to_string(), entry));
        }
        let name = paths::file_name(target);
        if !self.dst.nfc_names() || name.is_ascii() {
            return None;
        }
        let wanted: String = name.nfc().collect();
        let dir = paths::parent(target)?;
        let found = self.dst.list(&dir).ok()?.into_iter().find(|e| e.name.nfc().collect::<String>() == wanted)?;
        Some((paths::join(&dir, &found.name), found))
    }

    /// Names in the folder `target` that are not in the folder `source`.
    fn target_extras(&self, source: &str, target: &str) -> Vec<String> {
        let (Ok(theirs), Ok(ours)) = (self.dst.list(target), self.src.list(source)) else {
            return Vec::new();
        };
        let ours: HashSet<String> = ours.iter().map(|e| fold(&e.name)).collect();
        let mut extras: Vec<String> =
            theirs.into_iter().filter(|e| !ours.contains(&fold(&e.name))).map(|e| e.name).collect();
        extras.sort();
        extras
    }

    /// Both sides can carry on a transfer from an offset.
    fn can_resume(&self) -> bool {
        self.src.resume_support().0 && self.dst.resume_support().1
    }

    /// How many bytes of `source` the file at `target` already holds, if a
    /// stopped transfer of this unchanged source left it there.
    fn partial_offset(&self, entry: &Entry, source: &str, target: &str) -> Option<u64> {
        let partials = self.partials.as_ref()?;
        if entry.kind != EntryKind::File || !self.can_resume() {
            return None;
        }
        let record = partials.find(&self.dst.id(), target)?;
        let same_source = record.source_fs == self.src.id()
            && record.source == source
            && record.source_size == entry.size
            && record.source_modified_ms == entry.modified_ms;
        let existing = self.dst.stat(target).ok()?;
        (same_source && existing.kind == EntryKind::File && existing.size < entry.size).then_some(existing.size)
    }

    /// Whether a transfer that failed with `error` leaves its partial file
    /// for a later resume. A full disk or a refusal will not get better by
    /// resuming, so those partial files go.
    fn keeps_partial(&self, entry: &Entry, error: &VfsError) -> bool {
        self.partials.is_some()
            && entry.kind == EntryKind::File
            && self.can_resume()
            && matches!(error, VfsError::Cancelled | VfsError::Io { .. } | VfsError::Throttled { .. })
    }

    fn record_partial(&self, entry: &Entry, source: &str, target: &str) {
        if let Some(partials) = &self.partials {
            partials.record(PartialRecord {
                target_fs: self.dst.id(),
                target: target.to_string(),
                source_fs: self.src.id(),
                source: source.to_string(),
                source_size: entry.size,
                source_modified_ms: entry.modified_ms,
            });
        }
    }

    fn forget_partial(&self, target: &str) {
        if let Some(partials) = &self.partials {
            partials.forget(&self.dst.id(), target);
        }
    }

    /// Count a skipped item (and what is under it) as handled, so progress
    /// still reaches the end.
    fn account_skipped(&mut self, source: &str, entry: &Entry, one_per_root: bool) -> Flow {
        self.out.skipped += 1;
        if one_per_root {
            self.progress.items_done += 1;
        } else if entry.kind == EntryKind::Directory {
            let (items, bytes) = self.measure(&Arc::clone(&self.src), source, false)?;
            self.progress.items_done += items;
            self.progress.bytes_done += bytes;
        } else {
            self.progress.items_done += 1;
            self.progress.bytes_done += weight(entry);
        }
        self.ctx.progress(&self.progress);
        Ok(())
    }

    // ------------------------------------------------------- Decisions

    fn decide(&mut self, info: &ConflictInfo) -> Flow<ConflictChoice> {
        let slot = match info.kind {
            _ if info.resumable => 3,
            ConflictKind::FileFile => 0,
            ConflictKind::DirDir => 1,
            ConflictKind::Mixed => 2,
        };
        let ask = |engine: &mut Self| {
            let answer = engine.ctx.conflict(info);
            if answer.apply_to_all && answer.choice != ConflictChoice::Stop {
                engine.sticky[slot] = Some(answer.choice);
            }
            answer.choice
        };
        let mut asked = false;
        let mut choice = if let Some(choice) = self.sticky[slot] {
            choice
        } else if info.resumable {
            match self.policies.resume {
                ResumePolicy::Ask => {
                    asked = true;
                    ask(self)
                }
                ResumePolicy::Resume => ConflictChoice::Resume,
                ResumePolicy::Replace => ConflictChoice::Replace,
                ResumePolicy::Skip => ConflictChoice::Skip,
            }
        } else {
            let policy = match info.kind {
                ConflictKind::DirDir => self.policies.folder,
                _ => self.policies.file,
            };
            match policy {
                ConflictPolicy::Ask => {
                    asked = true;
                    ask(self)
                }
                ConflictPolicy::Replace => ConflictChoice::Replace,
                ConflictPolicy::ReplaceOlder => ConflictChoice::ReplaceOlder,
                ConflictPolicy::KeepBoth => ConflictChoice::KeepBoth,
                ConflictPolicy::Skip => ConflictChoice::Skip,
                ConflictPolicy::Merge => ConflictChoice::Merge,
            }
        };
        // Replacing a folder deletes what only the target has. A policy or
        // an earlier "apply to all" does not cover that: the user sees the
        // prompt (which names those items) and confirms there.
        if !asked && choice == ConflictChoice::Replace && !info.target_extras.is_empty() {
            choice = self.ctx.conflict(info).choice;
        }
        if choice == ConflictChoice::Stop {
            return Err(Stop);
        }
        Ok(choice)
    }

    /// Decide what to do about a failed item. Returns Retry, or Skip after
    /// recording the failure; Stop unwinds.
    fn on_error(&mut self, path: &str, error: &VfsError) -> Flow<ErrorChoice> {
        if matches!(error, VfsError::Cancelled) {
            return Err(Stop);
        }
        if let Some(choice) = self.auto_retry(path, error)? {
            return Ok(choice);
        }
        let choice = self.ask_error(path, error, false);
        self.apply_error_choice(path, error, choice)
    }

    /// Server waits and automatic retries, before the policy or the user
    /// is asked. None when neither applies.
    fn auto_retry(&mut self, path: &str, error: &VfsError) -> Flow<Option<ErrorChoice>> {
        // A server that asks to wait: wait as long as it says, without
        // counting against the retry limit.
        if let VfsError::Throttled { retry_after_secs, .. } = error {
            if self.throttled.0 != path {
                self.throttled = (path.to_string(), 0);
            }
            if self.throttled.1 < MAX_THROTTLED_RETRIES {
                self.throttled.1 += 1;
                let delay = Duration::from_secs(*retry_after_secs).min(MAX_THROTTLE_WAIT);
                self.out.notes.push(format!("Server busy, retried after {} s: {path}", delay.as_secs()));
                return if self.ctx.wait(delay) { Ok(Some(ErrorChoice::Retry)) } else { Err(Stop) };
            }
        }
        // Automatic retries with a growing back-off, for failures that may
        // pass (the network, the server), before the error policy applies.
        if self.policies.retries > 0 && matches!(error, VfsError::Io { .. } | VfsError::Throttled { .. }) {
            if self.retrying.0 != path {
                self.retrying = (path.to_string(), 0);
            }
            if self.retrying.1 < self.policies.retries {
                let delay = self.policies.retry_delay.saturating_mul(1 << self.retrying.1.min(16)).min(MAX_RETRY_DELAY);
                self.retrying.1 += 1;
                self.out.notes.push(format!("Retry {} of {}: {path}", self.retrying.1, self.policies.retries));
                return if self.ctx.wait(delay) { Ok(Some(ErrorChoice::Retry)) } else { Err(Stop) };
            }
        }
        Ok(None)
    }

    /// What the policy or the user says about a failure.
    fn ask_error(&self, path: &str, error: &VfsError, can_elevate: bool) -> ErrorChoice {
        if self.skip_all_errors {
            return ErrorChoice::Skip;
        }
        match self.policies.error {
            ErrorPolicy::Skip => ErrorChoice::Skip,
            ErrorPolicy::Stop => ErrorChoice::Stop,
            ErrorPolicy::Ask => {
                self.ctx.error(&ErrorInfo { path: path.to_string(), message: error.to_string(), can_elevate })
            }
        }
    }

    /// True for this Mac's own disk, where an administrator can help.
    fn is_local(&self, vfs: &Arc<dyn Vfs>) -> bool {
        vfs.id() == "local"
    }

    /// Like `on_error`, but a permission problem that `action` would get
    /// past may be retried with administrator rights. Returns `Elevate`
    /// when that worked: the caller treats the step as done.
    fn on_failure(&mut self, path: &str, error: &VfsError, action: Option<Privileged>) -> Flow<ErrorChoice> {
        if matches!(error, VfsError::Cancelled) {
            return Err(Stop);
        }
        let Some(action) = action.filter(|_| matches!(error, VfsError::PermissionDenied(_))) else {
            return self.on_error(path, error);
        };
        if !self.elevate_all {
            let choice = self.ask_error(path, error, true);
            if choice != ErrorChoice::Elevate {
                return self.apply_error_choice(path, error, choice);
            }
        }
        match self.ctx.elevate(&action) {
            Ok(_) => {
                self.elevate_all = true;
                self.out.notes.push(format!("Done as administrator: {path}"));
                Ok(ErrorChoice::Elevate)
            }
            Err(why) => {
                // Declining the password is a failure, never a success.
                self.out.failures.push((path.to_string(), format!("Not done as administrator: {why}")));
                Ok(ErrorChoice::Skip)
            }
        }
    }

    fn apply_error_choice(&mut self, path: &str, error: &VfsError, choice: ErrorChoice) -> Flow<ErrorChoice> {
        match choice {
            // Nothing to elevate here: just try again.
            ErrorChoice::Retry | ErrorChoice::Elevate => Ok(ErrorChoice::Retry),
            ErrorChoice::Stop => {
                self.out.failures.push((path.to_string(), error.to_string()));
                Err(Stop)
            }
            ErrorChoice::Skip | ErrorChoice::SkipAll => {
                self.skip_all_errors |= choice == ErrorChoice::SkipAll;
                self.out.failures.push((path.to_string(), error.to_string()));
                Ok(ErrorChoice::Skip)
            }
        }
    }

    fn finish(mut self) -> Outcome {
        if !self.out.undoable {
            self.out.effects.clear();
        }
        self.out
    }

    // -------------------------------------------------------------- Others

    /// Copy each item next to itself as `<name> copy`. Never asks: the new
    /// name is always free.
    pub fn duplicate(mut self, sources: &[String]) -> Outcome {
        if self.prepare(sources).is_err() {
            self.out.stopped = true;
            return self.finish();
        }
        for source in sources {
            let Ok(entry) = self.src.stat(source) else {
                self.out.failures.push((source.clone(), "The item could not be found.".into()));
                continue;
            };
            let is_dir = entry.kind == EntryKind::Directory;
            let target = self.free_sibling(source, |name, taken| names::duplicate_name(name, is_dir, taken));
            self.progress.current = entry.name.clone();
            let copied = if is_dir {
                self.copy_dir(source, &target, false, Step::Copy)
            } else {
                self.copy_file(&entry, source, &target, false, false, None)
            };
            match copied {
                Ok(true) => {
                    self.out.results.push(target.clone());
                    self.out.effects.push(Effect::Created { path: target });
                }
                Ok(false) => {}
                Err(Stop) => {
                    self.out.stopped = true;
                    break;
                }
            }
        }
        self.finish()
    }

    /// Set permission bits on `targets`; with `recursive`, on everything
    /// inside them too. Symlinks are left alone: changing a link would
    /// really change its target.
    pub fn set_mode(mut self, targets: &[String], mode: u32, recursive: bool) -> Outcome {
        self.out.undoable = false;
        if recursive {
            if self.prepare(targets).is_err() {
                self.out.stopped = true;
                return self.finish();
            }
        } else {
            self.progress.items_total = targets.len() as u64;
        }
        for target in targets {
            if self.set_mode_item(target, mode, recursive).is_err() {
                self.out.stopped = true;
                break;
            }
        }
        self.finish()
    }

    fn set_mode_item(&mut self, path: &str, mode: u32, recursive: bool) -> Flow {
        if !self.ctx.checkpoint() {
            return Err(Stop);
        }
        self.progress.current = paths::file_name(path).to_string();
        let kind = match self.src.stat(path) {
            Ok(entry) => entry.kind,
            Err(e) => {
                self.on_error(path, &e)?;
                return Ok(());
            }
        };
        if kind != EntryKind::Symlink {
            loop {
                match self.src.set_mode(path, mode) {
                    Ok(()) => {
                        self.out.done += 1;
                        break;
                    }
                    Err(e) => match self.on_error(path, &e)? {
                        ErrorChoice::Retry => continue,
                        _ => break,
                    },
                }
            }
        }
        self.progress.items_done += 1;
        self.ctx.progress(&self.progress);
        if recursive && kind == EntryKind::Directory {
            if let Some(children) = self.list_source(path)? {
                for child in children {
                    self.set_mode_item(&paths::join(path, &child.name), mode, true)?;
                }
            }
        }
        Ok(())
    }

    /// Delete items permanently, recursively, never following symlinks.
    pub fn delete(mut self, paths_to_delete: &[String]) -> Outcome {
        self.out.undoable = false;
        if self.prepare(paths_to_delete).is_err() {
            self.out.stopped = true;
            return self.finish();
        }
        for path in paths_to_delete {
            if self.delete_item(path).is_err() {
                self.out.stopped = true;
                break;
            }
        }
        self.finish()
    }

    /// Returns true if the item (and everything under it) is gone.
    fn delete_item(&mut self, path: &str) -> Flow<bool> {
        if !self.ctx.checkpoint() {
            return Err(Stop);
        }
        self.progress.current = paths::file_name(path).to_string();
        let entry = loop {
            match self.src.stat(path) {
                Ok(e) => break e,
                Err(e) => match self.on_error(path, &e)? {
                    ErrorChoice::Retry => continue,
                    _ => return Ok(false),
                },
            }
        };
        let is_dir = entry.kind == EntryKind::Directory;
        if is_dir {
            let Some(children) = self.list_source(path)? else {
                return Ok(false);
            };
            let mut all_gone = true;
            for child in children {
                all_gone &= self.delete_item(&paths::join(path, &child.name))?;
            }
            if !all_gone {
                // Something inside stayed, so the folder has to stay too.
                self.progress.items_done += 1;
                return Ok(false);
            }
        }
        loop {
            let removed = if is_dir { self.src.remove_dir(path) } else { self.src.remove_file(path) };
            match removed {
                Ok(()) => break,
                Err(e) => {
                    let action =
                        self.is_local(&self.src).then(|| Privileged::Remove { path: path.to_string(), is_dir });
                    match self.on_failure(path, &e, action)? {
                        ErrorChoice::Retry => continue,
                        ErrorChoice::Elevate => break,
                        _ => {
                            self.progress.items_done += 1;
                            return Ok(false);
                        }
                    }
                }
            }
        }
        self.progress.items_done += 1;
        self.progress.bytes_done += weight(&entry);
        self.out.done += 1;
        self.ctx.progress(&self.progress);
        Ok(true)
    }
}

/// Bytes an item contributes to a transfer. A symlink is copied as a link,
/// so the size of whatever it points at does not count.
fn weight(entry: &Entry) -> u64 {
    if entry.kind == EntryKind::Symlink {
        0
    } else {
        entry.size
    }
}

/// True if `path` lies inside `ancestor`, compared by whole components.
fn is_inside(path: &str, ancestor: &str) -> bool {
    let ancestor = ancestor.trim_end_matches('/');
    path.len() > ancestor.len() && path.starts_with(ancestor) && path.as_bytes()[ancestor.len()] == b'/'
}

/// Delete a tree without asking anything. Used for the engine's own
/// leftovers (a replaced target parked aside, a half-built replacement).
pub fn remove_tree(vfs: &dyn Vfs, path: &str) -> VfsResult<()> {
    let entry = vfs.stat(path)?;
    if entry.kind != EntryKind::Directory {
        return vfs.remove_file(path);
    }
    for child in vfs.list(path)? {
        remove_tree(vfs, &paths::join(path, &child.name))?;
    }
    vfs.remove_dir(path)
}
