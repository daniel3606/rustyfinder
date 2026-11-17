//! Per-pane controller: owns the navigation history and the current
//! snapshot, loads directories on worker threads, and tells an observer when
//! a new snapshot is ready.
//!
//! Every request gets a generation number. A slow listing that finishes
//! after a newer request is dropped, so the UI never shows a stale folder.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

use crate::entry::Entry;
use crate::error::VfsError;
use crate::folder_views::{self, FolderView, FolderViews};
use crate::history::History;
use crate::paths;
use crate::registry::{ConnId, Location, VfsRegistry, LOCAL};
use crate::search::{self, Query};
use crate::snapshot::{Snapshot, ViewSpec};
use crate::vfs::Vfs;
use crate::watch::DirWatcher;

/// Why a snapshot was delivered. The UI uses it to decide what to do with
/// the selection and scroll position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotReason {
    /// Moved to a different folder: reset selection and scroll.
    Navigated,
    /// Same folder re-read from disk: keep selection by name.
    Reloaded,
    /// Same contents under a new sort/filter: keep selection by name.
    ViewChanged,
}

/// Everything the UI needs to refresh its chrome after a load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneState {
    /// Which file system the folder is on (`LOCAL` for the disk).
    pub conn: ConnId,
    pub path: String,
    pub reason: SnapshotReason,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub can_go_up: bool,
    /// Entry to select once shown, e.g. the folder we just came up out of.
    pub select_name: Option<String>,
    /// Set while the pane shows search results instead of the folder.
    pub search: Option<SearchStatus>,
}

/// A search whose results the pane is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchStatus {
    pub term: String,
    /// Results are still arriving.
    pub running: bool,
    /// Something the user should know: files skipped, engine switched.
    pub note: Option<String>,
}

/// Receives results. Called on worker threads, never on the caller's thread.
pub trait PaneObserver: Send + Sync {
    fn snapshot_ready(&self, snapshot: Arc<Snapshot>, state: PaneState);
    fn load_failed(&self, path: String, error: VfsError);
}

enum Intent {
    Visit {
        conn: ConnId,
        path: String,
        select: Option<String>,
        /// Do not report a failure (used for sync browsing, where a missing
        /// counterpart folder is expected).
        quiet: bool,
    },
    Back,
    Forward,
    Reload,
    /// Reload triggered by the folder watcher, not the user.
    AutoReload,
}

struct State {
    history: History,
    view: ViewSpec,
    snapshot: Option<Arc<Snapshot>>,
    /// Last selected name per visited folder, so coming back to a folder
    /// puts the cursor where it was.
    remembered: HashMap<String, String>,
    watcher: Option<DirWatcher>,
    /// The search being shown, if any.
    search: Option<(Query, bool, SearchStatus)>,
    /// Per-folder view settings, and the view for folders with none.
    folder_views: Option<(Arc<FolderViews>, FolderView)>,
    /// The next arrival keeps the current view (a restored tab).
    keep_view_once: bool,
}

struct Inner {
    registry: Arc<VfsRegistry>,
    observer: Arc<dyn PaneObserver>,
    state: Mutex<State>,
    generation: AtomicU64,
    /// Requests in flight. The watcher holds off while this is non-zero so
    /// an automatic reload never cancels a navigation the user asked for.
    loading: AtomicU64,
    refresh_pending: AtomicBool,
    /// The folder changed while something was loading: reload once that
    /// load is done, or the change would be lost (nothing else notices).
    reload_after: AtomicBool,
    /// Bumped whenever a search starts or ends; a running search stops
    /// when it no longer holds the current number.
    search_generation: AtomicU64,
    /// The search number Stop was pressed for: it ends early but its
    /// results stay on show.
    search_stopped: AtomicU64,
}

/// How long to wait after a change before reloading, so a burst of changes
/// (a copy writing many files) causes one reload, not hundreds.
const REFRESH_DEBOUNCE: Duration = Duration::from_millis(200);

/// How often a running search hands over what it has found so far.
const RESULTS_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub struct PaneController {
    inner: Arc<Inner>,
}

impl PaneController {
    /// A pane on a single local-style backend (handy for tests).
    pub fn new(vfs: Arc<dyn Vfs>, home: impl Into<String>, observer: Arc<dyn PaneObserver>) -> Self {
        Self::with_registry(Arc::new(VfsRegistry::new(vfs, home)), observer)
    }

    /// A pane that can show any connection in `registry`.
    pub fn with_registry(registry: Arc<VfsRegistry>, observer: Arc<dyn PaneObserver>) -> Self {
        Self {
            inner: Arc::new(Inner {
                registry,
                observer,
                state: Mutex::new(State {
                    history: History::new(),
                    view: ViewSpec::default(),
                    snapshot: None,
                    remembered: HashMap::new(),
                    watcher: None,
                    search: None,
                    folder_views: None,
                    keep_view_once: false,
                }),
                generation: AtomicU64::new(0),
                loading: AtomicU64::new(0),
                refresh_pending: AtomicBool::new(false),
                reload_after: AtomicBool::new(false),
                search_generation: AtomicU64::new(0),
                search_stopped: AtomicU64::new(0),
            }),
        }
    }

    /// Go to `path` on the file system the pane is on (may start with
    /// `~`, may contain `..`).
    pub fn navigate(&self, path: &str) {
        self.visit(self.current_conn(), path, None, false);
    }

    /// Go to `path` on another connection (or back to the local disk).
    pub fn navigate_to(&self, conn: ConnId, path: &str) {
        self.visit(conn, path, None, false);
    }

    /// Like `navigate`, but selects `name` once the folder is shown.
    pub fn navigate_selecting(&self, path: &str, name: &str) {
        self.visit(self.current_conn(), path, Some(name.to_string()), false);
    }

