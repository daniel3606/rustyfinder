//! Engine behaviour against the real local file system, in temp dirs.
//! Test names carry the spec IDs they cover (FO = file-ops, CF = conflicts,
//! EC = edge-cases).

use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use super::engine::*;
use crate::error::VfsError;
use crate::local::LocalFs;
use crate::vfs::Vfs;

/// Scripted stand-in for the user: answers prompts from queues and records
/// what was asked.
#[derive(Default)]
struct Script {
    conflict_answers: Mutex<VecDeque<ConflictAnswer>>,
    error_answers: Mutex<VecDeque<ErrorChoice>>,
    conflicts_asked: Mutex<Vec<ConflictInfo>>,
    errors_asked: Mutex<Vec<ErrorInfo>>,
    /// Stop once this many checkpoints have passed (negative: never).
    stop_after: AtomicI64,
    last: Mutex<Progress>,
    /// Steps run "as administrator", and whether the password is given.
    elevated: Mutex<Vec<Privileged>>,
    refuse_password: bool,
}

impl Script {
    fn new() -> Self {
        let s = Self::default();
        s.stop_after.store(-1, Ordering::SeqCst);
        s
    }

    fn answering(choices: &[(ConflictChoice, bool)]) -> Self {
        let s = Self::new();
        *s.conflict_answers.lock().unwrap() = choices
            .iter()
            .map(|(choice, apply_to_all)| ConflictAnswer { choice: *choice, apply_to_all: *apply_to_all })
            .collect();
        s
    }

    fn asked(&self) -> usize {
        self.conflicts_asked.lock().unwrap().len()
    }
}

impl OpContext for Script {
    fn checkpoint(&self) -> bool {
        let left = self.stop_after.load(Ordering::SeqCst);
        if left < 0 {
            return true;
        }
        if left == 0 {
            return false;
        }
        self.stop_after.store(left - 1, Ordering::SeqCst);
        true
    }

    fn progress(&self, progress: &Progress) {
        *self.last.lock().unwrap() = progress.clone();
    }

    fn conflict(&self, info: &ConflictInfo) -> ConflictAnswer {
        self.conflicts_asked.lock().unwrap().push(info.clone());
        self.conflict_answers.lock().unwrap().pop_front().expect("unexpected conflict prompt")
    }

    fn error(&self, info: &ErrorInfo) -> ErrorChoice {
        self.errors_asked.lock().unwrap().push(info.clone());
        self.error_answers.lock().unwrap().pop_front().expect("unexpected error prompt")
    }

    /// Plays the administrator: opens the locked folder just for the step.
    fn elevate(&self, action: &Privileged) -> Result<String, String> {
        use std::os::unix::fs::PermissionsExt;
        self.elevated.lock().unwrap().push(action.clone());
        if self.refuse_password {
            return Err("User canceled.".into());
        }
        let folder = |p: &str| Path::new(p).parent().unwrap().to_path_buf();
        let dirs: Vec<_> = match action {
            Privileged::Remove { path, .. } | Privileged::MakeDir { path } | Privileged::Trash { path } => {
                vec![folder(path)]
            }
            Privileged::Move { from, to } | Privileged::Copy { from, to } => vec![folder(from), folder(to)],
        };
        for d in &dirs {
            fs::set_permissions(d, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let argv = action.argv("");
        let ok = std::process::Command::new(&argv[0]).args(&argv[1..]).status().unwrap().success();
        for d in &dirs {
            fs::set_permissions(d, fs::Permissions::from_mode(0o555)).unwrap();
        }
        if ok {
            Ok(String::new())
        } else {
            Err("failed".into())
        }
    }
}

struct Fx {
    tmp: tempfile::TempDir,
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("src")).unwrap();
        fs::create_dir(tmp.path().join("dst")).unwrap();
        Self { tmp }
    }

    fn p(&self, rel: &str) -> String {
        self.tmp.path().join(rel).to_str().unwrap().to_string()
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.tmp.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.tmp.path().join(rel)).unwrap()
    }

    fn exists(&self, rel: &str) -> bool {
        fs::symlink_metadata(self.tmp.path().join(rel)).is_ok()
    }

    /// Names in a folder, sorted, exactly as stored on disk.
    fn names(&self, rel: &str) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.tmp.path().join(rel))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn set_mtime(&self, rel: &str, ms: i64) {
        LocalFs.set_times(&self.p(rel), Some(ms), None).unwrap();
    }

    fn engine<'a>(&self, ctx: &'a Script, policies: Policies) -> Engine<'a> {
        let fs: Arc<dyn Vfs> = Arc::new(LocalFs);
        Engine::new(ctx, Arc::clone(&fs), fs, true, policies)
    }

    fn copy(&self, ctx: &Script, sources: &[&str], dest: &str) -> Outcome {
        let sources: Vec<String> = sources.iter().map(|s| self.p(s)).collect();
        self.engine(ctx, Policies::default()).transfer(TransferMode::Copy, &sources, &self.p(dest), None).unwrap()
    }

    fn mv(&self, ctx: &Script, sources: &[&str], dest: &str) -> Outcome {
        let sources: Vec<String> = sources.iter().map(|s| self.p(s)).collect();
        self.engine(ctx, Policies::default()).transfer(TransferMode::Move, &sources, &self.p(dest), None).unwrap()
    }
}

