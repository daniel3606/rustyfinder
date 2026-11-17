//! The activity manager end to end, against the local file system.

use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::activity::*;
use super::engine::{ConflictAnswer, ConflictChoice, Effect, ErrorChoice, TransferMode};
use crate::entry::Entry;
use crate::error::VfsResult;
use crate::local::LocalFs;
use crate::registry::{VfsRegistry, LOCAL};
use crate::store::Store;
use crate::vfs::Vfs;

enum Event {
    Prompt(ActivityId, Prompt),
    Finished(FinishReport),
}

struct Observer {
    tx: Mutex<Sender<Event>>,
    lists: Mutex<Vec<Vec<ActivityInfo>>>,
}

impl ActivityObserver for Observer {
    fn activities_changed(&self, activities: Vec<ActivityInfo>) {
        self.lists.lock().unwrap().push(activities);
    }
    fn prompt(&self, activity: ActivityId, prompt: Prompt) {
        let _ = self.tx.lock().unwrap().send(Event::Prompt(activity, prompt));
    }
    fn finished(&self, report: FinishReport) {
        let _ = self.tx.lock().unwrap().send(Event::Finished(report));
    }
}

/// Stand-in Trash: a folder beside the fixture.
struct FakeTrash(PathBuf);

impl Platform for FakeTrash {
    fn trash(&self, path: &str) -> Result<String, String> {
        if path.ends_with("untrashable") {
            return Err("The item can’t be moved to the Trash.".into());
        }
        let name = crate::paths::file_name(path);
        let mut target = self.0.join(name);
        let mut n = 1;
        while target.exists() {
            n += 1;
            target = self.0.join(format!("{name} {n}"));
        }
        fs::rename(path, &target).map_err(|e| e.to_string())?;
        Ok(target.to_str().unwrap().to_string())
    }

    /// A stand-in alias: a file naming its target.
    fn make_alias(&self, target: &str, alias: &str) -> Result<(), String> {
        if target.ends_with("unaliasable") {
            return Err("No alias for this one.".into());
        }
        fs::write(alias, format!("alias to {target}")).map_err(|e| e.to_string())
    }

    /// Stand-in tags and comments: kept in a file beside the item.
    fn set_tags(&self, path: &str, tags: &[String]) -> Result<(), String> {
        fs::write(format!("{path}.tags"), tags.join(",")).map_err(|e| e.to_string())
    }

    fn set_comment(&self, path: &str, comment: &str) -> Result<(), String> {
        fs::write(format!("{path}.comment"), comment).map_err(|e| e.to_string())
    }

    /// Plays the administrator for a Trash step: opens the folder, moves
    /// the item, closes the folder again.
    fn elevate(&self, action: &super::privileged::Privileged) -> Result<String, String> {
        use std::os::unix::fs::PermissionsExt;
        let super::privileged::Privileged::Trash { path } = action else {
            return Err("unexpected".into());
        };
        if path.contains("refused") {
            return Err("User canceled.".into());
        }
        let dir = std::path::Path::new(path).parent().unwrap().to_path_buf();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let result = self.trash(path);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        result
    }
}

/// Local disk whose copies block until released, to hold activities in
/// the Running state for as long as a test needs.
struct Gated {
    gate: Mutex<Option<Receiver<()>>>,
}

impl Vfs for Gated {
    fn id(&self) -> String {
        "gated".into()
    }
    fn list(&self, path: &str) -> VfsResult<Vec<Entry>> {
        LocalFs.list(path)
    }
    fn stat(&self, path: &str) -> VfsResult<Entry> {
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
    fn same_volume(&self, a: &str, b: &str) -> bool {
        LocalFs.same_volume(a, b)
    }
    fn same_item(&self, a: &str, b: &str) -> bool {
        LocalFs.same_item(a, b)
    }
    fn copy_within(&self, from: &str, to: &str, progress: &mut dyn FnMut(u64) -> bool) -> VfsResult<bool> {
        // Each copy waits for one token.
        if let Some(gate) = self.gate.lock().unwrap().as_ref() {
            let _ = gate.recv_timeout(Duration::from_secs(10));
        }
        LocalFs.copy_within(from, to, progress)
    }
}

struct Fx {
    tmp: tempfile::TempDir,
    mgr: ActivityManager,
    rx: Receiver<Event>,
    observer: Arc<Observer>,
    store: Arc<Store>,
    registry: Arc<VfsRegistry>,
}

impl Fx {
    fn new() -> Self {
        Self::with_vfs(Arc::new(LocalFs))
    }

    fn with_vfs(vfs: Arc<dyn Vfs>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        for d in ["src", "dst", "trash", "support"] {
            fs::create_dir(tmp.path().join(d)).unwrap();
        }
        let store = Arc::new(Store::open(tmp.path().join("support"), tmp.path().to_str().unwrap()));
        let (tx, rx) = channel();
        let observer = Arc::new(Observer { tx: Mutex::new(tx), lists: Mutex::new(Vec::new()) });
        let registry = Arc::new(VfsRegistry::new(vfs, tmp.path().to_str().unwrap()));
        let mgr = ActivityManager::new(
            Arc::clone(&registry),
            Arc::clone(&store),
            Arc::new(FakeTrash(tmp.path().join("trash"))),
            Arc::clone(&observer) as Arc<dyn ActivityObserver>,
        );
        Self { tmp, mgr, rx, observer, store, registry }
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

    fn names(&self, rel: &str) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.tmp.path().join(rel))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn next(&self) -> Event {
        self.rx.recv_timeout(Duration::from_secs(10)).expect("no event from the activity")
    }

    fn finished(&self) -> FinishReport {
        loop {
            if let Event::Finished(report) = self.next() {
                return report;
            }
        }
    }

    fn copy(&self, sources: &[&str], dest: &str) -> ActivityId {
        self.mgr.submit(
            1,
            Request::Transfer {
                mode: TransferMode::Copy,
                sources: sources.iter().map(|s| self.p(s)).collect(),
                dest_dir: self.p(dest),
                rename_to: None,
            },
        )
    }