    /// Like `navigate`, but a failure is not reported to the observer.
    pub fn navigate_quiet(&self, path: &str) {
        self.visit(self.current_conn(), path, None, true);
    }

    fn visit(&self, conn: ConnId, path: &str, select: Option<String>, quiet: bool) {
        let path = paths::normalize(path, &self.inner.registry.home(conn));
        start(&self.inner, Intent::Visit { conn, path, select, quiet });
    }

    /// The connection the pane is on (the local disk before any load).
    pub fn current_conn(&self) -> ConnId {
        let st = self.inner.state.lock().unwrap();
        st.history.current().map_or(LOCAL, |l| l.conn)
    }

    pub fn current_location(&self) -> Option<Location> {
        self.inner.state.lock().unwrap().history.current().cloned()
    }

    /// A connection closed: drop it from this pane's history. Returns true
    /// if the pane was showing it and now needs somewhere else to go.
    pub fn forget_connection(&self, conn: ConnId) -> bool {
        let mut st = self.inner.state.lock().unwrap();
        let was_current = st.history.current().is_some_and(|l| l.conn == conn);
        st.history.forget(conn);
        if was_current {
            st.snapshot = None;
            st.watcher = None;
        }
        was_current
    }

    /// Record which entry is selected in the current folder, so returning to
    /// this folder later restores it.
    pub fn remember_selection(&self, name: Option<&str>) {
        let mut st = self.inner.state.lock().unwrap();
        let Some(current) = st.history.current().map(memory_key) else {
            return;
        };
        match name {
            Some(name) => st.remembered.insert(current, name.to_string()),
            None => st.remembered.remove(&current),
        };
    }

    /// Set the view used by the first load, without triggering one.
    pub fn set_initial_view(&self, view: ViewSpec) {
        self.inner.state.lock().unwrap().view = view;
    }

    /// Open each folder with its own view settings (or `fallback` when it
    /// has none), and remember view changes made in a folder for it.
    pub fn set_folder_views(&self, views: Arc<FolderViews>, fallback: FolderView) {
        self.inner.state.lock().unwrap().folder_views = Some((views, fallback));
    }

    /// The next folder opened keeps the view as it is now.
    pub fn keep_view_on_next_visit(&self) {
        self.inner.state.lock().unwrap().keep_view_once = true;
    }

    /// The current folder's key in the per-folder settings.
    fn folder_key(&self) -> Option<(Arc<FolderViews>, String)> {
        let st = self.inner.state.lock().unwrap();
        let (views, _) = st.folder_views.clone()?;
        let location = st.history.current()?.clone();
        drop(st);
        let source = self.inner.registry.get(location.conn)?.id();
        Some((views, folder_views::key(&source, &location.path)))
    }

    /// View Options › This Folder and Subfolders.
    pub fn apply_view_to_subfolders(&self) {
        if let Some((views, key)) = self.folder_key() {
            views.remember(&key, FolderView { recursive: true, ..FolderView::of(&self.view()) });
        }
    }

    /// View Options › Use as Defaults.
    pub fn use_view_as_defaults(&self) {
        if let Some((views, _)) = self.folder_key() {
            views.set_defaults(FolderView::of(&self.view()));
        }
    }

    /// Restore Default View Settings (⌥⌘R): drop this folder's own settings
    /// and show it as it would open without them.
    pub fn restore_default_view(&self) {
        let Some((views, key)) = self.folder_key() else { return };
        views.forget(&key);
        let fallback = self.inner.state.lock().unwrap().folder_views.as_ref().map(|(_, f)| *f).unwrap_or_default();
        let look = views.lookup(&key).unwrap_or(fallback);
        self.apply_view(look.apply_to(&self.view()), false);
    }

    /// Whether the current folder has settings of its own, and whether
    /// they apply to its subfolders too.
    pub fn view_scope(&self) -> (bool, bool) {
        self.folder_key().and_then(|(views, key)| views.own(&key)).map_or((false, false), |v| (true, v.recursive))
    }

    /// Open the parent folder and select the folder we came from.
    pub fn go_up(&self) {
        let Some(current) = self.current_location() else {
            return;
        };
        let Some(parent) = paths::parent(&current.path) else {
            return;
        };
        let select = Some(paths::file_name(&current.path).to_string());
        start(&self.inner, Intent::Visit { conn: current.conn, path: parent, select, quiet: false });
    }

    /// Back out of search results to the folder, else to the folder before.
    pub fn go_back(&self) {
        if self.is_searching() {
            return self.end_search();
        }
        start(&self.inner, Intent::Back);
    }

    pub fn go_forward(&self) {
        start(&self.inner, Intent::Forward);
    }

    /// Re-read the current folder from disk (or run the search again).
    pub fn reload(&self) {
        let search = self.inner.state.lock().unwrap().search.clone();
        match search {
            Some((query, use_spotlight, _)) => self.search(query, use_spotlight),
            None => start(&self.inner, Intent::Reload),
        }
    }

    pub fn is_searching(&self) -> bool {
        self.inner.state.lock().unwrap().search.is_some()
    }

    /// Show what `query` finds below the current folder instead of the
    /// folder itself. Results arrive in batches while the search runs.
    /// With `use_spotlight` the system index answers where it can; servers
    /// and questions it cannot answer fall back to walking the tree.
    pub fn search(&self, query: Query, use_spotlight: bool) {
        self.search_in(None, query, use_spotlight);
    }