const OLD: i64 = 1_000_000_000_000;
const NEW: i64 = 1_500_000_000_000;

// ------------------------------------------------------------------ Copy

#[test]
fn fo01_copy_file_keeps_bytes_and_dates() {
    let f = Fx::new();
    f.write("src/a.txt", "hello");
    LocalFs.set_times(&f.p("src/a.txt"), Some(OLD), Some(OLD - 5000)).unwrap();
    let ctx = Script::new();
    let out = f.copy(&ctx, &["src/a.txt"], "dst");

    assert_eq!(f.read("dst/a.txt"), "hello");
    let e = LocalFs.stat(&f.p("dst/a.txt")).unwrap();
    assert_eq!(e.modified_ms, Some(OLD));
    assert_eq!(e.created_ms, Some(OLD - 5000));
    assert!(f.exists("src/a.txt"));
    assert_eq!((out.done, out.skipped, out.stopped), (1, 0, false));
    assert_eq!(out.results, vec![f.p("dst/a.txt")]);
    assert_eq!(out.effects, vec![Effect::Created { path: f.p("dst/a.txt") }]);
    let p = ctx.last.lock().unwrap().clone();
    assert_eq!((p.items_done, p.items_total, p.bytes_done, p.bytes_total), (1, 1, 5, 5));
    assert!(!p.preparing);
}

#[test]
fn copy_tree_keeps_structure_folder_dates_and_counts_everything() {
    let f = Fx::new();
    f.write("src/d/one.txt", "1");
    f.write("src/d/sub/two.txt", "22");
    fs::create_dir(f.tmp.path().join("src/d/empty")).unwrap();
    f.set_mtime("src/d/sub", OLD);
    f.set_mtime("src/d", OLD);
    let ctx = Script::new();
    let out = f.copy(&ctx, &["src/d"], "dst");

    assert_eq!(f.read("dst/d/one.txt"), "1");
    assert_eq!(f.read("dst/d/sub/two.txt"), "22");
    assert!(f.exists("dst/d/empty"));
    // Folder dates survive even though children were added after creation.
    assert_eq!(LocalFs.stat(&f.p("dst/d")).unwrap().modified_ms, Some(OLD));
    assert_eq!(LocalFs.stat(&f.p("dst/d/sub")).unwrap().modified_ms, Some(OLD));
    assert_eq!(out.done, 5); // d, one.txt, sub, two.txt, empty
    let p = ctx.last.lock().unwrap().clone();
    assert_eq!((p.items_done, p.items_total, p.bytes_done, p.bytes_total), (5, 5, 3, 3));
    assert_eq!(out.effects.len(), 1); // one undo step for the whole tree
}

#[test]
fn ec05_symlinks_are_copied_as_links_and_loops_do_not_recurse() {
    let f = Fx::new();
    f.write("src/F/real.txt", "x");
    symlink(".", f.tmp.path().join("src/F/loop")).unwrap();
    symlink("real.txt", f.tmp.path().join("src/F/link")).unwrap();
    symlink("nowhere", f.tmp.path().join("src/F/broken")).unwrap();
    let ctx = Script::new();
    let out = f.copy(&ctx, &["src/F"], "dst");

    assert!(!out.stopped && out.failures.is_empty());
    for (name, target) in [("loop", "."), ("link", "real.txt"), ("broken", "nowhere")] {
        let link = fs::read_link(f.tmp.path().join("dst/F").join(name)).unwrap();
        assert_eq!(link, Path::new(target), "{name}");
    }
    assert_eq!(f.names("dst/F"), vec!["broken", "link", "loop", "real.txt"]);
}

#[test]
fn hostile_names_copy_intact() {
    let f = Fx::new();
    let names = [
        " leading space",
        "trailing space ",
        "trailing dot.",
        ".dotfile",
        "emoji 📁🎉.txt",
        "rtl \u{5e9}\u{5dc}\u{5d5}\u{5dd}.txt",
        "cafe\u{301}.txt", // NFD
        "colon:slash",
        "zero-byte",
    ];
    for name in names {
        f.write(&format!("src/h/{name}"), if name == "zero-byte" { "" } else { name });
    }
    let long = "L".repeat(255);
    f.write(&format!("src/h/{long}"), "long");
    let ctx = Script::new();
    let out = f.copy(&ctx, &["src/h"], "dst");

    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert_eq!(f.names("dst/h"), f.names("src/h"));
    assert_eq!(f.read(&format!("dst/h/{long}")), "long");
    assert_eq!(f.read("dst/h/zero-byte"), "");
}

// ------------------------------------------------------- Preconditions

#[test]
fn fo04_copy_into_itself_is_refused_and_nothing_is_written() {
    let f = Fx::new();
    f.write("src/a/sub/x.txt", "x");
    let ctx = Script::new();
    for dest in ["src/a/sub", "src/a"] {
        let err = f
            .engine(&ctx, Policies::default())
            .transfer(TransferMode::Copy, &[f.p("src/a")], &f.p(dest), None)
            .unwrap_err();
        assert!(matches!(&err, VfsError::Invalid(m) if m.contains("into itself")), "{err}");
    }
    assert_eq!(f.names("src/a/sub"), vec!["x.txt"]);
    // A sibling whose name merely starts the same is fine.
    fs::create_dir(f.tmp.path().join("src/ab")).unwrap();
    assert!(f
        .engine(&ctx, Policies::default())
        .transfer(TransferMode::Copy, &[f.p("src/a")], &f.p("src/ab"), None)
        .is_ok());
}