    /// Wait until `check` holds for the activity list.
    fn wait_for(&self, what: &str, check: impl Fn(&[ActivityInfo]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if check(&self.mgr.activities()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for: {what}; activities: {:#?}", self.mgr.activities());
    }
}

fn states(list: &[ActivityInfo]) -> Vec<ActivityState> {
    list.iter().map(|a| a.state).collect()
}

#[test]
fn copy_runs_reports_and_logs() {
    let f = Fx::new();
    f.write("src/a.txt", "hello");
    let id = f.copy(&["src/a.txt"], "dst");
    let report = f.finished();

    assert_eq!(report.id, id);
    assert_eq!(report.window, 1);
    assert_eq!(report.title, "Copying “a.txt”");
    assert_eq!(report.result, ActivityResult::Success);
    assert_eq!(report.results, vec![f.p("dst/a.txt")]);
    assert_eq!(f.read("dst/a.txt"), "hello");
    assert!(f.mgr.activities().is_empty(), "finished activities leave the list");
    assert!(!f.mgr.has_active(None));

    let log = f.mgr.log();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].title, "Copied 1 item");
    assert_eq!(log[0].result, ActivityResult::Success);
    assert!(log[0].time_ms > 1_600_000_000_000);
    // The log survives a relaunch.
    assert_eq!(f.store.load_log(), log);
    // The list was announced while it ran and again once it emptied.
    let lists = f.observer.lists.lock().unwrap();
    assert!(lists.iter().any(|l| l.len() == 1));
    assert!(lists.last().unwrap().is_empty());
}

#[test]
fn titles_name_one_item_or_count_several() {
    let f = Fx::new();
    for n in ["a", "b", "c"] {
        f.write(&format!("src/{n}"), n);
    }
    f.copy(&["src/a", "src/b", "src/c"], "dst");
    assert_eq!(f.finished().title, "Copying 3 items");
    f.mgr.submit(1, Request::Trash { paths: vec![f.p("src/a")] });
    assert_eq!(f.finished().title, "Moving “a” to the Trash");
    f.mgr.submit(1, Request::Delete { paths: vec![f.p("src/b"), f.p("src/c")] });
    assert_eq!(f.finished().title, "Deleting 2 items");
    assert_eq!(f.mgr.log().last().unwrap().title, "Deleted 2 items");
}

#[test]
fn a_refused_operation_fails_with_a_reason_and_is_logged_as_failed() {
    let f = Fx::new();
    f.write("src/d/x", "x");
    f.copy(&["src/d"], "src/d");
    let report = f.finished();
    assert_eq!(report.result, ActivityResult::Failed);
    assert!(report.message.as_deref().unwrap().contains("into itself"));
    let entry = f.mgr.log().pop().unwrap();
    assert_eq!(entry.result, ActivityResult::Failed);
    assert_eq!(entry.title, "Copying “d”"); // never claims "Copied"
    assert!(f.mgr.undo_title(1).is_none());
}

#[test]
fn conflict_prompt_blocks_until_answered() {
    let f = Fx::new();
    f.write("src/a.txt", "source");
    f.write("dst/a.txt", "target");
    let id = f.copy(&["src/a.txt"], "dst");

    let Event::Prompt(asked, Prompt::Conflict(info)) = f.next() else {
        panic!("expected a conflict prompt");
    };
    assert_eq!(asked, id);
    assert_eq!(info.target_path, f.p("dst/a.txt"));
    assert!(f.mgr.activities()[0].waiting_for_user);
    assert_eq!(f.read("dst/a.txt"), "target", "nothing happens before the answer");

    f.mgr.answer_conflict(id, ConflictAnswer { choice: ConflictChoice::Replace, apply_to_all: false });
    assert_eq!(f.finished().result, ActivityResult::Success);
    assert_eq!(f.read("dst/a.txt"), "source");
    assert!(f.mgr.log().last().unwrap().lines.iter().any(|l| l.starts_with("Replaced")));
}

#[test]
fn stopping_an_activity_waiting_at_a_prompt_ends_it() {
    let f = Fx::new();
    f.write("src/a.txt", "source");
    f.write("dst/a.txt", "target");
    let id = f.copy(&["src/a.txt"], "dst");
    assert!(matches!(f.next(), Event::Prompt(..)));
    f.mgr.stop(id);
    let report = f.finished();
    assert_eq!(report.result, ActivityResult::Stopped);
    assert_eq!(f.read("dst/a.txt"), "target");
    assert_eq!(f.mgr.log().last().unwrap().result, ActivityResult::Stopped);
}

#[test]
fn fo16_skipping_a_failed_item_is_partial_and_names_the_failure() {
    let f = Fx::new();
    f.write("src/fine", "f");
    f.write("src/untrashable", "u");
    f.mgr.submit(1, Request::Trash { paths: vec![f.p("src/fine"), f.p("src/untrashable")] });
    let report = f.finished();
    assert_eq!(report.result, ActivityResult::Partial);
    assert!(report.message.as_deref().unwrap().contains("untrashable"));
    let entry = f.mgr.log().pop().unwrap();
    assert_eq!(entry.result, ActivityResult::Partial);
    assert!(entry.lines.iter().any(|l| l.starts_with("Failed:") && l.contains("untrashable")));
    assert_eq!(entry.title, "Moved 1 item to the Trash");
}

#[test]
fn fo06_fo07_queue_limits_concurrency_and_can_be_reordered() {
    let (release, gate) = channel();
    let f = Fx::with_vfs(Arc::new(Gated { gate: Mutex::new(Some(gate)) }));
    f.store.update_settings(|s| s.max_transfers = 2);
    let ids: Vec<ActivityId> = (0..5)
        .map(|i| {
            f.write(&format!("src/f{i}"), "x");
            f.copy(&[&format!("src/f{i}")], "dst")
        })
        .collect();

    f.wait_for("two running, three queued", |list| {
        list.len() == 5
            && list.iter().filter(|a| a.state != ActivityState::Queued).count() == 2
            && states(&list[2..]) == vec![ActivityState::Queued; 3]
    });

    // Drag the last queued one to the front of the queue.
    f.mgr.reorder(ids[4], 0);
    let order: Vec<ActivityId> = f.mgr.activities().iter().map(|a| a.id).collect();
    assert_eq!(order, vec![ids[0], ids[1], ids[4], ids[2], ids[3]]);

    // One finishes: exactly one queued activity starts, and it is the
    // one that was moved to the front.
    release.send(()).unwrap();
    f.finished();
    f.wait_for("the reordered one started", |list| {
        list.len() == 4
            && list.iter().filter(|a| a.state != ActivityState::Queued).count() == 2
            && list.iter().any(|a| a.id == ids[4] && a.state != ActivityState::Queued)
    });

    // A queued activity can be stopped before it ever runs.
    f.mgr.stop(ids[3]);
    assert!(!f.exists("dst/f3"));
    for _ in 0..4 {
        release.send(()).unwrap();
    }
    f.wait_for("everything drained", |list| list.is_empty());
    assert_eq!(f.names("dst"), vec!["f0", "f1", "f2", "f4"]);
}

#[test]
fn fo08_a_same_volume_move_does_not_wait_in_the_queue() {
    let (release, gate) = channel();
    let f = Fx::with_vfs(Arc::new(Gated { gate: Mutex::new(Some(gate)) }));
    f.store.update_settings(|s| s.max_transfers = 1);
    f.write("src/big", "x");
    f.write("src/waiting", "x");
    f.write("src/quick", "q");
    f.copy(&["src/big"], "dst");
    f.copy(&["src/waiting"], "dst");
    f.wait_for("one running, one queued", |l| l.len() == 2 && l[1].state == ActivityState::Queued);

    f.mgr.submit(
        1,
        Request::Transfer {
            mode: TransferMode::Move,
            sources: vec![f.p("src/quick")],
            dest_dir: f.p("dst"),
            rename_to: None,
        },
    );
    let report = f.finished();
    assert_eq!(report.title, "Moving “quick”");
    assert_eq!(f.read("dst/quick"), "q");
    assert_eq!(f.mgr.activities().len(), 2, "the copies are still waiting");
    release.send(()).unwrap();
    release.send(()).unwrap();
    f.wait_for("drained", |l| l.is_empty());
}

#[test]
fn sd02_lowering_the_limit_applies_from_the_next_start() {
    let (release, gate) = channel();
    let f = Fx::with_vfs(Arc::new(Gated { gate: Mutex::new(Some(gate)) }));
    for i in 0..4 {
        f.write(&format!("src/f{i}"), "x");
        f.copy(&[&format!("src/f{i}")], "dst");
    }
    f.wait_for("three running", |l| l.iter().filter(|a| a.state != ActivityState::Queued).count() == 3);
    f.store.update_settings(|s| s.max_transfers = 1);
    // Finishing one of three leaves two running: over the new limit, so
    // the queued one must keep waiting.
    release.send(()).unwrap();
    f.finished();
    f.wait_for("still queued", |l| l.len() == 3 && l.iter().filter(|a| a.state == ActivityState::Queued).count() == 1);
    for _ in 0..3 {
        release.send(()).unwrap();
    }
    f.wait_for("drained", |l| l.is_empty());
}

#[test]
fn pause_holds_the_activity_and_resume_continues_it() {
    let (release, gate) = channel();
    let f = Fx::with_vfs(Arc::new(Gated { gate: Mutex::new(Some(gate)) }));
    f.write("src/a", "a");
    f.write("src/b", "b");
    let id = f.copy(&["src/a", "src/b"], "dst");
    f.wait_for("running", |l| l.len() == 1 && l[0].state != ActivityState::Queued);
    f.mgr.pause(id);
    assert_eq!(f.mgr.activities()[0].state, ActivityState::Paused);

    release.send(()).unwrap(); // lets the first file through
    release.send(()).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!f.exists("dst/b"), "a paused activity must not move on to the next item");
    assert_eq!(f.mgr.activities()[0].state, ActivityState::Paused);

