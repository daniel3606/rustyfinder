//! Timing of the paths that make a big folder feel fast or slow.
//!
//!   cargo run --release -p rustyfinder-core --example bench [files]
//!
//! Makes a temporary folder with `files` empty files (default 100 000),
//! then times listing it, the first sort, re-sorts, sorting by date added
//! (which reads that date lazily, once) and grouping. Each step runs three
//! times; the best run is printed.

use std::sync::Arc;
use std::time::{Duration, Instant};

use rustyfinder_core::group::GroupKey;
use rustyfinder_core::snapshot::{Snapshot, ViewSpec};
use rustyfinder_core::sort::{SortKey, SortSpec};
use rustyfinder_core::{LocalFs, Vfs};

fn best<T>(mut run: impl FnMut() -> T) -> (Duration, T) {
    let mut best = Duration::MAX;
    let mut last = None;
    for _ in 0..3 {
        let start = Instant::now();
        let out = run();
        best = best.min(start.elapsed());
        last = Some(out);
    }
    (best, last.unwrap())
}

fn view(key: SortKey, ascending: bool, group: GroupKey) -> ViewSpec {
    ViewSpec { sort: SortSpec { key, ascending, folders_first: true }, group, ..Default::default() }
}

fn main() {
    let files: usize = std::env::args().nth(1).and_then(|n| n.parse().ok()).unwrap_or(100_000);
    let dir = tempfile::tempdir().expect("temp folder");
    eprint!("making {files} files… ");
    for i in 0..files {
        std::fs::File::create(dir.path().join(format!("file{}.dat", (i * 7919) % files))).unwrap();
    }
    eprintln!("done");
    let path = dir.path().to_str().unwrap().to_string();
    let source: Arc<dyn Vfs> = Arc::new(LocalFs);

    let (list, entries) = best(|| LocalFs.list(&path).unwrap());
    println!("list {files} entries            {:>8.1} ms", list.as_secs_f64() * 1000.0);

    let (first, snap) =
        best(|| Snapshot::listed(&path, entries.clone(), ViewSpec::default(), Some(Arc::clone(&source))));
    println!("first sort (name)               {:>8.1} ms", first.as_secs_f64() * 1000.0);

    let (by_size, _) = best(|| snap.with_view(view(SortKey::Size, false, GroupKey::None)));
    println!("re-sort by size                 {:>8.1} ms", by_size.as_secs_f64() * 1000.0);

    // The first sort by date added reads that date for every item (once);
    // later sorts reuse it.
    let fresh = Snapshot::listed(&path, entries.clone(), ViewSpec::default(), Some(Arc::clone(&source)));
    let start = Instant::now();
    let _ = fresh.with_view(view(SortKey::Added, false, GroupKey::None));
    println!("sort by date added, first time  {:>8.1} ms", start.elapsed().as_secs_f64() * 1000.0);
    let (again, _) = best(|| fresh.with_view(view(SortKey::Added, true, GroupKey::None)));
    println!("sort by date added, again       {:>8.1} ms", again.as_secs_f64() * 1000.0);

    let (grouped, g) = best(|| snap.with_view(view(SortKey::Name, true, GroupKey::Modified)));
    println!(
        "group by date modified          {:>8.1} ms  ({} groups)",
        grouped.as_secs_f64() * 1000.0,
        g.groups().len()
    );
}