#[test]
fn replacing_an_item_with_one_it_contains_is_refused() {
    let f = Fx::new();
    f.write("dst/a/x/a/inner.txt", "x");
    let ctx = Script::new();
    let err = f
        .engine(&ctx, Policies::default())
        .transfer(TransferMode::Copy, &[f.p("dst/a/x/a")], &f.p("dst"), None)
        .unwrap_err();
    assert!(matches!(&err, VfsError::Invalid(m) if m.contains("contains")), "{err}");
    assert!(f.exists("dst/a/x/a/inner.txt"));
}

#[test]
fn fo05_same_named_sources_stop_the_whole_operation() {
    let f = Fx::new();
    f.write("src/x/r.txt", "x");
    f.write("src/y/r.txt", "y");
    f.write("src/y/other.txt", "o");
    let ctx = Script::new();
    let err = f
        .engine(&ctx, Policies::default())
        .transfer(
            TransferMode::Copy,
            &[f.p("src/x/r.txt"), f.p("src/y/other.txt"), f.p("src/y/r.txt")],
            &f.p("dst"),
            None,
        )
        .unwrap_err();
    assert!(matches!(&err, VfsError::Invalid(m) if m.contains("“r.txt”")), "{err}");
    assert!(f.names("dst").is_empty());
}

#[test]
fn missing_or_non_folder_destination_is_refused() {
    let f = Fx::new();
    f.write("src/a.txt", "a");
    f.write("dst/file", "f");
    let ctx = Script::new();
    let run = |dest: &str| {
        f.engine(&ctx, Policies::default()).transfer(TransferMode::Copy, &[f.p("src/a.txt")], &f.p(dest), None)
    };
    assert!(matches!(run("nope"), Err(VfsError::NotFound(_))));
    assert!(matches!(run("dst/file"), Err(VfsError::NotADirectory(_))));
}

// ------------------------------------------------------------ Conflicts

#[test]
fn cf01_cf02_cf03_replace_older_follows_dates() {
    let f = Fx::new();
    for name in ["newer.txt", "older.txt"] {
        f.write(&format!("src/{name}"), "source");
        f.write(&format!("dst/{name}"), "target");
    }
    f.set_mtime("src/newer.txt", NEW);
    f.set_mtime("dst/newer.txt", OLD);
    f.set_mtime("src/older.txt", OLD);
    f.set_mtime("dst/older.txt", NEW);
    let ctx = Script::answering(&[(ConflictChoice::ReplaceOlder, true)]);
    let out = f.copy(&ctx, &["src/newer.txt", "src/older.txt"], "dst");

    assert_eq!(f.read("dst/newer.txt"), "source");
    assert_eq!(f.read("dst/older.txt"), "target");
    assert_eq!((out.done, out.skipped), (1, 1));
    assert!(out.notes.iter().any(|n| n.starts_with("Skipped (not newer)")));
    assert_eq!(ctx.asked(), 1); // "apply to all" covered the second one
}

#[test]
fn cf04_keep_both_takes_the_next_free_number() {
    let f = Fx::new();
    f.write("src/a.txt", "source");
    f.write("dst/a.txt", "target");
    f.write("dst/a 2.txt", "taken");
    let ctx = Script::answering(&[(ConflictChoice::KeepBoth, false)]);
    let out = f.copy(&ctx, &["src/a.txt"], "dst");

    assert_eq!(f.read("dst/a 3.txt"), "source");
    assert_eq!(f.read("dst/a.txt"), "target");
    assert_eq!(f.read("dst/a 2.txt"), "taken");
    assert_eq!(out.results, vec![f.p("dst/a 3.txt")]);
}

#[test]
fn cf05_merge_adds_missing_children_and_asks_about_clashing_ones() {
    let f = Fx::new();
    f.write("src/d/x", "new x");
    f.write("src/d/y", "y");
    f.write("dst/d/x", "old x");
    f.write("dst/d/keep", "keep");
    let ctx = Script::answering(&[(ConflictChoice::Merge, false), (ConflictChoice::Skip, false)]);
    let out = f.copy(&ctx, &["src/d"], "dst");

    assert_eq!(f.names("dst/d"), vec!["keep", "x", "y"]);
    assert_eq!(f.read("dst/d/x"), "old x");
    assert_eq!(f.read("dst/d/y"), "y");
    let asked = ctx.conflicts_asked.lock().unwrap();
    assert_eq!(asked[0].kind, ConflictKind::DirDir);
    assert_eq!(asked[1].kind, ConflictKind::FileFile);
    assert_eq!(asked[1].target_path, f.p("dst/d/x"));
    assert!(out.effects.is_empty(), "a merge cannot be undone");
}