    /// Like `search`, but below `root` instead of the current folder: the
    /// pane moves there (a history entry) and shows the results.
    pub fn search_in(&self, root: Option<Location>, query: Query, use_spotlight: bool) {
        let inner = Arc::clone(&self.inner);
        // A navigation still loading must not replace the results.
        inner.generation.fetch_add(1, Ordering::SeqCst);
        let ticket = inner.search_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let location = {
            let mut st = inner.state.lock().unwrap();
            if let Some(root) = &root {
                st.history.visit(root);
            }
            let Some(location) = st.history.current().cloned() else { return };
            // The folder is not on show, so there is nothing to refresh.
            st.watcher = None;
            let status = SearchStatus { term: query.term.clone(), running: true, note: None };
            st.search = Some((query.clone(), use_spotlight, status));
            location
        };
        spawn(move || {
            let current = || {
                inner.search_generation.load(Ordering::SeqCst) == ticket
                    && inner.search_stopped.load(Ordering::SeqCst) != ticket
            };
            let mut status = SearchStatus { term: query.term.clone(), running: true, note: None };
            let Some(vfs) = inner.registry.get(location.conn) else {
                status.running = false;
                status.note = Some("The connection is closed.".into());
                publish_results(&inner, ticket, &location, &[], status, true);
                return;
            };
            // The index only knows the local disk.
            let indexed = use_spotlight && location.conn == LOCAL && search::spotlight_expression(&query).is_some();
            if use_spotlight && !indexed {
                status.note = Some("Spotlight can’t answer this here, so RustyFinder searched itself.".into());
            }
            // Show the (empty) result list at once, then fill it in.
            publish_results(&inner, ticket, &location, &[], status.clone(), true);
            let mut entries: Vec<Entry> = Vec::new();
            let mut published = (Instant::now(), 0usize);
            let mut found = |entry: Entry| {
                entries.push(entry);
                if published.0.elapsed() >= RESULTS_INTERVAL {
                    publish_results(&inner, ticket, &location, &entries, status.clone(), false);
                    published = (Instant::now(), entries.len());
                }
            };
            let result = if indexed {
                search::spotlight(&*vfs, &location.path, &query, &current, &mut found)
            } else {
                search::walk(&*vfs, &location.path, &query, &current, &mut found)
            };
            status.running = false;
            let stopped = inner.search_stopped.load(Ordering::SeqCst) == ticket;
            match result {
                Ok(_) if stopped => status.note = Some("Search stopped.".into()),
                Ok(outcome) if outcome.skipped_large > 0 => {
                    let n = outcome.skipped_large;
                    status.note = Some(format!("{n} large file{} not read.", if n == 1 { " was" } else { "s were" }));
                }
                Ok(_) => {}
                Err(message) => status.note = Some(message),
            }
            publish_results(&inner, ticket, &location, &entries, status, false);
        });
    }

    /// Stop a running search early. What it found so far stays on show.
    pub fn stop_search(&self) {
        let ticket = self.inner.search_generation.load(Ordering::SeqCst);
        let running = self.inner.state.lock().unwrap().search.as_ref().is_some_and(|s| s.2.running);
        if running {
            self.inner.search_stopped.store(ticket, Ordering::SeqCst);
        }
    }