    f.mgr.resume(id);
    assert_eq!(f.finished().result, ActivityResult::Success);
    assert_eq!(f.names("dst"), vec!["a", "b"]);
}

#[test]
fn stop_all_only_touches_its_own_window() {
    let (release, gate) = channel();
    let f = Fx::with_vfs(Arc::new(Gated { gate: Mutex::new(Some(gate)) }));
    f.write("src/a", "a");
    f.write("src/b", "b");
    f.copy(&["src/a"], "dst");
    let other = f.mgr.submit(
        2,
        Request::Transfer {
            mode: TransferMode::Copy,
            sources: vec![f.p("src/b")],
            dest_dir: f.p("dst"),
            rename_to: None,
        },
    );
    f.wait_for("both running", |l| l.len() == 2 && l.iter().all(|a| a.state != ActivityState::Queued));
    assert!(f.mgr.has_active(Some(1)) && f.mgr.has_active(Some(2)));
    f.mgr.stop_all(1);
    release.send(()).unwrap();
    release.send(()).unwrap();
    f.wait_for("drained", |l| l.is_empty());
    assert!(f.exists("dst/b"));
    let _ = other;
}

// ------------------------------------------------------------ Undo / redo

fn drain(f: &Fx) -> FinishReport {
    let report = f.finished();
    f.wait_for("idle", |l| l.is_empty());
    report
}

#[test]
fn fo10_trash_then_undo_puts_the_item_back_and_redo_trashes_it_again() {
    let f = Fx::new();
    f.write("src/f", "content");
    assert!(f.mgr.undo_title(1).is_none());
    f.mgr.submit(1, Request::Trash { paths: vec![f.p("src/f")] });
    drain(&f);
    assert!(!f.exists("src/f"));
    assert_eq!(f.names("trash"), vec!["f"]);
    assert_eq!(f.mgr.undo_title(1).as_deref(), Some("Undo Move to Trash"));

    f.mgr.undo(1).unwrap();
    let report = drain(&f);
    assert_eq!(report.title, "Undo Move to Trash of “f”");
    assert_eq!(report.results, vec![f.p("src/f")]);
    assert_eq!(f.read("src/f"), "content");
    assert!(f.names("trash").is_empty());
    assert!(f.mgr.undo_title(1).is_none());
    assert_eq!(f.mgr.redo_title(1).as_deref(), Some("Redo Move to Trash"));
    // The undo is logged as such.
    assert_eq!(f.mgr.log().last().unwrap().title, "Undo Move to Trash of “f”");

    f.mgr.redo(1).unwrap();
    drain(&f);
    assert!(!f.exists("src/f"));
    assert_eq!(f.mgr.log().last().unwrap().title, "Redo Move to Trash of “f”");
    assert_eq!(f.mgr.undo_title(1).as_deref(), Some("Undo Move to Trash"));
    assert!(f.mgr.redo_title(1).is_none());
}

#[test]
fn fo19_rename_is_logged_with_both_names_and_can_be_undone() {
    let f = Fx::new();
    f.write("src/a/inner", "i");
    f.mgr.submit(1, Request::Rename { path: f.p("src/a"), new_name: "b".into() });
    let report = drain(&f);
    assert_eq!(report.results, vec![f.p("src/b")]);
    assert_eq!(f.names("src"), vec!["b"]);
    assert!(f.mgr.log().last().unwrap().lines.contains(&"a → b".to_string()));

    f.mgr.undo(1).unwrap();
    drain(&f);
    assert_eq!(f.names("src"), vec!["a"]);
    assert_eq!(f.read("src/a/inner"), "i");
    assert!(f.mgr.log().last().unwrap().title.contains("Rename"));
}

#[test]
fn undoing_a_case_only_rename_restores_the_spelling() {
    let f = Fx::new();
    f.write("src/photo.jpg", "p");
    f.mgr.submit(1, Request::Rename { path: f.p("src/photo.jpg"), new_name: "Photo.jpg".into() });
    drain(&f);
    assert_eq!(f.names("src"), vec!["Photo.jpg"]);
    f.mgr.undo(1).unwrap();
    drain(&f);
    assert_eq!(f.names("src"), vec!["photo.jpg"]);
}

#[test]
fn a_failed_rename_is_reported_and_not_undoable() {
    let f = Fx::new();
    f.write("src/a", "a");
    f.write("src/b", "b");
    f.mgr.submit(1, Request::Rename { path: f.p("src/a"), new_name: "b".into() });
    let report = drain(&f);
    assert_eq!(report.result, ActivityResult::Failed);
    assert!(report.message.as_deref().unwrap().contains("already exists"));
    assert!(f.mgr.undo_title(1).is_none());
    assert_eq!(f.mgr.log().last().unwrap().result, ActivityResult::Failed);
    assert_eq!(f.read("src/b"), "b");
}

#[test]
fn copy_and_move_undo_and_redo() {
    let f = Fx::new();
    f.write("src/a", "a");
    f.copy(&["src/a"], "dst");
    drain(&f);
    f.mgr.undo(1).unwrap(); // the copy goes to the Trash, the source stays
    drain(&f);
    assert!(!f.exists("dst/a") && f.exists("src/a"));
    assert_eq!(f.names("trash"), vec!["a"]);
    f.mgr.redo(1).unwrap(); // and comes back out of it
    drain(&f);
    assert_eq!(f.read("dst/a"), "a");
    assert!(f.names("trash").is_empty());

    f.mgr.submit(
        1,
        Request::Transfer {
            mode: TransferMode::Move,
            sources: vec![f.p("src/a")],
            dest_dir: f.p("dst/"),
            rename_to: Some("moved".into()),
        },
    );
    drain(&f);
    assert!(!f.exists("src/a"));
    f.mgr.undo(1).unwrap();
    drain(&f);
    assert_eq!(f.read("src/a"), "a");
    assert!(!f.exists("dst/moved"));
}

#[test]
fn fo13_new_folder_twice_then_undo() {
    let f = Fx::new();
    f.mgr.submit(1, Request::NewFolder { dir: f.p("src") });
    let first = drain(&f);
    f.mgr.submit(1, Request::NewFolder { dir: f.p("src") });
    let second = drain(&f);
    assert_eq!(f.names("src"), vec!["untitled folder", "untitled folder 2"]);
    assert!(first.rename_after && second.rename_after);
    assert_eq!(second.results, vec![f.p("src/untitled folder 2")]);

    f.mgr.undo(1).unwrap();
    drain(&f);
    assert_eq!(f.names("src"), vec!["untitled folder"]);
    assert_eq!(f.mgr.undo_title(1).as_deref(), Some("Undo New Folder"));
}

#[test]
fn new_folder_with_selection_moves_items_in_and_undo_moves_them_out() {
    let f = Fx::new();
    f.write("src/a", "a");
    f.write("src/b", "b");
    f.mgr.submit(1, Request::NewFolderWithSelection { dir: f.p("src"), paths: vec![f.p("src/a"), f.p("src/b")] });
    let report = drain(&f);
    assert_eq!(report.results, vec![f.p("src/New Folder With Items")]);
    assert!(report.rename_after);
    assert_eq!(f.names("src"), vec!["New Folder With Items"]);
    assert_eq!(f.names("src/New Folder With Items"), vec!["a", "b"]);

    f.mgr.undo(1).unwrap();
    drain(&f);
    assert_eq!(f.names("src"), vec!["a", "b"]);
    assert_eq!(f.names("trash"), vec!["New Folder With Items"]);
}

#[test]
fn fo09_permanent_delete_is_not_undoable_and_leaves_earlier_undo_alone() {
    let f = Fx::new();
    f.write("src/keep", "k");
    f.write("src/gone", "g");
    f.mgr.submit(1, Request::Trash { paths: vec![f.p("src/keep")] });
    drain(&f);
    f.mgr.submit(1, Request::Delete { paths: vec![f.p("src/gone")] });
    drain(&f);
    assert!(!f.exists("src/gone"));
    assert!(f.names("trash").iter().all(|n| n != "gone"));
    // Undo skips past the delete to the last undoable operation.
    assert_eq!(f.mgr.undo_title(1).as_deref(), Some("Undo Move to Trash"));
}

#[test]
fn a_new_operation_clears_redo_and_undo_stacks_are_per_window() {
    let f = Fx::new();
    f.write("src/a", "a");
    f.write("src/b", "b");
    f.mgr.submit(1, Request::Trash { paths: vec![f.p("src/a")] });
    drain(&f);
    f.mgr.undo(1).unwrap();
    drain(&f);
    assert!(f.mgr.redo_title(1).is_some());
    f.mgr.submit(1, Request::Trash { paths: vec![f.p("src/b")] });
    drain(&f);
    assert!(f.mgr.redo_title(1).is_none());
    assert!(f.mgr.undo_title(2).is_none());
    assert!(f.mgr.undo(2).is_none());
}

#[test]
fn undo_asks_instead_of_overwriting_when_the_name_was_taken_meanwhile() {
    let f = Fx::new();
    f.write("src/f", "original");
    f.mgr.submit(1, Request::Trash { paths: vec![f.p("src/f")] });
    drain(&f);
    f.write("src/f", "newcomer");
    let id = f.mgr.undo(1).unwrap();
    assert!(matches!(f.next(), Event::Prompt(_, Prompt::Conflict(_))));
    f.mgr.answer_conflict(id, ConflictAnswer { choice: ConflictChoice::KeepBoth, apply_to_all: false });
    drain(&f);
    assert_eq!(f.read("src/f"), "newcomer");
    assert_eq!(f.read("src/f 2"), "original");
}

#[test]
fn symlink_request_and_error_answer_plumbing() {
    let f = Fx::new();
    f.write("src/doc", "d");
    f.mgr.submit(1, Request::Symlink { paths: vec![f.p("src/doc")], dest_dir: f.p("dst") });
    let report = drain(&f);
    assert_eq!(report.results, vec![f.p("dst/doc symlink")]);
    assert_eq!(f.read("dst/doc symlink"), "d");
    // Answering a prompt nobody asked is harmless.
    f.mgr.answer_error(999, ErrorChoice::Skip);
    f.mgr.pause(999);
    f.mgr.stop(999);
}

// ------------------------------------------------------- Two connections

/// A second connection that is really the same disk, but which the engine
/// must treat as a separate file system: no rename across, bytes streamed.
fn add_server(f: &Fx) -> u64 {
    f.registry.add(Arc::new(LocalFs), "server", f.p("dst"))
}

#[test]
fn copy_between_connections_streams_and_always_queues() {
    let f = Fx::new();
    f.write("src/a.txt", "hello");
    LocalFs.set_times(&f.p("src/a.txt"), Some(1_000_000_000_000), None).unwrap();
    let server = add_server(&f);
    f.mgr.submit_on(
        1,
        Request::Transfer {
            mode: TransferMode::Copy,
            sources: vec![f.p("src/a.txt")],
            dest_dir: f.p("dst"),
            rename_to: None,
        },
        LOCAL,
        server,
    );
    let report = drain(&f);
    assert_eq!(report.result, ActivityResult::Success);
    assert_eq!(f.read("dst/a.txt"), "hello");
    // The date is set on the target after an upload.
    assert_eq!(LocalFs.stat(&f.p("dst/a.txt")).unwrap().modified_ms, Some(1_000_000_000_000));
    // An upload cannot be undone: a server has no Trash to move it to.
    assert!(f.mgr.undo_title(1).is_none());
}

#[test]
fn move_between_connections_can_be_undone_by_moving_back() {
    let f = Fx::new();
    f.write("src/a.txt", "hello");
    let server = add_server(&f);
    f.mgr.submit_on(
        1,
        Request::Transfer {
            mode: TransferMode::Move,
            sources: vec![f.p("src/a.txt")],
            dest_dir: f.p("dst"),
            rename_to: None,
        },
        LOCAL,
        server,
    );
    drain(&f);
    assert!(!f.exists("src/a.txt") && f.exists("dst/a.txt"));
    assert_eq!(f.mgr.undo_title(1).as_deref(), Some("Undo Move"));
    f.mgr.undo(1).unwrap();
    drain(&f);
    assert_eq!(f.read("src/a.txt"), "hello");
    assert!(!f.exists("dst/a.txt"));
}

#[test]
fn fo11_a_server_has_no_trash_and_a_closed_connection_is_refused() {
    let f = Fx::new();
    f.write("dst/a.txt", "x");
    let server = add_server(&f);
    f.mgr.submit_on(1, Request::Trash { paths: vec![f.p("dst/a.txt")] }, server, server);
    let report = drain(&f);
    assert_eq!(report.result, ActivityResult::Failed);
    assert!(report.message.unwrap().contains("no Trash"));
    assert!(f.exists("dst/a.txt"));

    // Delete works there, and is what the shell offers instead.
    f.mgr.submit_on(1, Request::Delete { paths: vec![f.p("dst/a.txt")] }, server, server);
    assert_eq!(drain(&f).result, ActivityResult::Success);
    assert!(!f.exists("dst/a.txt"));

    f.registry.remove(server);
    f.mgr.submit_on(1, Request::NewFolder { dir: f.p("dst") }, server, server);
    let report = drain(&f);
    assert_eq!(report.result, ActivityResult::Failed);
    assert!(report.message.unwrap().contains("closed"));
}

#[test]
fn put_replaces_the_remote_file_without_asking() {
    let f = Fx::new();
    f.write("src/edited.html", "new content");
    f.write("dst/index.html", "old content");
    let server = add_server(&f);
    f.mgr.submit_on(1, Request::Put { source: f.p("src/edited.html"), target: f.p("dst/index.html") }, LOCAL, server);
    let report = drain(&f);
    assert_eq!(report.title, "Uploading “index.html”");
    assert_eq!(report.result, ActivityResult::Success);
    assert_eq!(f.read("dst/index.html"), "new content");
    assert_eq!(f.names("dst"), vec!["index.html"]); // no staging file left
    assert_eq!(f.mgr.log().last().unwrap().title, "Uploaded “index.html”");
    assert!(f.mgr.undo_title(1).is_none());
}

#[test]
fn a_put_that_fails_ends_failed_without_an_error_prompt() {
    // The app offers its own "Remote Editing Error" with Retry instead.
    let f = Fx::new();
    f.write("dst/index.html", "old content");
    let server = add_server(&f);
    f.mgr.submit_on(1, Request::Put { source: f.p("src/gone.html"), target: f.p("dst/index.html") }, LOCAL, server);
    match f.next() {
        Event::Finished(report) => {
            assert_eq!(report.result, ActivityResult::Failed);
            assert!(report.message.is_some());
        }
        Event::Prompt(..) => panic!("a failed upload of an edited file must not prompt"),
    }
    assert_eq!(f.read("dst/index.html"), "old content");
}

// ----------------------------------------------- Multi-rename and sync

#[test]
fn mr04_multi_rename_swaps_names_logs_pairs_and_undoes_in_one_step() {
    let f = Fx::new();
    f.write("src/a", "was a");
    f.write("src/b", "was b");
    f.write("src/c.txt", "c");
    f.mgr.submit(
        1,
        Request::MultiRename {
            pairs: vec![
                (f.p("src/a"), f.p("src/b")),
                (f.p("src/b"), f.p("src/a")),
                (f.p("src/c.txt"), f.p("src/C.TXT")),
            ],
        },
    );
    let report = drain(&f);
    assert_eq!(report.title, "Renaming 3 items");
    assert_eq!(report.result, ActivityResult::Success);
    assert_eq!(f.read("src/a"), "was b");
    assert_eq!(f.read("src/b"), "was a");
    assert_eq!(f.names("src"), vec!["C.TXT", "a", "b"]);
    let entry = f.mgr.log().pop().unwrap();
    assert_eq!(entry.title, "Renamed 3 items");
    assert!(entry.lines.contains(&"a → b".to_string()) && entry.lines.contains(&"c.txt → C.TXT".to_string()));

    // One undo puts every name back.
    assert_eq!(f.mgr.undo_title(1).as_deref(), Some("Undo Rename"));
    f.mgr.undo(1).unwrap();
    drain(&f);
    assert_eq!(f.read("src/a"), "was a");
    assert_eq!(f.read("src/b"), "was b");
    assert_eq!(f.names("src"), vec!["a", "b", "c.txt"]);
}

#[test]
fn multi_rename_never_overwrites_an_item_that_appeared_meanwhile() {
    let f = Fx::new();
    f.write("src/a", "a");
    f.write("src/taken", "precious");
    f.mgr.submit(1, Request::MultiRename { pairs: vec![(f.p("src/a"), f.p("src/taken"))] });
    let report = drain(&f);
    assert_eq!(report.result, ActivityResult::Failed);
    assert_eq!(f.read("src/taken"), "precious");
    assert_eq!(f.read("src/a"), "a");
}

#[test]
fn sy07_running_a_sync_plan_leaves_nothing_to_do() {
    use crate::sync::{analyze, Pair, SyncOptions};
    let f = Fx::new();
    f.write("src/new.txt", "new");
    f.write("src/sub/deep.txt", "deep");
    f.write("src/changed.txt", "longer content");
    f.write("dst/changed.txt", "short");
    f.write("dst/orphan.txt", "orphan");
    let (left, right) = (f.p("src"), f.p("dst"));
    let options = SyncOptions { delete_orphans: true, ..Default::default() };
    let plan_now = || {
        analyze(
            &Pair { left: &LocalFs, left_root: &left, right: &LocalFs, right_root: &right, case_insensitive: true },
            &options,
            &|| true,
            &|_| {},
        )
        .unwrap()
    };
    let plan = plan_now();
    assert_eq!(plan.rows.len(), 5); // new.txt, sub, sub/deep.txt, changed.txt, orphan.txt

    let steps = plan
        .rows
        .iter()
        .filter(|r| r.checked)
        .map(|r| SyncStep {
            path: r.path.clone(),
            action: r.action,
            to_right: r.target == crate::sync::Side::Right,
            is_dir: r.is_dir,
        })
        .collect();
    f.mgr.submit(1, Request::Sync { left_root: left.clone(), right_root: right.clone(), steps });
    let report = drain(&f);
    assert_eq!(report.title, "Synchronizing 5 items");
    assert_eq!(report.result, ActivityResult::Success);

    assert_eq!(f.read("dst/new.txt"), "new");
    assert_eq!(f.read("dst/sub/deep.txt"), "deep");
    assert_eq!(f.read("dst/changed.txt"), "longer content");
    assert!(!f.exists("dst/orphan.txt"));
    assert!(f.names("trash").is_empty(), "a sync deletes, it does not trash");
    let entry = f.mgr.log().pop().unwrap();
    assert_eq!(entry.title, "Synchronized 5 items");
    assert!(entry.lines.contains(&"Added 3, updated 1, deleted 1".to_string()));
    assert!(f.mgr.undo_title(1).is_none());

    // Copies keep modification times, so a second look finds nothing.
    assert!(plan_now().rows.is_empty(), "{:?}", plan_now().rows);
}

// ---------------------------------------------------------------- Archives

/// Zip `sources` (relative to the fixture) into `target` and open it as a
/// connection, the way the app does before extracting.
fn open_zip(f: &Fx, sources: &[&str], target: &str) -> (crate::registry::ConnId, Vec<String>) {
    let sources: Vec<String> = sources.iter().map(|s| f.p(s)).collect();
    let options = crate::archive::CompressOptions::default();
    assert_eq!(crate::archive::compress(&sources, &f.p(target), &options, &|| true, &mut |_, _| {}), Ok(true));
    open_archive(f, target)
}

fn open_archive(f: &Fx, target: &str) -> (crate::registry::ConnId, Vec<String>) {
    let opened = crate::archive::open(&f.p(target), None, &f.p("support")).unwrap();
    let origin = crate::registry::Location::local(f.p(target));
    let conn = f.registry.add_archive(opened.vfs, crate::paths::file_name(target), origin);
    (conn, opened.blocked)
}

fn extract(f: &Fx, conn: crate::registry::ConnId, name: &str, dest: &str, blocked: Vec<String>) -> FinishReport {
    let vfs = f.registry.get(conn).unwrap();
    let (sources, rename_to) = crate::archive::extract_plan(&*vfs, name).unwrap();
    let request = Request::Extract { archive_name: name.into(), sources, dest_dir: f.p(dest), rename_to, blocked };
    f.mgr.submit_on(1, request, conn, LOCAL);
    drain(f)
}

#[test]
fn ar03_several_top_level_items_extract_into_a_folder_named_after_the_archive() {
    let f = Fx::new();
    for name in ["1.txt", "2.txt", "3.txt"] {
        f.write(&format!("src/{name}"), name);
    }
    let old = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    fs::File::open(f.tmp.path().join("src/1.txt")).unwrap().set_modified(old).unwrap();
    let (conn, blocked) = open_zip(&f, &["src/1.txt", "src/2.txt", "src/3.txt"], "src/a.zip");

    let report = extract(&f, conn, "a.zip", "dst", blocked);
    assert_eq!(report.result, ActivityResult::Success);
    assert_eq!(report.title, "Extracting “a.zip”");
    assert_eq!(report.results, vec![f.p("dst/a")]);
    assert_eq!(f.names("dst/a"), vec!["1.txt", "2.txt", "3.txt"]);
    assert_eq!(f.read("dst/a/2.txt"), "2.txt");
    // AR-02: the archived date comes out with the file.
    let extracted = fs::metadata(f.tmp.path().join("dst/a/1.txt")).unwrap().modified().unwrap();
    assert_eq!(extracted, old);
    assert_eq!(f.mgr.log().last().unwrap().title, "Extracted “a.zip”");

    // Extracting is undoable like any copy.
    f.mgr.undo(1).unwrap();
    drain(&f);
    assert!(!f.exists("dst/a"));
}

#[test]
fn a_single_top_level_item_is_extracted_as_itself() {
    let f = Fx::new();
    f.write("src/docs/readme.txt", "read me");
    f.write("src/docs/deep/more.txt", "more");
    let (conn, blocked) = open_zip(&f, &["src/docs"], "src/one.zip");
    let report = extract(&f, conn, "one.zip", "dst", blocked);
    assert_eq!(report.results, vec![f.p("dst/docs")]);
    assert_eq!(f.read("dst/docs/deep/more.txt"), "more");
    assert_eq!(f.names("dst"), vec!["docs"]);
}

#[test]
fn extracting_onto_an_existing_folder_asks_like_any_folder_conflict() {
    let f = Fx::new();
    f.write("src/docs/readme.txt", "new");
    f.write("dst/docs/keep.txt", "keep");
    let (conn, _) = open_zip(&f, &["src/docs"], "src/one.zip");
    let vfs = f.registry.get(conn).unwrap();
    let (sources, rename_to) = crate::archive::extract_plan(&*vfs, "one.zip").unwrap();
    let request = Request::Extract {
        archive_name: "one.zip".into(),
        sources,
        dest_dir: f.p("dst"),
        rename_to,
        blocked: Vec::new(),
    };
    let id = f.mgr.submit_on(1, request, conn, LOCAL);
    match f.next() {
        Event::Prompt(_, Prompt::Conflict(info)) => assert_eq!(info.kind, super::engine::ConflictKind::DirDir),
        _ => panic!("expected a folder conflict"),
    }
    f.mgr.answer_conflict(id, ConflictAnswer { choice: ConflictChoice::Merge, apply_to_all: false });
    assert_eq!(f.finished().result, ActivityResult::Success);
    assert_eq!(f.names("dst/docs"), vec!["keep.txt", "readme.txt"]);
}

#[test]
fn ar04_entries_that_climb_out_are_not_written_and_the_log_names_them() {
    let f = Fx::new();
    let script = "import zipfile\nz = zipfile.ZipFile('evil.zip', 'w')\nz.writestr('ok.txt', 'fine')\nz.writestr('../evil.txt', 'bad')\nz.close()\n";
    let status = std::process::Command::new("/usr/bin/python3")
        .args(["-c", script])
        .current_dir(f.tmp.path().join("src"))
        .status()
        .unwrap();
    assert!(status.success());
    let (conn, blocked) = open_archive(&f, "src/evil.zip");
    assert_eq!(blocked, ["../evil.txt"]);
    let report = extract(&f, conn, "evil.zip", "dst", blocked);
    assert_eq!(report.result, ActivityResult::Success);
    assert_eq!(f.names("dst"), vec!["ok.txt"]);
    assert!(!f.exists("evil.txt") && !f.exists("src/evil.txt"));
    let log = f.mgr.log();
    assert!(log.last().unwrap().lines.iter().any(|l| l.contains("Blocked “../evil.txt”")), "{log:?}");
}

#[test]
fn ar05_compress_names_the_archive_and_never_reuses_a_name() {
    let f = Fx::new();
    f.write("src/one.txt", "1");
    f.write("src/two.txt", "2");
    let request = || Request::Compress {
        sources: vec![f.p("src/one.txt"), f.p("src/two.txt")],
        dest_dir: f.p("src"),
        name: "Archive.zip".into(),
        options: crate::archive::CompressOptions::default(),
    };
    let report = {
        f.mgr.submit(1, request());
        drain(&f)
    };
    assert_eq!(report.result, ActivityResult::Success);
    assert_eq!(report.title, "Compressing 2 items");
    assert_eq!(report.results, vec![f.p("src/Archive.zip")], "the new archive is what gets selected");
    let report = {
        f.mgr.submit(1, request());
        drain(&f)
    };
    assert_eq!(report.results, vec![f.p("src/Archive 2.zip")]);

    let (conn, _) = open_archive(&f, "src/Archive 2.zip");
    let mut names: Vec<String> = f.registry.get(conn).unwrap().list("/").unwrap().into_iter().map(|e| e.name).collect();
    names.sort();
    assert_eq!(names, ["one.txt", "two.txt"]);

    // Undo removes the archive that was made.
    f.mgr.undo(1).unwrap();
    drain(&f);
    assert!(!f.exists("src/Archive 2.zip") && f.exists("src/Archive.zip"));
}

#[test]
fn a_compress_that_fails_says_why_and_leaves_nothing() {
    let f = Fx::new();
    let request = Request::Compress {
        sources: vec![f.p("src/missing.txt")],
        dest_dir: f.p("src"),
        name: "missing.zip".into(),
        options: crate::archive::CompressOptions::default(),
    };
    f.mgr.submit(1, request);
    let report = drain(&f);
    assert_eq!(report.result, ActivityResult::Failed);
    assert!(report.message.unwrap().contains("can’t be created"));
    assert!(f.names("src").is_empty());
}

// ------------------------------------------------------- Aliases, tags

#[test]
fn alias_request_names_items_and_undo_trashes_them() {
    let f = Fx::new();
    f.write("src/doc.txt", "d");
    f.write("src/unaliasable", "x");
    f.write("dst/doc.txt alias", "taken");
    f.mgr.submit(1, Request::Alias { paths: vec![f.p("src/doc.txt"), f.p("src/unaliasable")], dest_dir: f.p("dst") });
    let report = drain(&f);
    assert_eq!(report.result, ActivityResult::Partial);
    assert_eq!(report.results, vec![f.p("dst/doc.txt alias 2")]);
    assert_eq!(f.read("dst/doc.txt alias 2"), format!("alias to {}", f.p("src/doc.txt")));
    assert_eq!(f.mgr.undo_title(1).as_deref(), Some("Undo Make Alias"));
    f.mgr.undo(1).unwrap();
    drain(&f);
    assert!(!f.exists("dst/doc.txt alias 2"));
    assert!(f.exists("dst/doc.txt alias"));
}

#[test]
fn aliases_are_refused_on_a_server() {
    let f = Fx::new();
    f.write("dst/doc", "d");
    let server = add_server(&f);
    f.mgr.submit_on(1, Request::Alias { paths: vec![f.p("dst/doc")], dest_dir: f.p("dst") }, server, server);
    let report = drain(&f);
    assert_eq!(report.result, ActivityResult::Failed);
    assert!(report.message.unwrap().contains("only be made on this Mac"));
}

#[test]
fn recorded_tag_and_comment_changes_undo_and_redo() {
    let f = Fx::new();
    f.write("src/a", "a");
    let a = f.p("src/a");
    f.mgr.record(
        1,
        "Tags",
        "Changed the tags of “a”",
        vec![Effect::Tags { path: a.clone(), before: vec!["Red".into()], after: vec!["Red".into(), "Work".into()] }],
    );
    assert_eq!(f.mgr.undo_title(1).as_deref(), Some("Undo Tags"));
    assert_eq!(f.mgr.log().last().unwrap().title, "Changed the tags of “a”");
    f.mgr.undo(1).unwrap();
    let report = drain(&f);
    assert_eq!(report.result, ActivityResult::Success);
    assert_eq!(f.read("src/a.tags"), "Red");
    assert_eq!(f.mgr.redo_title(1).as_deref(), Some("Redo Tags"));
    f.mgr.redo(1).unwrap();
    drain(&f);
    assert_eq!(f.read("src/a.tags"), "Red,Work");

    f.mgr.record(
        1,
        "Comment",
        "Changed the comment of “a”",
        vec![Effect::Comment { path: a.clone(), before: "old".into(), after: "new".into() }],
    );
    // A new change clears what could be redone.
    assert_eq!(f.mgr.redo_title(1), None);
    f.mgr.undo(1).unwrap();
    drain(&f);
    assert_eq!(f.read("src/a.comment"), "old");
    // Nothing recorded for no change.
    f.mgr.record(1, "Tags", "x", Vec::new());
    assert_eq!(f.mgr.undo_title(1).as_deref(), Some("Undo Tags"));
}

// ------------------------------------------------- Administrator rights

#[test]
fn trashing_from_a_locked_folder_asks_and_can_run_as_administrator() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fx::new();
    f.write("src/sys/App.app", "bundle");
    f.write("src/sys/refused", "x");
    let sys = f.tmp.path().join("src/sys");
    fs::set_permissions(&sys, fs::Permissions::from_mode(0o555)).unwrap();

    let id = f.mgr.submit(1, Request::Trash { paths: vec![f.p("src/sys/App.app")] });
    match f.next() {
        Event::Prompt(_, Prompt::Error(info)) => assert!(info.can_elevate, "{info:?}"),
        _ => panic!("expected an error prompt"),
    }
    f.mgr.answer_error(id, ErrorChoice::Elevate);
    let report = drain(&f);
    assert_eq!(report.result, ActivityResult::Success);
    assert!(!f.exists("src/sys/App.app"));
    assert_eq!(f.read("trash/App.app"), "bundle");
    assert_eq!(f.mgr.undo_title(1).as_deref(), Some("Undo Move to Trash"));

    // Declining the password is reported as a failure.
    let id = f.mgr.submit(1, Request::Trash { paths: vec![f.p("src/sys/refused")] });
    assert!(matches!(f.next(), Event::Prompt(_, Prompt::Error(_))));
    f.mgr.answer_error(id, ErrorChoice::Elevate);
    let report = drain(&f);
    assert_eq!(report.result, ActivityResult::Failed);
    assert!(report.message.unwrap().contains("Not done as administrator"));
    fs::set_permissions(&sys, fs::Permissions::from_mode(0o755)).unwrap();
}