#[test]
fn cf06_apply_to_all_skip_asks_once() {
    let f = Fx::new();
    let sources: Vec<String> = (0..5).map(|i| format!("src/f{i}")).collect();
    for s in &sources {
        f.write(s, "source");
        f.write(&s.replace("src/", "dst/"), "target");
    }
    let ctx = Script::answering(&[(ConflictChoice::Skip, true)]);
    let refs: Vec<&str> = sources.iter().map(String::as_str).collect();
    let out = f.copy(&ctx, &refs, "dst");

    assert_eq!(ctx.asked(), 1);
    assert_eq!((out.done, out.skipped), (0, 5));
    assert!((0..5).all(|i| f.read(&format!("dst/f{i}")) == "target"));
    let p = ctx.last.lock().unwrap().clone();
    assert_eq!(p.items_done, p.items_total); // skipped items still complete the bar
}

#[test]
fn apply_to_all_is_per_conflict_kind() {
    let f = Fx::new();
    f.write("src/file", "source");
    f.write("dst/file", "target");
    f.write("src/dir/inner", "i");
    f.write("dst/dir/other", "o");
    let ctx = Script::answering(&[(ConflictChoice::Skip, true), (ConflictChoice::Merge, true)]);
    f.copy(&ctx, &["src/file", "src/dir"], "dst");
    // Skipping all file conflicts must not have skipped the folder one.
    assert_eq!(ctx.asked(), 2);
    assert_eq!(f.names("dst/dir"), vec!["inner", "other"]);
}

#[test]
fn cf07_a_replace_policy_resolves_without_asking_and_is_logged() {
    let f = Fx::new();
    f.write("src/a.txt", "source");
    f.write("dst/a.txt", "target");
    let ctx = Script::new(); // any prompt would panic
    let policies = Policies { file: ConflictPolicy::Replace, ..Default::default() };
    let out = f.engine(&ctx, policies).transfer(TransferMode::Copy, &[f.p("src/a.txt")], &f.p("dst"), None).unwrap();
    assert_eq!(f.read("dst/a.txt"), "source");
    assert!(out.notes.iter().any(|n| n.starts_with("Replaced")));
    assert_eq!(f.names("dst"), vec!["a.txt"]); // no temp file left behind
}

#[test]
fn cf08_stopping_mid_replace_leaves_the_original_intact() {
    let f = Fx::new();
    f.write("src/d/one", "new 1");
    f.write("src/d/two", "new 2");
    f.write("src/d/three", "new 3");
    f.write("dst/d/original", "precious");
    let ctx = Script::answering(&[(ConflictChoice::Replace, false)]);
    // Checkpoints: counting d (1), starting d (2), first child (3). The
    // second child then finds the operation stopped.
    ctx.stop_after.store(3, Ordering::SeqCst);
    let out = f.copy(&ctx, &["src/d"], "dst");

    assert!(out.stopped);
    assert_eq!(f.names("dst"), vec!["d"], "half-built replacement must be cleaned up");
    assert_eq!(f.names("dst/d"), vec!["original"]);
    assert_eq!(f.read("dst/d/original"), "precious");
}

#[test]
fn folder_replace_removes_the_old_contents_completely() {
    let f = Fx::new();
    f.write("src/d/new", "n");
    f.write("dst/d/old", "o");
    let ctx = Script::answering(&[(ConflictChoice::Replace, false)]);
    f.copy(&ctx, &["src/d"], "dst");
    assert_eq!(f.names("dst/d"), vec!["new"]);
    assert_eq!(f.names("dst"), vec!["d"]);
}

#[test]
fn cf09_replace_takes_the_sources_capitalisation() {
    let f = Fx::new();
    f.write("src/Report.txt", "source");
    f.write("dst/report.txt", "target");
    if !f.exists("dst/REPORT.TXT") {
        return; // case-sensitive volume: these names do not conflict (CF-10)
    }
    let ctx = Script::answering(&[(ConflictChoice::Replace, false)]);
    f.copy(&ctx, &["src/Report.txt"], "dst");
    assert_eq!(f.names("dst"), vec!["Report.txt"]);
    assert_eq!(f.read("dst/Report.txt"), "source");
}

#[test]
fn nfc_and_nfd_spellings_of_a_name_conflict_locally() {
    let f = Fx::new();
    f.write("src/caf\u{e9}.txt", "nfc source");
    f.write("dst/cafe\u{301}.txt", "nfd target");
    if !f.exists("dst/caf\u{e9}.txt") {
        return; // a volume that keeps both spellings apart
    }
    let ctx = Script::answering(&[(ConflictChoice::Skip, false)]);
    let out = f.copy(&ctx, &["src/caf\u{e9}.txt"], "dst");
    assert_eq!(ctx.asked(), 1);
    assert_eq!(out.skipped, 1);
    assert_eq!(f.names("dst").len(), 1);
}

#[test]
fn file_versus_folder_conflict_can_replace_either_way() {
    let f = Fx::new();
    f.write("src/thing", "now a file");
    f.write("dst/thing/inside", "was a folder");
    f.write("src/other/inside", "now a folder");
    f.write("dst/other", "was a file");
    let ctx = Script::answering(&[(ConflictChoice::Replace, false), (ConflictChoice::Replace, false)]);
    f.copy(&ctx, &["src/thing", "src/other"], "dst");

    assert_eq!(f.read("dst/thing"), "now a file");
    assert_eq!(f.read("dst/other/inside"), "now a folder");
    assert_eq!(f.names("dst"), vec!["other", "thing"]);
    assert!(ctx.conflicts_asked.lock().unwrap().iter().all(|c| c.kind == ConflictKind::Mixed));
}