    /// Leave the search results and show the folder again.
    pub fn end_search(&self) {
        self.inner.search_generation.fetch_add(1, Ordering::SeqCst);
        if self.inner.state.lock().unwrap().search.take().is_some() {
            start(&self.inner, Intent::Reload);
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.inner.state.lock().unwrap().history.can_go_back()
    }

    /// Change sort, hidden-file visibility or filter. Re-uses the loaded
    /// entries; nothing is read from disk.
    pub fn set_view(&self, view: ViewSpec) {
        self.apply_view(view, true);
    }

    fn apply_view(&self, view: ViewSpec, remember: bool) {
        // A change of look made in a folder is that folder's from now on.
        if remember && !FolderView::of(&view).same_look(&FolderView::of(&self.view())) {
            if let Some((views, key)) = self.folder_key() {
                let keep_recursive = views.own(&key).is_some_and(|v| v.recursive);
                let mut look = FolderView::of(&view);
                look.recursive = keep_recursive;
                if keep_recursive {
                    // Still covers the subtree, without wiping it again.
                    views.remember(&key, FolderView { recursive: false, ..look });
                    views.remember_flag(&key, true);
                } else {
                    views.remember(&key, look);
                }
            }
        }
        let inner = Arc::clone(&self.inner);
        let gen = inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let base = {
            let mut st = inner.state.lock().unwrap();
            st.view = view.clone();
            st.snapshot.clone()
        };
        // Nothing loaded yet: the new view simply applies to the next load.
        let Some(base) = base else { return };
        spawn(move || {
            let snapshot = Arc::new(base.with_view(view));
            let state = {
                let mut st = inner.state.lock().unwrap();
                if inner.generation.load(Ordering::SeqCst) != gen {
                    return;
                }
                // Search results may have grown while this ran: re-apply
                // the view to what is there now rather than lose them.
                let snapshot = match &st.snapshot {
                    Some(now) if !Arc::ptr_eq(now, &base) => Arc::new(now.with_view(st.view.clone())),
                    _ => snapshot,
                };
                st.snapshot = Some(Arc::clone(&snapshot));
                let Some(location) = st.history.current().cloned() else {
                    return;
                };
                let mut state = describe(&inner.registry, &st.history, &location, SnapshotReason::ViewChanged, None);
                state.search = st.search.as_ref().map(|s| s.2.clone());
                (snapshot, state)
            };
            let (snapshot, state) = state;
            inner.observer.snapshot_ready(snapshot, state);
        });
    }

    pub fn view(&self) -> ViewSpec {
        self.inner.state.lock().unwrap().view.clone()
    }

    pub fn current_path(&self) -> Option<String> {
        let st = self.inner.state.lock().unwrap();
        st.history.current().map(|l| l.path.clone())
    }

    pub fn snapshot(&self) -> Option<Arc<Snapshot>> {
        self.inner.state.lock().unwrap().snapshot.clone()
    }
}

/// Hand a batch of search results to the UI, unless the search has been
/// replaced or ended. `first` resets selection and scroll like a navigation.
fn publish_results(
    inner: &Arc<Inner>,
    ticket: u64,
    location: &Location,
    entries: &[Entry],
    status: SearchStatus,
    first: bool,
) {
    let view = inner.state.lock().unwrap().view.clone();
    let snapshot = Arc::new(Snapshot::of_results(location.path.clone(), entries.to_vec(), view));
    let (snapshot, state) = {
        let mut st = inner.state.lock().unwrap();
        if inner.search_generation.load(Ordering::SeqCst) != ticket {
            return;
        }
        let snapshot =
            if *snapshot.view() == st.view { snapshot } else { Arc::new(snapshot.with_view(st.view.clone())) };
        st.snapshot = Some(Arc::clone(&snapshot));
        if let Some(search) = st.search.as_mut() {
            search.2 = status.clone();
        }
        let reason = if first { SnapshotReason::Navigated } else { SnapshotReason::Reloaded };
        let mut state = describe(&inner.registry, &st.history, location, reason, None);
        state.search = Some(status);
        (snapshot, state)
    };
    inner.observer.snapshot_ready(snapshot, state);
}

/// At the end of a load: a change noticed meanwhile gets its reload now.
struct ReloadAfter(Arc<Inner>);

impl Drop for ReloadAfter {
    fn drop(&mut self) {
        if self.0.reload_after.swap(false, Ordering::SeqCst) {
            start(&self.0, Intent::AutoReload);
        }
    }
}

fn start(inner: &Arc<Inner>, intent: Intent) {
    let inner = Arc::clone(inner);
    // Under the state lock, so an automatic reload and a navigation can't
    // interleave: an automatic reload starts only if nothing is loading,
    // and a navigation counts as loading from the moment it starts.
    // Otherwise a reload could slip in after the check and cancel the
    // navigation, leaving the tab where it was.
    let gen = {
        let st = inner.state.lock().unwrap();
        if matches!(intent, Intent::AutoReload) {
            // A change noticed while search results are on show must not
            // swap them for the folder.
            if st.search.is_some() {
                return;
            }
            if inner.loading.load(Ordering::SeqCst) > 0 {
                inner.reload_after.store(true, Ordering::SeqCst);
                return;
            }
        }
        inner.loading.fetch_add(1, Ordering::SeqCst);
        inner.generation.fetch_add(1, Ordering::SeqCst) + 1
    };

    // Work out the target now, but only commit history once the load
    // succeeds, so a failed navigation leaves the pane where it was.
    let (location, select, view) = {
        let mut st = inner.state.lock().unwrap();
        let target = match &intent {
            Intent::Visit { conn, path, select, .. } => {
                Some((Location { conn: *conn, path: path.clone() }, select.clone()))
            }
            Intent::Back => st.history.peek_back().map(|l| (l.clone(), None)),
            Intent::Forward => st.history.peek_forward().map(|l| (l.clone(), None)),
            Intent::Reload | Intent::AutoReload => st.history.current().map(|l| (l.clone(), None)),
        };
        let Some((l, s)) = target else {
            inner.loading.fetch_sub(1, Ordering::SeqCst);
            return;
        };
        // Arriving somewhere new opens it with its own settings, except in
        // Column view, which keeps its layout while browsing inside it.
        let base = st.view.clone();
        let mut view = base.clone();
        let arriving = matches!(intent, Intent::Visit { .. } | Intent::Back | Intent::Forward)
            && !std::mem::take(&mut st.keep_view_once);
        if arriving && base.layout != crate::snapshot::Layout::Columns {
            if let (Some((views, fallback)), Some(vfs)) = (&st.folder_views, inner.registry.get(l.conn)) {
                let look = views.lookup(&folder_views::key(&vfs.id(), &l.path)).unwrap_or(*fallback);
                view = look.apply_to(&base);
            }
        }
        (l, s, (view, base))
    };
    let (view, base_view) = view;

    spawn(move || {
        // Runs when this load is over, whichever way it ends.
        let _after = ReloadAfter(Arc::clone(&inner));
        let path = location.path.clone();
        // A connection that has been closed has no backend any more.
        let vfs = inner.registry.get(location.conn);
        let listed = match &vfs {
            Some(vfs) => vfs.list(&path),
            None => Err(VfsError::Io { path: path.clone(), message: "The connection is closed.".into() }),
        };
        inner.loading.fetch_sub(1, Ordering::SeqCst);
        if inner.generation.load(Ordering::SeqCst) != gen {
            return;
        }
        let entries = match listed {
            Ok(entries) => entries,
            Err(error) => {
                match intent {
                    Intent::Visit { quiet: true, .. } => {}
                    // The folder we were showing is gone (deleted, or its
                    // drive was unplugged): fall back quietly to the
                    // nearest folder that still exists.
                    Intent::AutoReload => {
                        if let (VfsError::NotFound(_), Some(vfs)) = (&error, &vfs) {
                            let home = inner.registry.home(location.conn);
                            let fallback = nearest_existing(&**vfs, &path, &home);
                            start(
                                &inner,
                                Intent::Visit { conn: location.conn, path: fallback, select: None, quiet: true },
                            );
                        }
                    }
                    _ => inner.observer.load_failed(path, error),
                }
                return;
            }
        };
        let snapshot = Arc::new(Snapshot::listed(path.clone(), entries, view, vfs.clone()));

        let (snapshot, state) = {
            let mut st = inner.state.lock().unwrap();
            if inner.generation.load(Ordering::SeqCst) != gen {
                return;
            }
            let moved = st.history.current() != Some(&location);
            match intent {
                Intent::Visit { .. } => st.history.visit(&location),
                Intent::Back => {
                    st.history.go_back();
                }
                Intent::Forward => {
                    st.history.go_forward();
                }
                Intent::Reload | Intent::AutoReload => {}
            }
            // The folder's own view, unless the user changed the view while
            // it was being listed.
            if st.view == base_view {
                st.view = snapshot.view().clone();
            }
            // The view may have changed while we were listing.
            let snapshot = if *snapshot.view() == st.view {
                Arc::clone(&snapshot)
            } else {
                Arc::new(snapshot.with_view(st.view.clone()))
            };
            st.snapshot = Some(Arc::clone(&snapshot));
            // Arriving anywhere ends a search.
            let was_searching = st.search.take().is_some();
            if was_searching {
                inner.search_generation.fetch_add(1, Ordering::SeqCst);
            }
            let reason = if moved || was_searching { SnapshotReason::Navigated } else { SnapshotReason::Reloaded };
            let select =
                if moved { select.or_else(|| st.remembered.get(&memory_key(&location)).cloned()) } else { select };
            if moved || st.watcher.is_none() {
                st.watcher = vfs.as_ref().and_then(|vfs| watch(&inner, &**vfs, &path));
            }
            let state = describe(&inner.registry, &st.history, &location, reason, select);
            (snapshot, state)
        };
        inner.observer.snapshot_ready(snapshot, state);
    });
}

/// Key for the per-folder selection memory.
fn memory_key(location: &Location) -> String {
    format!("{}:{}", location.conn, location.path)
}

/// Start watching `path`; changes schedule one debounced automatic reload.
fn watch(inner: &Arc<Inner>, vfs: &dyn Vfs, path: &str) -> Option<DirWatcher> {
    let dir = vfs.watch_path(path)?;
    let weak: Weak<Inner> = Arc::downgrade(inner);
    DirWatcher::new(&dir, move || {
        let Some(inner) = weak.upgrade() else { return };
        if inner.refresh_pending.swap(true, Ordering::SeqCst) {
            return; // a reload is already scheduled
        }
        spawn(move || {
            thread::sleep(REFRESH_DEBOUNCE);
            inner.refresh_pending.store(false, Ordering::SeqCst);
            if inner.loading.load(Ordering::SeqCst) == 0 {
                start(&inner, Intent::AutoReload);
            }
        });
    })
}

/// Closest ancestor of `path` that can still be listed, else `home`.
fn nearest_existing(vfs: &dyn Vfs, path: &str, home: &str) -> String {
    let mut current = paths::parent(path);
    while let Some(dir) = current {
        if dir != "/" && vfs.stat(&dir).is_ok_and(|e| e.is_dir) {
            return dir;
        }
        current = paths::parent(&dir);
    }
    home.to_string()
}

fn describe(
    registry: &VfsRegistry,
    history: &History,
    location: &Location,
    reason: SnapshotReason,
    select_name: Option<String>,
) -> PaneState {
    PaneState {
        conn: location.conn,
        path: location.path.clone(),
        reason,
        can_go_back: history.can_go_back(),
        can_go_forward: history.can_go_forward(),
        // From the top of an archive, "up" leads out to the archive file.
        can_go_up: paths::parent(&location.path).is_some() || registry.origin(location.conn).is_some(),
        select_name,
        search: None,
    }
}

fn spawn(job: impl FnOnce() + Send + 'static) {
    thread::Builder::new().name("rustyfinder-pane".into()).spawn(job).expect("failed to spawn pane worker thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{Entry, EntryKind};
    use crate::error::VfsResult;
    use crate::local::LocalFs;
    use crate::sort::{SortKey, SortSpec};
    use std::fs;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::time::Duration;

    enum Event {
        Ready(Arc<Snapshot>, PaneState),
        Failed(String, VfsError),
    }

    struct Chan(Mutex<Sender<Event>>);

    impl PaneObserver for Chan {
        fn snapshot_ready(&self, snapshot: Arc<Snapshot>, state: PaneState) {
            let _ = self.0.lock().unwrap().send(Event::Ready(snapshot, state));
        }
        fn load_failed(&self, path: String, error: VfsError) {
            let _ = self.0.lock().unwrap().send(Event::Failed(path, error));
        }
    }

    fn pane_with(vfs: Arc<dyn Vfs>) -> (PaneController, Receiver<Event>) {
        let (tx, rx) = channel();
        let pane = PaneController::new(vfs, "/home/me", Arc::new(Chan(Mutex::new(tx))));
        (pane, rx)
    }

    fn ready(rx: &Receiver<Event>) -> (Arc<Snapshot>, PaneState) {
        match rx.recv_timeout(Duration::from_secs(5)).expect("no event") {
            Event::Ready(s, st) => (s, st),
            Event::Failed(p, e) => panic!("load of {p} failed: {e}"),
        }
    }

    fn tree() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("a/inner")).unwrap();
        fs::create_dir(tmp.path().join("b")).unwrap();
        fs::write(tmp.path().join("a/file.txt"), b"x").unwrap();
        fs::write(tmp.path().join("a/.secret"), b"x").unwrap();
        tmp
    }