#[test]
fn cf12_stop_in_a_conflict_prompt_keeps_what_was_done() {
    let f = Fx::new();
    f.write("src/a", "a");
    f.write("src/b", "source b");
    f.write("src/c", "c");
    f.write("dst/b", "target b");
    let ctx = Script::answering(&[(ConflictChoice::Stop, false)]);
    let out = f.copy(&ctx, &["src/a", "src/b", "src/c"], "dst");

    assert!(out.stopped);
    assert_eq!(f.read("dst/a"), "a");
    assert_eq!(f.read("dst/b"), "target b");
    assert!(!f.exists("dst/c"));
}

#[test]
fn pasting_into_the_same_folder_makes_a_copy_instead_of_a_conflict() {
    let f = Fx::new();
    f.write("src/a.txt", "a");
    let ctx = Script::new();
    let out = f.copy(&ctx, &["src/a.txt"], "src");
    assert_eq!(f.names("src"), vec!["a copy.txt", "a.txt"]);
    assert_eq!(out.results, vec![f.p("src/a copy.txt")]);
}

// ------------------------------------------------------------------ Move

#[test]
fn move_on_one_volume_is_a_rename_and_keeps_everything() {
    let f = Fx::new();
    f.write("src/d/inner.txt", "i");
    f.write("src/a.txt", "a");
    f.set_mtime("src/a.txt", OLD);
    let ctx = Script::new();
    let out = f.mv(&ctx, &["src/d", "src/a.txt"], "dst");

    assert!(f.names("src").is_empty());
    assert_eq!(f.read("dst/d/inner.txt"), "i");
    assert_eq!(LocalFs.stat(&f.p("dst/a.txt")).unwrap().modified_ms, Some(OLD));
    assert_eq!(
        out.effects,
        vec![
            Effect::Moved { from: f.p("src/d"), to: f.p("dst/d") },
            Effect::Moved { from: f.p("src/a.txt"), to: f.p("dst/a.txt") },
        ]
    );
    let p = ctx.last.lock().unwrap().clone();
    assert_eq!((p.items_done, p.items_total), (2, 2)); // one step per root, no counting
}

#[test]
fn move_conflicts_replace_merge_and_keep_both() {
    let f = Fx::new();
    f.write("src/file", "source");
    f.write("dst/file", "target");
    f.write("src/dir/new", "n");
    f.write("src/dir/clash", "source clash");
    f.write("dst/dir/old", "o");
    f.write("dst/dir/clash", "target clash");
    f.write("src/both", "source both");
    f.write("dst/both", "target both");
    let ctx = Script::answering(&[
        (ConflictChoice::Replace, false),  // file
        (ConflictChoice::Merge, false),    // dir
        (ConflictChoice::Skip, false),     // dir/clash
        (ConflictChoice::KeepBoth, false), // both
    ]);
    let out = f.mv(&ctx, &["src/file", "src/dir", "src/both"], "dst");

    assert_eq!(f.read("dst/file"), "source");
    assert_eq!(f.names("dst/dir"), vec!["clash", "new", "old"]);
    assert_eq!(f.read("dst/dir/clash"), "target clash");
    assert_eq!(f.read("dst/both"), "target both");
    assert_eq!(f.read("dst/both 2"), "source both");
    // The skipped child keeps its source, and so the source folder stays.
    assert_eq!(f.names("src"), vec!["dir"]);
    assert_eq!(f.names("src/dir"), vec!["clash"]);
    assert!(out.effects.is_empty());
}

#[test]
fn moving_a_folder_over_a_folder_with_replace_swaps_it_whole() {
    let f = Fx::new();
    f.write("src/d/new", "n");
    f.write("dst/d/old", "o");
    let ctx = Script::answering(&[(ConflictChoice::Replace, false)]);
    f.mv(&ctx, &["src/d"], "dst");
    assert_eq!(f.names("dst/d"), vec!["new"]);
    assert_eq!(f.names("dst"), vec!["d"]);
    assert!(f.names("src").is_empty());
}

#[test]
fn moving_an_item_onto_itself_does_nothing() {
    let f = Fx::new();
    f.write("src/a.txt", "a");
    let ctx = Script::new();
    let out = f.mv(&ctx, &["src/a.txt"], "src");
    assert_eq!(f.names("src"), vec!["a.txt"]);
    assert_eq!((out.done, out.skipped), (0, 1));
}

/// A backend that reports every path as being on another volume, to
/// exercise the copy-then-delete path of a move on one disk.
struct OtherVolume(LocalFs);

impl Vfs for OtherVolume {
    fn id(&self) -> String {
        "other-volume".into()
    }
    fn list(&self, path: &str) -> crate::error::VfsResult<Vec<crate::entry::Entry>> {
        self.0.list(path)
    }
    fn stat(&self, path: &str) -> crate::error::VfsResult<crate::entry::Entry> {
        self.0.stat(path)
    }
    fn mkdir(&self, path: &str) -> crate::error::VfsResult<()> {
        self.0.mkdir(path)
    }
    fn rename(&self, from: &str, to: &str, replace: bool) -> crate::error::VfsResult<()> {
        self.0.rename(from, to, replace)
    }
    fn remove_file(&self, path: &str) -> crate::error::VfsResult<()> {
        if path.ends_with("undeletable") {
            return Err(VfsError::PermissionDenied(path.to_string()));
        }
        self.0.remove_file(path)
    }
    fn remove_dir(&self, path: &str) -> crate::error::VfsResult<()> {
        self.0.remove_dir(path)
    }
    fn open_read(&self, path: &str) -> crate::error::VfsResult<Box<dyn std::io::Read + Send>> {
        if path.ends_with("unreadable") {
            return Err(VfsError::PermissionDenied(path.to_string()));
        }
        self.0.open_read(path)
    }
    fn create(&self, path: &str) -> crate::error::VfsResult<Box<dyn std::io::Write + Send>> {
        self.0.create(path)
    }
    fn set_times(&self, path: &str, m: Option<i64>, c: Option<i64>) -> crate::error::VfsResult<()> {
        self.0.set_times(path, m, c)
    }
    fn set_mode(&self, path: &str, mode: u32) -> crate::error::VfsResult<()> {
        self.0.set_mode(path, mode)
    }
    fn symlink(&self, target: &str, link: &str) -> crate::error::VfsResult<()> {
        self.0.symlink(target, link)
    }
    fn same_volume(&self, _a: &str, _b: &str) -> bool {
        false
    }
    fn same_item(&self, a: &str, b: &str) -> bool {
        self.0.same_item(a, b)
    }
}

/// Engine whose source and target are different backends: no rename, no
/// fast copy, bytes are streamed.
fn cross<'a>(ctx: &'a Script, policies: Policies) -> Engine<'a> {
    Engine::new(ctx, Arc::new(OtherVolume(LocalFs)), Arc::new(OtherVolume(LocalFs)), false, policies)
}

#[test]
fn move_across_volumes_copies_then_deletes_the_source() {
    let f = Fx::new();
    f.write("src/d/one", "1");
    f.write("src/d/sub/two", "22");
    symlink("one", f.tmp.path().join("src/d/link")).unwrap();
    f.set_mtime("src/d/one", OLD);
    let ctx = Script::new();
    let out =
        cross(&ctx, Policies::default()).transfer(TransferMode::Move, &[f.p("src/d")], &f.p("dst"), None).unwrap();

    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert!(f.names("src").is_empty());
    assert_eq!(f.read("dst/d/sub/two"), "22");
    assert_eq!(fs::read_link(f.tmp.path().join("dst/d/link")).unwrap(), Path::new("one"));
    // Streamed copies still carry the modification date across.
    assert_eq!(LocalFs.stat(&f.p("dst/d/one")).unwrap().modified_ms, Some(OLD));
    let p = ctx.last.lock().unwrap().clone();
    assert_eq!((p.items_done, p.items_total, p.bytes_done, p.bytes_total), (5, 5, 3, 3));
}

#[test]
fn a_failed_item_keeps_its_source_when_moving_across_volumes() {
    let f = Fx::new();
    f.write("src/d/fine", "ok");
    f.write("src/d/unreadable", "secret");
    let ctx = Script::new();
    ctx.error_answers.lock().unwrap().push_back(ErrorChoice::Skip);
    let out =
        cross(&ctx, Policies::default()).transfer(TransferMode::Move, &[f.p("src/d")], &f.p("dst"), None).unwrap();

    assert_eq!(out.failures.len(), 1);
    assert_eq!(f.read("dst/d/fine"), "ok");
    assert!(!f.exists("dst/d/unreadable"), "no partial file may remain");
    // The failed file and its folder are still at the source; the rest moved.
    assert_eq!(f.names("src/d"), vec!["unreadable"]);
}

// ------------------------------------------------------ Errors and stop

#[test]
fn error_prompt_retry_skip_all_and_stop() {
    let f = Fx::new();
    for name in ["a-unreadable", "b-unreadable", "c-fine"] {
        f.write(&format!("src/{name}"), name);
    }
    let sources = [f.p("src/a-unreadable"), f.p("src/b-unreadable"), f.p("src/c-fine")];

    // Retry once, then Skip All: the second failure is not asked about.
    let ctx = Script::new();
    ctx.error_answers.lock().unwrap().extend([ErrorChoice::Retry, ErrorChoice::SkipAll]);
    let out = cross(&ctx, Policies::default()).transfer(TransferMode::Copy, &sources, &f.p("dst"), None).unwrap();
    assert_eq!(ctx.errors_asked.lock().unwrap().len(), 2);
    assert_eq!(out.failures.len(), 2);
    assert_eq!(f.names("dst"), vec!["c-fine"]);
    assert!(!out.stopped);

    // Stop ends the operation at the first failure.
    fs::remove_file(f.tmp.path().join("dst/c-fine")).unwrap();
    let ctx = Script::new();
    ctx.error_answers.lock().unwrap().push_back(ErrorChoice::Stop);
    let out = cross(&ctx, Policies::default()).transfer(TransferMode::Copy, &sources, &f.p("dst"), None).unwrap();
    assert!(out.stopped);
    assert!(f.names("dst").is_empty());
}