    /// Wait for the batch that ends a search.
    fn finished(rx: &Receiver<Event>) -> (Arc<Snapshot>, PaneState) {
        loop {
            let (snapshot, state) = ready(rx);
            if state.search.as_ref().is_some_and(|s| !s.running) {
                return (snapshot, state);
            }
        }
    }

    fn names(snapshot: &Snapshot) -> Vec<String> {
        snapshot.iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn search_shows_results_with_relative_paths_and_back_returns_to_the_folder() {
        // SE-02
        let tmp = tree();
        fs::write(tmp.path().join("file here.md"), b"x").unwrap();
        let root = tmp.path().to_str().unwrap().to_string();
        let (pane, rx) = pane_with(Arc::new(LocalFs));
        pane.navigate(&root);
        ready(&rx);

        let query = Query { term: "file".into(), subfolders: true, ..Query::default() };
        pane.search(query, false);
        let (first, state) = ready(&rx);
        assert_eq!(state.reason, SnapshotReason::Navigated);
        assert!(first.is_empty() && state.search.as_ref().unwrap().running);
        let (snapshot, state) = finished(&rx);
        assert_eq!(names(&snapshot), ["file here.md", "a/file.txt"]);
        assert_eq!(state.path, root);
        assert_eq!(state.search.unwrap().term, "file");
        assert!(pane.is_searching());

        // Sorting the results keeps them, and the tab still says "search".
        let mut view = pane.view();
        view.sort.ascending = false;
        pane.set_view(view);
        let (snapshot, state) = ready(&rx);
        assert_eq!(names(&snapshot), ["a/file.txt", "file here.md"]);
        assert!(state.search.is_some());

        pane.go_back();
        let (snapshot, state) = ready(&rx);
        assert_eq!(state.path, root);
        assert!(state.search.is_none() && !pane.is_searching());
        assert_eq!(snapshot.len(), 3);
        assert!(!state.can_go_back, "the search itself is not a history entry");
    }

    #[test]
    fn going_somewhere_else_ends_a_search_and_reload_runs_it_again() {
        let tmp = tree();
        let root = tmp.path().to_str().unwrap().to_string();
        let (pane, rx) = pane_with(Arc::new(LocalFs));
        pane.navigate(&root);
        ready(&rx);
        let query =
            Query { term: "txt".into(), criterion: search::Criterion::Extension, subfolders: true, ..Query::default() };
        pane.search(query, false);
        assert_eq!(names(&finished(&rx).0), ["a/file.txt"]);

        fs::write(tmp.path().join("new.txt"), b"x").unwrap();
        pane.reload();
        assert_eq!(names(&finished(&rx).0), ["a/file.txt", "new.txt"]);

        pane.navigate(&format!("{root}/a"));
        let (_, state) = ready(&rx);
        assert!(state.search.is_none() && !pane.is_searching());
        assert_eq!(state.reason, SnapshotReason::Navigated);
    }

    /// A file system whose listings take a while, so a search is still
    /// running when Stop is pressed.
    struct Slow(LocalFs);

    impl Vfs for Slow {
        fn id(&self) -> String {
            "slow".into()
        }
        fn list(&self, path: &str) -> VfsResult<Vec<Entry>> {
            std::thread::sleep(Duration::from_millis(40));
            self.0.list(path)
        }
        fn stat(&self, path: &str) -> VfsResult<Entry> {
            self.0.stat(path)
        }
        fn open_read(&self, path: &str) -> VfsResult<Box<dyn std::io::Read + Send>> {
            self.0.open_read(path)
        }
    }

    #[test]
    fn stop_ends_a_search_early_and_keeps_its_results() {
        // search.md §1: the Stop button.
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..60 {
            let dir = tmp.path().join(format!("d{i:02}"));
            fs::create_dir(&dir).unwrap();
            fs::write(dir.join("hit.txt"), b"x").unwrap();
        }
        let root = tmp.path().to_str().unwrap().to_string();
        let (pane, rx) = pane_with(Arc::new(Slow(LocalFs)));
        pane.navigate(&root);
        ready(&rx);
        pane.search(Query { term: "hit".into(), subfolders: true, ..Query::default() }, false);
        ready(&rx);
        std::thread::sleep(Duration::from_millis(300));
        pane.stop_search();
        let (snapshot, state) = finished(&rx);
        let status = state.search.unwrap();
        assert_eq!(status.note.as_deref(), Some("Search stopped."));
        assert!(!snapshot.is_empty() && snapshot.len() < 60, "partial results kept: {}", snapshot.len());
        assert!(pane.is_searching(), "the results stay on show");
        // Stopping again, or when nothing runs, changes nothing.
        pane.stop_search();
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn a_search_that_cannot_run_explains_why() {
        let (pane, rx) = pane_with(Arc::new(LocalFs));
        let tmp = tree();
        pane.navigate(tmp.path().to_str().unwrap());
        ready(&rx);
        let bad = Query { term: "(".into(), regex: true, ..Query::default() };
        pane.search(bad, false);
        let (snapshot, state) = finished(&rx);
        assert!(snapshot.is_empty());
        assert!(state.search.unwrap().note.unwrap().contains("regular expression"));
    }

    fn pane_with_folder_views() -> (tempfile::TempDir, String, PaneController, Receiver<Event>) {
        let tmp = tree();
        fs::create_dir_all(tmp.path().join("a/inner")).unwrap();
        let root = tmp.path().to_str().unwrap().to_string();
        let (pane, rx) = pane_with(Arc::new(LocalFs));
        pane.set_folder_views(Arc::new(FolderViews::in_memory()), FolderView::default());
        (tmp, root, pane, rx)
    }

    /// VW-07: a view set in a folder comes back there and nowhere else.
    #[test]
    fn spec_vw07_view_is_kept_per_folder() {
        let (_tmp, root, pane, rx) = pane_with_folder_views();
        pane.navigate(&format!("{root}/a"));
        ready(&rx);
        pane.set_view(ViewSpec { layout: crate::snapshot::Layout::Icons, ..pane.view() });
        ready(&rx);
        pane.navigate(&format!("{root}/b"));
        let (snap, _) = ready(&rx);
        assert_eq!(snap.view().layout, crate::snapshot::Layout::List, "a sibling keeps its own (default) view");
        pane.go_back();
        let (snap, _) = ready(&rx);
        assert_eq!(snap.view().layout, crate::snapshot::Layout::Icons);
        assert_eq!(pane.view_scope(), (true, false));
    }

    /// VW-08: applied to subfolders, a folder below opens the same way;
    /// Restore Default View Settings brings back the default.
    #[test]
    fn spec_vw08_view_applies_to_subfolders() {
        let (_tmp, root, pane, rx) = pane_with_folder_views();
        pane.navigate(&root);
        ready(&rx);
        let by_size = SortSpec { key: SortKey::Size, ascending: false, folders_first: true };
        pane.set_view(ViewSpec { sort: by_size, ..pane.view() });
        ready(&rx);
        pane.apply_view_to_subfolders();
        assert_eq!(pane.view_scope(), (true, true));
        pane.navigate(&format!("{root}/a/inner"));
        let (snap, _) = ready(&rx);
        assert_eq!(snap.view().sort, by_size);
        pane.go_back();
        ready(&rx);
        pane.restore_default_view();
        let (snap, _) = ready(&rx);
        assert_eq!(snap.view().sort.key, SortKey::Name);
        assert_eq!(pane.view_scope(), (false, false));
    }

    #[test]
    fn hidden_files_setting_is_not_per_folder() {
        let (_tmp, root, pane, rx) = pane_with_folder_views();
        pane.navigate(&format!("{root}/a"));
        ready(&rx);
        pane.set_view(ViewSpec { show_hidden: true, ..pane.view() });
        ready(&rx);
        pane.navigate(&format!("{root}/b"));
        let (snap, _) = ready(&rx);
        assert!(snap.view().show_hidden);
    }

    #[test]
    fn navigate_up_back_forward() {
        let tmp = tree();
        let root = tmp.path().to_str().unwrap().to_string();
        let (pane, rx) = pane_with(Arc::new(LocalFs));

        pane.navigate(&root);
        let (snap, st) = ready(&rx);
        assert_eq!(snap.len(), 2);
        assert_eq!(st.reason, SnapshotReason::Navigated);
        assert!(!st.can_go_back && !st.can_go_forward && st.can_go_up);

        pane.navigate(&format!("{root}/a/"));
        let (snap, st) = ready(&rx);
        assert_eq!(st.path, format!("{root}/a"));
        assert_eq!(snap.len(), 2); // inner, file.txt (hidden one filtered)
        assert!(st.can_go_back);

        pane.go_up();
        let (_, st) = ready(&rx);
        assert_eq!(st.path, root);
        assert_eq!(st.select_name.as_deref(), Some("a"));

        pane.go_back();
        let (_, st) = ready(&rx);
        assert_eq!(st.path, format!("{root}/a"));
        assert!(st.can_go_forward);

        pane.go_forward();
        let (_, st) = ready(&rx);
        assert_eq!(st.path, root);
        assert!(!st.can_go_forward);
    }

    #[test]
    fn failed_navigation_keeps_position() {
        let tmp = tree();
        let root = tmp.path().to_str().unwrap().to_string();
        let (pane, rx) = pane_with(Arc::new(LocalFs));
        pane.navigate(&root);
        ready(&rx);

        pane.navigate(&format!("{root}/missing"));
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Event::Failed(_, VfsError::NotFound(_)) => {}
            _ => panic!("expected NotFound"),
        }
        assert_eq!(pane.current_path().as_deref(), Some(root.as_str()));
        pane.reload();
        let (_, st) = ready(&rx);
        assert!(!st.can_go_back);
        assert_eq!(st.reason, SnapshotReason::Reloaded);
    }

    #[test]
    fn view_change_reuses_listing_and_reload_sees_new_files() {
        let tmp = tree();
        let a = format!("{}/a", tmp.path().to_str().unwrap());
        let (pane, rx) = pane_with(Arc::new(LocalFs));
        pane.navigate(&a);
        let (snap, _) = ready(&rx);
        assert_eq!(snap.len(), 2);

        // A file created after the load must not appear on a view change...
        fs::write(format!("{a}/new.txt"), b"x").unwrap();
        pane.set_view(ViewSpec {
            show_hidden: true,
            sort: SortSpec { key: SortKey::Name, ascending: false, folders_first: false },
            filter: String::new(),
            ..Default::default()
        });
        let (snap, st) = ready(&rx);
        assert_eq!(st.reason, SnapshotReason::ViewChanged);
        let names: Vec<_> = snap.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["inner", "file.txt", ".secret"]);

        // ...but does after a reload, which also keeps the new view.
        pane.reload();
        let (snap, _) = ready(&rx);
        assert_eq!(snap.len(), 4);
        assert_eq!(snap.get(0).unwrap().name, "new.txt");
    }