#[test]
fn error_policies_skip_and_stop_never_ask() {
    let f = Fx::new();
    f.write("src/a-unreadable", "x");
    f.write("src/b-fine", "y");
    let sources = [f.p("src/a-unreadable"), f.p("src/b-fine")];
    let ctx = Script::new(); // a prompt would panic
    let skip = Policies { error: ErrorPolicy::Skip, ..Default::default() };
    let out = cross(&ctx, skip).transfer(TransferMode::Copy, &sources, &f.p("dst"), None).unwrap();
    assert_eq!((out.failures.len(), out.stopped), (1, false));
    assert_eq!(f.names("dst"), vec!["b-fine"]);

    fs::remove_file(f.tmp.path().join("dst/b-fine")).unwrap();
    let stop = Policies { error: ErrorPolicy::Stop, ..Default::default() };
    let out = cross(&ctx, stop).transfer(TransferMode::Copy, &sources, &f.p("dst"), None).unwrap();
    assert!(out.stopped);
    assert!(f.names("dst").is_empty());
}

#[test]
fn stopping_during_preparation_touches_nothing() {
    let f = Fx::new();
    f.write("src/d/a", "a");
    f.write("src/d/b", "b");
    let ctx = Script::new();
    ctx.stop_after.store(0, Ordering::SeqCst);
    let out = f.copy(&ctx, &["src/d"], "dst");
    assert!(out.stopped);
    assert!(f.names("dst").is_empty());
}

#[test]
fn stopping_a_streamed_copy_removes_the_partial_file() {
    let f = Fx::new();
    f.write("src/big", &"x".repeat(3 * 1024 * 1024));
    let ctx = Script::new();
    ctx.stop_after.store(3, Ordering::SeqCst); // measure, item, then one chunk in
    let out =
        cross(&ctx, Policies::default()).transfer(TransferMode::Copy, &[f.p("src/big")], &f.p("dst"), None).unwrap();
    assert!(out.stopped);
    assert!(f.names("dst").is_empty());
    assert!(f.exists("src/big"));
}

// ---------------------------------------------------- Duplicate, delete

#[test]
fn fo12_duplicate_numbers_its_copies() {
    let f = Fx::new();
    f.write("src/photo.jpg", "p");
    f.write("src/dir.v2/inner", "i");
    let ctx = Script::new();
    let first = f.engine(&ctx, Policies::default()).duplicate(&[f.p("src/photo.jpg"), f.p("src/dir.v2")]);
    let second = f.engine(&ctx, Policies::default()).duplicate(&[f.p("src/photo.jpg")]);

    assert_eq!(f.names("src"), vec!["dir.v2", "dir.v2 copy", "photo copy 2.jpg", "photo copy.jpg", "photo.jpg"]);
    assert_eq!(f.read("src/dir.v2 copy/inner"), "i");
    assert_eq!(first.results, vec![f.p("src/photo copy.jpg"), f.p("src/dir.v2 copy")]);
    assert_eq!(second.effects, vec![Effect::Created { path: f.p("src/photo copy 2.jpg") }]);
    assert_eq!(ctx.asked(), 0);
}

#[test]
fn delete_removes_trees_but_never_follows_symlinks() {
    let f = Fx::new();
    f.write("src/keep/precious", "p");
    f.write("src/d/sub/x", "x");
    symlink(f.p("src/keep"), f.tmp.path().join("src/d/link-to-keep")).unwrap();
    symlink(".", f.tmp.path().join("src/d/loop")).unwrap();
    let ctx = Script::new();
    let out = f.engine(&ctx, Policies::default()).delete(&[f.p("src/d")]);

    assert!(!f.exists("src/d"));
    assert_eq!(f.read("src/keep/precious"), "p");
    assert_eq!(out.done, 5); // d, sub, x, link, loop
    assert!(out.effects.is_empty() && !out.undoable);
    let p = ctx.last.lock().unwrap().clone();
    assert_eq!(p.items_done, p.items_total);
}

#[test]
fn delete_keeps_a_folder_whose_child_could_not_be_removed() {
    let f = Fx::new();
    f.write("src/d/fine", "f");
    f.write("src/d/undeletable", "u");
    let ctx = Script::new();
    ctx.error_answers.lock().unwrap().push_back(ErrorChoice::Skip);
    let fs_: Arc<dyn Vfs> = Arc::new(OtherVolume(LocalFs));
    let out = Engine::new(&ctx, Arc::clone(&fs_), fs_, true, Policies::default()).delete(&[f.p("src/d")]);

    assert_eq!(f.names("src/d"), vec!["undeletable"]);
    assert_eq!(out.failures.len(), 1);
    // Exactly one prompt: the folder is not reported as a second failure.
    assert_eq!(ctx.errors_asked.lock().unwrap().len(), 1);
}

#[test]
fn copy_to_as_gives_a_single_item_a_new_name() {
    let f = Fx::new();
    f.write("src/a.txt", "a");
    f.write("src/b.txt", "b");
    let ctx = Script::new();
    let out = f
        .engine(&ctx, Policies::default())
        .transfer(TransferMode::Copy, &[f.p("src/a.txt")], &f.p("dst"), Some("renamed.txt"))
        .unwrap();
    assert_eq!(f.read("dst/renamed.txt"), "a");
    assert_eq!(out.results, vec![f.p("dst/renamed.txt")]);
    assert!(f
        .engine(&ctx, Policies::default())
        .transfer(TransferMode::Copy, &[f.p("src/a.txt"), f.p("src/b.txt")], &f.p("dst"), Some("x"))
        .is_err());
}