    #[test]
    fn no_op_requests_do_nothing() {
        let (pane, rx) = pane_with(Arc::new(LocalFs));
        pane.go_back();
        pane.go_forward();
        pane.go_up();
        pane.reload();
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn folder_changes_refresh_automatically() {
        let tmp = tree();
        let a = format!("{}/a", tmp.path().to_str().unwrap());
        let (pane, rx) = pane_with(Arc::new(LocalFs));
        pane.navigate(&a);
        ready(&rx);
        thread::sleep(Duration::from_millis(400)); // let the watcher arm

        fs::write(format!("{a}/appeared.txt"), b"x").unwrap();
        // A refresh already under way (the watcher can still be reporting
        // the folder's creation) may land first without the new file; the
        // file must show in a refresh that follows, within the deadline.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let (snap, st) = ready(&rx);
            assert_eq!(st.reason, SnapshotReason::Reloaded);
            if snap.index_of("appeared.txt").is_some() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "the new file never showed up");
        }
    }

    #[test]
    fn deleted_folder_falls_back_to_nearest_ancestor() {
        let tmp = tree();
        let root = fs::canonicalize(tmp.path()).unwrap().to_str().unwrap().to_string();
        let (pane, rx) = pane_with(Arc::new(LocalFs));
        pane.navigate(&format!("{root}/a/inner"));
        ready(&rx);
        thread::sleep(Duration::from_millis(400));

        fs::remove_dir_all(format!("{root}/a")).unwrap();
        let (_, st) = loop {
            let (snap, st) = ready(&rx);
            if st.reason == SnapshotReason::Navigated {
                break (snap, st);
            }
        };
        assert_eq!(st.path, root);
    }

    #[test]
    fn returning_to_a_folder_restores_its_selection() {
        let tmp = tree();
        let root = tmp.path().to_str().unwrap().to_string();
        let (pane, rx) = pane_with(Arc::new(LocalFs));
        pane.navigate(&root);
        ready(&rx);
        pane.remember_selection(Some("b"));
        pane.navigate(&format!("{root}/a"));
        ready(&rx);
        pane.go_back();
        let (_, st) = ready(&rx);
        assert_eq!(st.select_name.as_deref(), Some("b"));
    }

    #[test]
    fn quiet_navigation_does_not_report_failure() {
        let tmp = tree();
        let root = tmp.path().to_str().unwrap().to_string();
        let (pane, rx) = pane_with(Arc::new(LocalFs));
        pane.navigate(&root);
        ready(&rx);
        pane.navigate_quiet(&format!("{root}/missing"));
        // No failure is reported. (An automatic refresh of the folder may
        // still arrive: the watcher can see the files `tree()` just made.)
        let deadline = std::time::Instant::now() + Duration::from_millis(300);
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(Event::Failed(path, error)) => panic!("quiet navigation reported {path}: {error}"),
                Ok(Event::Ready(_, st)) => assert_eq!(st.path, root, "only the folder itself may refresh"),
                Err(_) => break,
            }
        }
        assert_eq!(pane.current_path().as_deref(), Some(root.as_str()));
    }

    #[test]
    fn spec_pt09_back_twice_forward_twice() {
        let tmp = tree();
        let root = tmp.path().to_str().unwrap().to_string();
        let (pane, rx) = pane_with(Arc::new(LocalFs));
        for p in [root.clone(), format!("{root}/a"), format!("{root}/a/inner")] {
            pane.navigate(&p);
            ready(&rx);
        }
        for step in
            [PaneController::go_back, PaneController::go_back, PaneController::go_forward, PaneController::go_forward]
        {
            step(&pane);
            ready(&rx);
        }
        assert_eq!(pane.current_path().unwrap(), format!("{root}/a/inner"));
    }

    /// A backend whose first listing blocks until released, to prove stale
    /// results are dropped.
    struct SlowFs {
        gate: Mutex<Option<Receiver<()>>>,
    }

    impl Vfs for SlowFs {
        fn id(&self) -> String {
            "slow".into()
        }
        fn list(&self, path: &str) -> VfsResult<Vec<Entry>> {
            if path == "/slow" {
                let gate = self.gate.lock().unwrap().take();
                if let Some(gate) = gate {
                    let _ = gate.recv_timeout(Duration::from_secs(5));
                }
            }
            Ok(vec![Entry {
                name: format!("from{}", path.replace('/', "_")),
                kind: EntryKind::File,
                is_dir: false,
                size: 0,
                modified_ms: None,
                created_ms: None,
                is_hidden: false,
                mode: None,
                link_target: None,
                is_broken_link: false,
                ext: Default::default(),
            }])
        }
        fn stat(&self, path: &str) -> VfsResult<Entry> {
            Err(VfsError::Unsupported(path.into()))
        }
    }

    #[test]
    fn stale_load_is_dropped() {
        let (release, gate) = channel();
        let (pane, rx) = pane_with(Arc::new(SlowFs { gate: Mutex::new(Some(gate)) }));
        pane.navigate("/slow");
        pane.navigate("/fast");
        let (snap, st) = ready(&rx);
        assert_eq!(st.path, "/fast");
        assert_eq!(snap.get(0).unwrap().name, "from_fast");

        release.send(()).unwrap();
        // The slow result must never arrive.
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        assert_eq!(pane.current_path().as_deref(), Some("/fast"));
    }

    #[test]
    fn tilde_expands_to_home() {
        let (pane, rx) = pane_with(Arc::new(SlowFs { gate: Mutex::new(None) }));
        pane.navigate("~/Documents/../Desktop");
        let (_, st) = ready(&rx);
        assert_eq!(st.path, "/home/me/Desktop");
    }

    #[test]
    fn loading_from_a_closed_connection_fails_cleanly() {
        let tmp = tree();
        let root = tmp.path().to_str().unwrap().to_string();
        let (pane, rx) = pane_with(Arc::new(LocalFs));
        pane.navigate(&root);
        ready(&rx);
        pane.navigate_to(42, "/anything");
        match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Event::Failed(_, VfsError::Io { message, .. }) => assert!(message.contains("closed")),
            _ => panic!("expected a closed-connection error"),
        }
        // The pane is still where it was.
        assert_eq!(pane.current_path().as_deref(), Some(root.as_str()));
        assert_eq!(pane.current_conn(), crate::registry::LOCAL);
    }
}