#[test]
fn pv07_apply_to_enclosed_items_sets_every_subitem() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fx::new();
    f.write("src/F/a", "a");
    f.write("src/F/sub/b", "b");
    f.write("src/outside", "o");
    fs::set_permissions(f.tmp.path().join("src/outside"), fs::Permissions::from_mode(0o600)).unwrap();
    symlink(f.p("src/outside"), f.tmp.path().join("src/F/link")).unwrap();
    let mode = |rel: &str| fs::metadata(f.tmp.path().join(rel)).unwrap().permissions().mode() & 0o7777;

    let ctx = Script::new();
    let out = f.engine(&ctx, Policies::default()).set_mode(&[f.p("src/F")], 0o755, true);
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    for rel in ["src/F", "src/F/a", "src/F/sub", "src/F/sub/b"] {
        assert_eq!(mode(rel), 0o755, "{rel}");
    }
    // The link's target is outside the folder and must be untouched.
    assert_eq!(mode("src/outside"), 0o600);
    assert_eq!(out.done, 4);
    let p = ctx.last.lock().unwrap().clone();
    assert_eq!(p.items_done, p.items_total);

    // Without "enclosed items" only the item itself changes.
    let out = f.engine(&ctx, Policies::default()).set_mode(&[f.p("src/F")], 0o700, false);
    assert_eq!((out.done, mode("src/F"), mode("src/F/a")), (1, 0o700, 0o755));
    assert!(!out.undoable);
}

// ------------------------------------------------- Administrator rights

/// A folder this user may not change, made writable again when dropped
/// so the temp dir can be cleaned up.
struct Locked(std::path::PathBuf);

impl Locked {
    fn new(path: std::path::PathBuf) -> Self {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o555)).unwrap();
        Self(path)
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

#[test]
fn a_permission_problem_can_be_retried_as_administrator_once_for_all() {
    let f = Fx::new();
    f.write("src/locked/a", "a");
    f.write("src/locked/b", "b");
    let _lock = Locked::new(f.tmp.path().join("src/locked"));
    let ctx = Script::new();
    ctx.error_answers.lock().unwrap().push_back(ErrorChoice::Elevate);
    let out = f.engine(&ctx, Policies::default()).delete(&[f.p("src/locked/a"), f.p("src/locked/b")]);
    let asked = ctx.errors_asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1, "only the first failure asks");
    assert!(asked[0].can_elevate);
    assert_eq!(out.done, 2);
    assert!(out.failures.is_empty(), "{:?}", out.failures);
    assert_eq!(ctx.elevated.lock().unwrap().len(), 2);
    assert!(f.names("src/locked").is_empty());
}

#[test]
fn declining_the_password_is_a_failure() {
    let f = Fx::new();
    f.write("src/locked/a", "a");
    let _lock = Locked::new(f.tmp.path().join("src/locked"));
    let ctx = Script { refuse_password: true, ..Script::new() };
    ctx.error_answers.lock().unwrap().push_back(ErrorChoice::Elevate);
    let out = f.engine(&ctx, Policies::default()).delete(&[f.p("src/locked/a")]);
    assert_eq!(out.done, 0);
    assert_eq!(out.failures.len(), 1);
    assert!(out.failures[0].1.contains("Not done as administrator"), "{:?}", out.failures);
    assert!(f.exists("src/locked/a"));
}

#[test]
fn moves_and_copies_into_a_locked_folder_can_be_elevated() {
    let f = Fx::new();
    f.write("src/m", "move me");
    f.write("src/c", "copy me");
    fs::create_dir(f.tmp.path().join("dst/locked")).unwrap();
    let _lock = Locked::new(f.tmp.path().join("dst/locked"));
    let ctx = Script::new();
    ctx.error_answers.lock().unwrap().push_back(ErrorChoice::Elevate);
    let out = f.mv(&ctx, &["src/m"], "dst/locked");
    assert_eq!(out.done, 1, "{:?}", out.failures);
    assert_eq!(f.read("dst/locked/m"), "move me");
    assert!(matches!(ctx.elevated.lock().unwrap()[0], Privileged::Move { .. }));

    let ctx = Script::new();
    ctx.error_answers.lock().unwrap().push_back(ErrorChoice::Elevate);
    let out = f.copy(&ctx, &["src/c"], "dst/locked");
    assert_eq!(out.done, 1, "{:?}", out.failures);
    assert_eq!(f.read("dst/locked/c"), "copy me");
    assert_eq!(f.read("src/c"), "copy me");
}

#[test]
fn other_failures_and_other_backends_never_offer_elevation() {
    let f = Fx::new();
    f.write("src/a-unreadable", "x");
    let ctx = Script::new();
    ctx.error_answers.lock().unwrap().push_back(ErrorChoice::Skip);
    // The stand-in remote backend is not this Mac's disk.
    cross(&ctx, Policies::default())
        .transfer(TransferMode::Copy, &[f.p("src/a-unreadable")], &f.p("dst"), None)
        .unwrap();
    assert!(!ctx.errors_asked.lock().unwrap()[0].can_elevate);
    assert!(ctx.elevated.lock().unwrap().is_empty());
}
