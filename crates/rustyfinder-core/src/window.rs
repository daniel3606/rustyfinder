//! One browser window: two panes, each with an ordered list of tabs.
//!
//! All tab and pane rules from the panes-tabs spec live here (pinning, swap,
//! clone, sync browsing, closing), so the shell only renders `WindowInfo`
//! and forwards commands.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::error::VfsError;
use crate::folder_views::{FolderView, FolderViews};
use crate::pane::{PaneController, PaneObserver, PaneState, SnapshotReason};
use crate::paths;
use crate::registry::{ConnId, VfsRegistry, LOCAL};
use crate::snapshot::{Snapshot, ViewSpec};
use crate::store::{SavedPane, SavedTab, SavedWindow, Store};

pub type TabId = u64;

static NEXT_TAB_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }

    fn idx(self) -> usize {
        match self {
            Side::Left => 0,
            Side::Right => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabInfo {
    pub id: TabId,
    /// Which file system the tab shows (`LOCAL` for the disk).
    pub conn: ConnId,
    /// Name of the remote connection ("bob@example.org"), "" for local.
    pub label: String,
    pub path: String,
    pub pinned: bool,
    /// For a tab restored on a server it is not connected to yet: the
    /// server's URL. The tab shows nothing until the shell reconnects and
    /// calls `navigate_to`.
    pub pending: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneTabs {
    pub tabs: Vec<TabInfo>,
    pub selected: usize,
}

/// Everything the shell needs to draw the window's structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub left: PaneTabs,
    pub right: PaneTabs,
    pub active: Side,
    pub single_pane: bool,
    pub horizontal: bool,
    pub sync_browsing: bool,
}

impl WindowInfo {
    pub fn pane(&self, side: Side) -> &PaneTabs {
        match side {
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }

    /// The selected tab of `side`.
    pub fn current(&self, side: Side) -> &TabInfo {
        let pane = self.pane(side);
        &pane.tabs[pane.selected]
    }
}

/// What happened when a tab was asked to close.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseOutcome {
    Closed,
    /// Pinned tabs stay until unpinned.
    Pinned,
    /// It was the pane's last tab: the window should close instead.
    LastTab,
    /// No such tab.
    Missing,
}

/// Called on worker threads.
pub trait WindowObserver: Send + Sync {
    fn window_changed(&self, info: WindowInfo);
    fn snapshot_ready(&self, tab: TabId, snapshot: Arc<Snapshot>, state: PaneState);
    fn load_failed(&self, tab: TabId, path: String, error: VfsError);
}

struct Tab {
    id: TabId,
    controller: PaneController,
    pinned: bool,
    /// Location shown, or being loaded for a tab that has not loaded yet.
    conn: ConnId,
    path: String,
    /// Server URL of a restored remote tab waiting to reconnect.
    pending: String,
}

struct PaneSlot {
    tabs: Vec<Tab>,
    selected: usize,
}

struct State {
    panes: [PaneSlot; 2],
    active: Side,
    single_pane: bool,
    horizontal: bool,
    sync_browsing: bool,
}

struct Inner {
    registry: Arc<VfsRegistry>,
    store: Arc<Store>,
    observer: Arc<dyn WindowObserver>,
    state: Mutex<State>,
}

#[derive(Clone)]
pub struct WindowModel {
    inner: Arc<Inner>,
}

/// Routes one tab's pane events to the window, tagged with the tab id.
struct TabObserver {
    tab: TabId,
    window: Weak<Inner>,
}

impl PaneObserver for TabObserver {
    fn snapshot_ready(&self, snapshot: Arc<Snapshot>, state: PaneState) {
        let Some(inner) = self.window.upgrade() else {
            return;
        };
        let mut changed = None;
        if state.reason == SnapshotReason::Navigated {
            let mut st = inner.state.lock().unwrap();
            if let Some(tab) = st.tab_mut(self.tab) {
                tab.path = state.path.clone();
                tab.conn = state.conn;
                tab.pending.clear();
                changed = Some(st.info(&inner.registry));
            }
            drop(st);
            // Recent Folders are kept per data source: the disk, or each
            // server by its URL. Searches never get here (not Navigated).
            if state.conn == LOCAL {
                inner.store.record_recent(&state.path);
            } else {
                let url = inner.registry.url(state.conn);
                if !url.is_empty() {
                    inner.store.record_remote_recent(&url, &state.path);
                }
            }
        }
        inner.observer.snapshot_ready(self.tab, snapshot, state);
        if let Some(info) = changed {
            inner.observer.window_changed(info);
        }
    }

    fn load_failed(&self, path: String, error: VfsError) {
        if let Some(inner) = self.window.upgrade() {
            inner.observer.load_failed(self.tab, path, error);
        }
    }
}

impl State {
    fn locate(&self, id: TabId) -> Option<(Side, usize)> {
        for side in [Side::Left, Side::Right] {
            if let Some(i) = self.panes[side.idx()].tabs.iter().position(|t| t.id == id) {
                return Some((side, i));
            }
        }
        None
    }

    fn tab(&self, id: TabId) -> Option<&Tab> {
        let (side, i) = self.locate(id)?;
        Some(&self.panes[side.idx()].tabs[i])
    }

    fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        let (side, i) = self.locate(id)?;
        Some(&mut self.panes[side.idx()].tabs[i])
    }

    fn current(&self, side: Side) -> &Tab {
        let pane = &self.panes[side.idx()];
        &pane.tabs[pane.selected]
    }

    fn info(&self, registry: &VfsRegistry) -> WindowInfo {
        let pane = |side: Side| {
            let p = &self.panes[side.idx()];
            PaneTabs {
                tabs: p
                    .tabs
                    .iter()
                    .map(|t| TabInfo {
                        id: t.id,
                        conn: t.conn,
                        label: if t.pending.is_empty() { registry.label(t.conn) } else { t.pending.clone() },
                        path: t.path.clone(),
                        pinned: t.pinned,
                        pending: t.pending.clone(),
                    })
                    .collect(),
                selected: p.selected,
            }
        };
        WindowInfo {
            left: pane(Side::Left),
            right: pane(Side::Right),
            active: self.active,
            single_pane: self.single_pane,
            horizontal: self.horizontal,
            sync_browsing: self.sync_browsing,
        }
    }
}

impl WindowModel {
    /// Create a window. With `restore`, its tabs and layout come back;
    /// otherwise both panes open one tab at the starting directory.
    pub fn new(
        registry: Arc<VfsRegistry>,
        store: Arc<Store>,
        observer: Arc<dyn WindowObserver>,
        restore: Option<SavedWindow>,
    ) -> Self {
        let inner = Arc::new_cyclic(|weak: &Weak<Inner>| {
            let start = store.starting_directory();
            let default_view = ViewSpec {
                show_hidden: store.settings().show_hidden,
                sort: crate::sort::SortSpec { folders_first: store.settings().folders_on_top, ..Default::default() },
                ..Default::default()
            };
            let build_pane = |saved: Option<&SavedPane>| {
                let mut tabs: Vec<Tab> = saved
                    .map(|p| p.tabs.as_slice())
                    .unwrap_or_default()
                    .iter()
                    .filter(|t| !t.path.is_empty())
                    .map(|t| {
                        let mut view = t.view.clone();
                        view.show_hidden = default_view.show_hidden;
                        make_tab(
                            weak,
                            &registry,
                            &store,
                            LOCAL,
                            &t.path,
                            view,
                            TabStart { pinned: t.pinned, restored: true, pending: t.server.clone() },
                        )
                    })
                    .collect();
                if tabs.is_empty() {
                    tabs.push(make_tab(
                        weak,
                        &registry,
                        &store,
                        LOCAL,
                        &start,
                        default_view.clone(),
                        TabStart::default(),
                    ));
                }
                let selected = saved.map_or(0, |p| p.selected).min(tabs.len() - 1);
                PaneSlot { tabs, selected }
            };
            let state = State {
                panes: [build_pane(restore.as_ref().map(|w| &w.left)), build_pane(restore.as_ref().map(|w| &w.right))],
                active: if restore.as_ref().is_some_and(|w| w.right_active) { Side::Right } else { Side::Left },
                single_pane: restore.as_ref().is_some_and(|w| w.single_pane),
                horizontal: restore.as_ref().is_some_and(|w| w.horizontal),
                sync_browsing: restore.as_ref().is_some_and(|w| w.sync_browsing),
            };
            Inner { registry, store, observer, state: Mutex::new(state) }
        });
        Self { inner }
    }

    pub fn info(&self) -> WindowInfo {
        self.inner.state.lock().unwrap().info(&self.inner.registry)
    }

    /// Run `change` on the state, then tell the observer if it reports a
    /// change. The observer is called outside the lock.
    fn mutate<R>(&self, change: impl FnOnce(&mut State) -> (bool, R)) -> R {
        let (info, result) = {
            let mut st = self.inner.state.lock().unwrap();
            let (changed, result) = change(&mut st);
            (changed.then(|| st.info(&self.inner.registry)), result)
        };
        if let Some(info) = info {
            self.inner.observer.window_changed(info);
        }
        result
    }

    fn new_tab_at(&self, conn: ConnId, path: &str, view: ViewSpec) -> Tab {
        make_tab(
            &Arc::downgrade(&self.inner),
            &self.inner.registry,
            &self.inner.store,
            conn,
            path,
            view,
            TabStart::default(),
        )
    }

    // --- Layout

    pub fn set_active(&self, side: Side) {
        self.mutate(|st| {
            let changed = st.active != side;
            st.active = side;
            (changed, ())
        });
    }

    /// Give focus to the other pane. Does nothing in single-pane mode.
    pub fn switch_active(&self) {
        self.mutate(|st| {
            if st.single_pane {
                return (false, ());
            }
            st.active = st.active.other();
            (true, ())
        });
    }

    pub fn toggle_single_pane(&self) {
        self.mutate(|st| {
            st.single_pane = !st.single_pane;
            (true, ())
        });
    }

    pub fn set_horizontal(&self, horizontal: bool) {
        self.mutate(|st| {
            let changed = st.horizontal != horizontal;
            st.horizontal = horizontal;
            (changed, ())
        });
    }

    pub fn toggle_sync_browsing(&self) {
        self.mutate(|st| {
            st.sync_browsing = !st.sync_browsing;
            (true, ())
        });
    }

    /// Exchange all tabs between the panes. Focus follows its content.
    pub fn swap_panes(&self) {
        self.mutate(|st| {
            st.panes.swap(0, 1);
            st.active = st.active.other();
            (true, ())
        });
    }

    /// Show the active pane's location in the other pane.
    pub fn clone_pane(&self) {
        let (target, conn, path) = {
            let st = self.inner.state.lock().unwrap();
            let active = st.current(st.active);
            if !active.pending.is_empty() {
                return; // not connected yet: nothing to show
            }
            (st.current(st.active.other()).id, active.conn, active.path.clone())
        };
        self.navigate_on(target, conn, &path, None);
    }

    // --- Tabs

    /// Open a tab next to the selected one in `side` and select it. Without
    /// `path` it shows the current tab's location.
    pub fn new_tab(&self, side: Side, path: Option<&str>) -> TabId {
        let (conn, path, view) = {
            let st = self.inner.state.lock().unwrap();
            let current = st.current(side);
            if !current.pending.is_empty() {
                // Beside a tab still waiting for its server: the disk.
                let start = self.inner.store.starting_directory();
                (
                    LOCAL,
                    path.map_or(start, |p| paths::normalize(p, &self.inner.registry.home(LOCAL))),
                    current.controller.view(),
                )
            } else {
                // A given path is on the same file system as the current tab.
                let path = path.map_or_else(
                    || current.path.clone(),
                    |p| paths::normalize(p, &self.inner.registry.home(current.conn)),
                );
                (current.conn, path, current.controller.view())
            }
        };
        let tab = self.new_tab_at(conn, &path, view);
        let id = tab.id;
        self.mutate(|st| {
            let pane = &mut st.panes[side.idx()];
            let at = pane.selected + 1;
            pane.tabs.insert(at, tab);
            pane.selected = at;
            (true, ())
        });
        id
    }

    pub fn close_tab(&self, id: TabId) -> CloseOutcome {
        self.mutate(|st| {
            let Some((side, i)) = st.locate(id) else {
                return (false, CloseOutcome::Missing);
            };
            let pane = &mut st.panes[side.idx()];
            if pane.tabs[i].pinned {
                return (false, CloseOutcome::Pinned);
            }
            if pane.tabs.len() == 1 {
                return (false, CloseOutcome::LastTab);
            }
            pane.tabs.remove(i);
            if i < pane.selected || pane.selected >= pane.tabs.len() {
                pane.selected = pane.selected.saturating_sub(1);
            }
            (true, CloseOutcome::Closed)
        })
    }

    /// Close every unpinned tab in `side` except the selected one.
    pub fn close_other_tabs(&self, side: Side) {
        self.mutate(|st| {
            let pane = &mut st.panes[side.idx()];
            let keep = pane.tabs[pane.selected].id;
            let before = pane.tabs.len();
            pane.tabs.retain(|t| t.id == keep || t.pinned);
            pane.selected = pane.tabs.iter().position(|t| t.id == keep).unwrap_or(0);
            (pane.tabs.len() != before, ())
        });
    }

    /// Select a tab and make its pane active.
    pub fn select_tab(&self, id: TabId) {
        self.mutate(|st| {
            let Some((side, i)) = st.locate(id) else {
                return (false, ());
            };
            st.panes[side.idx()].selected = i;
            st.active = side;
            (true, ())
        });
    }

    /// Select the tab at `index` (0-based) in `side`, if it exists.
    pub fn select_tab_index(&self, side: Side, index: usize) {
        self.mutate(|st| {
            let pane = &mut st.panes[side.idx()];
            if index >= pane.tabs.len() || pane.selected == index {
                return (false, ());
            }
            pane.selected = index;
            (true, ())
        });
    }

    /// Move the selection by `delta` tabs, wrapping around.
    pub fn cycle_tab(&self, side: Side, delta: i32) {
        self.mutate(|st| {
            let pane = &mut st.panes[side.idx()];
            let n = pane.tabs.len() as i32;
            if n < 2 {
                return (false, ());
            }
            pane.selected = (pane.selected as i32 + delta).rem_euclid(n) as usize;
            (true, ())
        });
    }

    /// Move a tab to `index` in `side`: reordering within a pane, or moving
    /// across. A pane never ends up empty: if its last tab leaves, it gets a
    /// fresh tab at the same location.
    pub fn move_tab(&self, id: TabId, side: Side, index: usize) {
        let refill = {
            let st = self.inner.state.lock().unwrap();
            let Some((from, _)) = st.locate(id) else {
                return;
            };
            (from != side && st.panes[from.idx()].tabs.len() == 1).then(|| {
                let t = st.current(from);
                if t.pending.is_empty() {
                    (t.conn, t.path.clone(), t.controller.view())
                } else {
                    (LOCAL, self.inner.store.starting_directory(), t.controller.view())
                }
            })
        };
        let refill = refill.map(|(conn, path, view)| self.new_tab_at(conn, &path, view));
        self.mutate(|st| {
            let Some((from, i)) = st.locate(id) else {
                return (false, ());
            };
            let source = &mut st.panes[from.idx()];
            let tab = source.tabs.remove(i);
            if let Some(fresh) = refill {
                if source.tabs.is_empty() {
                    source.tabs.push(fresh);
                }
            }
            if i < source.selected || source.selected >= source.tabs.len() {
                source.selected = source.selected.saturating_sub(1);
            }
            let target = &mut st.panes[side.idx()];
            let at = index.min(target.tabs.len());
            target.tabs.insert(at, tab);
            target.selected = at;
            (true, ())
        });
    }

    /// Move a tab to the end of the other pane and select it there.
    pub fn move_tab_to_other_pane(&self, id: TabId) {
        let side = {
            let st = self.inner.state.lock().unwrap();
            let Some((from, _)) = st.locate(id) else {
                return;
            };
            from.other()
        };
        self.move_tab(id, side, usize::MAX);
    }

    pub fn set_pinned(&self, id: TabId, pinned: bool) {
        self.mutate(|st| match st.tab_mut(id) {
            Some(tab) if tab.pinned != pinned => {
                tab.pinned = pinned;
                (true, ())
            }
            _ => (false, ()),
        });
    }

    // --- Navigation

    /// Go to `path` in a tab. A pinned tab stays put: the location opens in
    /// a new tab beside it. With sync browsing on, the other pane follows a
    /// step into a subfolder or up to the parent.
    pub fn navigate(&self, id: TabId, path: &str) {
        let Some(conn) = self.with_tab(id, |t| t.conn) else {
            return;
        };
        self.navigate_on(id, conn, path, None);
        self.follow(id, path);
    }

    /// Go to `path` on a given connection: a server just connected to, or
    /// back to the local disk.
    pub fn navigate_to(&self, id: TabId, conn: ConnId, path: &str) {
        self.navigate_on(id, conn, path, None);
    }

    /// A connection closed. Tabs that were showing it go to the starting
    /// directory; their history loses its entries.
    pub fn connection_closed(&self, conn: ConnId) {
        let controllers: Vec<PaneController> = {
            let st = self.inner.state.lock().unwrap();
            st.panes.iter().flat_map(|p| p.tabs.iter().map(|t| t.controller.clone())).collect()
        };
        let start = self.inner.store.starting_directory();
        for controller in controllers {
            if controller.forget_connection(conn) {
                controller.navigate_to(LOCAL, &start);
            }
        }
    }

    /// Go to the parent folder, selecting the folder we left.
    pub fn go_up(&self, id: TabId) {
        // "Up" out of search results is the folder that was searched.
        if let Some(c) = self.with_tab(id, |t| t.controller.clone()) {
            if c.is_searching() {
                return c.end_search();
            }
        }
        let Some((path, parent)) = self.with_tab(id, |t| (t.path.clone(), paths::parent(&t.path))) else {
            return;
        };
        let Some(conn) = self.with_tab(id, |t| t.conn) else {
            return;
        };
        let Some(parent) = parent else {
            // The top of an archive: leave it, selecting the archive file.
            if let Some(origin) = self.inner.registry.origin(conn) {
                let folder = paths::parent(&origin.path).unwrap_or_else(|| "/".to_string());
                self.navigate_on(id, origin.conn, &folder, Some(paths::file_name(&origin.path)));
            }
            return;
        };
        self.navigate_on(id, conn, &parent, Some(paths::file_name(&path)));
        self.follow(id, &parent);
    }

    pub fn go_back(&self, id: TabId) {
        // A pinned tab's location is fixed, so it has no history to walk.
        if let Some(Some(c)) = self.with_tab(id, |t| (!t.pinned).then(|| t.controller.clone())) {
            c.go_back();
        }
    }

    pub fn go_forward(&self, id: TabId) {
        if let Some(Some(c)) = self.with_tab(id, |t| (!t.pinned).then(|| t.controller.clone())) {
            c.go_forward();
        }
    }

    pub fn reload(&self, id: TabId) {
        if let Some(c) = self.with_tab(id, |t| t.controller.clone()) {
            c.reload();
        }
    }

    /// Show what `query` finds below the tab's folder.
    pub fn search(&self, id: TabId, query: crate::search::Query, use_spotlight: bool) {
        if let Some(c) = self.with_tab(id, |t| t.controller.clone()) {
            c.search(query, use_spotlight);
        }
    }

    /// Show what `query` finds below `path` on `conn`, moving the tab there.
    pub fn search_at(&self, id: TabId, conn: ConnId, path: &str, query: crate::search::Query, use_spotlight: bool) {
        let path = paths::normalize(path, &self.inner.registry.home(conn));
        if let Some(c) = self.with_tab(id, |t| t.controller.clone()) {
            c.search_in(Some(crate::registry::Location { conn, path }), query, use_spotlight);
        }
    }

    /// Stop the tab's running search; its results stay.
    pub fn stop_search(&self, id: TabId) {
        if let Some(c) = self.with_tab(id, |t| t.controller.clone()) {
            c.stop_search();
        }
    }

    /// Leave search results for the folder again.
    pub fn end_search(&self, id: TabId) {
        if let Some(c) = self.with_tab(id, |t| t.controller.clone()) {
            c.end_search();
        }
    }

    pub fn set_view(&self, id: TabId, view: ViewSpec) {
        if let Some(c) = self.with_tab(id, |t| t.controller.clone()) {
            c.set_view(view);
        }
    }

    pub fn view(&self, id: TabId) -> Option<ViewSpec> {
        self.with_tab(id, |t| t.controller.view())
    }

    /// The tab's pane, for the per-folder view commands.
    pub fn controller(&self, id: TabId) -> Option<PaneController> {
        self.with_tab(id, |t| t.controller.clone())
    }

    /// Apply the hidden-files setting to every tab.
    pub fn set_show_hidden(&self, show: bool) {
        let controllers: Vec<PaneController> = {
            let st = self.inner.state.lock().unwrap();
            st.panes.iter().flat_map(|p| p.tabs.iter().map(|t| t.controller.clone())).collect()
        };
        for c in controllers {
            let mut view = c.view();
            if view.show_hidden != show {
                view.show_hidden = show;
                c.set_view(view);
            }
        }
    }

    pub fn remember_selection(&self, id: TabId, name: Option<&str>) {
        if let Some(c) = self.with_tab(id, |t| t.controller.clone()) {
            c.remember_selection(name);
        }
    }

    pub fn snapshot(&self, id: TabId) -> Option<Arc<Snapshot>> {
        self.with_tab(id, |t| t.controller.snapshot()).flatten()
    }

    fn with_tab<R>(&self, id: TabId, f: impl FnOnce(&Tab) -> R) -> Option<R> {
        let st = self.inner.state.lock().unwrap();
        st.tab(id).map(f)
    }

    fn navigate_on(&self, id: TabId, conn: ConnId, path: &str, select: Option<&str>) {
        let path = paths::normalize(path, &self.inner.registry.home(conn));
        let Some((pinned, current_conn, current, controller, pending)) =
            self.with_tab(id, |t| (t.pinned, t.conn, t.path.clone(), t.controller.clone(), !t.pending.is_empty()))
        else {
            return;
        };
        if pending {
            // A restored remote tab now has its connection: it simply
            // goes there, pinned or not.
            self.mutate(|st| match st.tab_mut(id) {
                Some(t) => {
                    t.pending.clear();
                    t.conn = conn;
                    t.path = path.clone();
                    (true, ())
                }
                None => (false, ()),
            });
            controller.navigate_to(conn, &path);
            return;
        }
        if pinned && (path != current || conn != current_conn) {
            let (side, view) = {
                let st = self.inner.state.lock().unwrap();
                let Some((side, _)) = st.locate(id) else {
                    return;
                };
                (side, controller.view())
            };
            let tab = self.new_tab_at(conn, &path, view);
            self.mutate(|st| {
                let pane = &mut st.panes[side.idx()];
                let at = pane.tabs.iter().position(|t| t.id == id).map_or(pane.tabs.len(), |i| i + 1);
                pane.tabs.insert(at, tab);
                pane.selected = at;
                (true, ())
            });
            return;
        }
        match select {
            Some(name) if conn == current_conn => controller.navigate_selecting(&path, name),
            _ => controller.navigate_to(conn, &path),
        }
    }

    /// Sync browsing: mirror a one-step move of tab `id` (into a child, or
    /// up to the parent) in the other pane's current tab.
    fn follow(&self, id: TabId, target: &str) {
        let home = self.with_tab(id, |t| self.inner.registry.home(t.conn)).unwrap_or_default();
        let target = paths::normalize(target, &home);
        let plan = {
            let st = self.inner.state.lock().unwrap();
            if !st.sync_browsing {
                return;
            }
            let Some((side, _)) = st.locate(id) else {
                return;
            };
            if st.current(side).id != id {
                return;
            }
            let from = &st.current(side).path;
            let other = st.current(side.other());
            // Whole path components only, never substring matches.
            let dest = if paths::parent(&target).as_deref() == Some(from.as_str()) {
                Some(paths::join(&other.path, paths::file_name(&target)))
            } else if paths::parent(from).as_deref() == Some(target.as_str()) {
                paths::parent(&other.path)
            } else {
                None
            };
            dest.map(|d| (other.controller.clone(), other.pinned, d))
        };
        // If the counterpart folder does not exist the other pane simply
        // stays where it is, so the failure is not reported.
        if let Some((controller, pinned, dest)) = plan {
            if !pinned {
                controller.navigate_quiet(&dest);
            }
        }
    }

    /// A volume went away (unplugged, or about to be ejected): every tab
    /// on it, pinned ones too, goes to the starting directory. Returns how
    /// many tabs moved.
    pub fn leave_volume(&self, root: &str) -> usize {
        let start = self.inner.store.starting_directory();
        let leaving: Vec<PaneController> = {
            let st = self.inner.state.lock().unwrap();
            st.panes
                .iter()
                .flat_map(|p| p.tabs.iter())
                .filter(|t| t.conn == LOCAL && t.pending.is_empty() && is_within(&t.path, root))
                .map(|t| t.controller.clone())
                .collect()
        };
        for controller in &leaving {
            controller.navigate_to(LOCAL, &start);
        }
        leaving.len()
    }

    /// A tab already showing `path` on `conn`: the active pane's selected
    /// tab first, then its other tabs, then the other pane's.
    pub fn tab_showing(&self, conn: ConnId, path: &str) -> Option<TabId> {
        let st = self.inner.state.lock().unwrap();
        let path = paths::normalize(path, &self.inner.registry.home(conn));
        let order = [st.active, st.active.other()];
        let current = st.current(st.active);
        if current.conn == conn && current.path == path && current.pending.is_empty() {
            return Some(current.id);
        }
        order
            .iter()
            .flat_map(|side| st.panes[side.idx()].tabs.iter())
            .find(|t| t.conn == conn && t.path == path && t.pending.is_empty())
            .map(|t| t.id)
    }

    // --- Session

    /// Snapshot of tabs and layout for restoring later. `ui` is the shell's
    /// own opaque state.
    pub fn saved(&self, ui: &str) -> SavedWindow {
        let start = self.inner.store.starting_directory();
        let st = self.inner.state.lock().unwrap();
        let pane = |side: Side| {
            let p = &st.panes[side.idx()];
            SavedPane {
                tabs: p
                    .tabs
                    .iter()
                    .map(|t| {
                        // A tab on a server the shell described comes back
                        // waiting to reconnect; one on an archive (or an
                        // undescribed server) at the starting directory.
                        let url =
                            if t.pending.is_empty() { self.inner.registry.url(t.conn) } else { t.pending.clone() };
                        let (path, server) = if t.conn == LOCAL && t.pending.is_empty() {
                            (t.path.clone(), String::new())
                        } else if !url.is_empty() {
                            (t.path.clone(), url)
                        } else {
                            (start.clone(), String::new())
                        };
                        SavedTab { path, pinned: t.pinned, view: t.controller.view(), server }
                    })
                    .collect(),
                selected: p.selected,
            }
        };
        SavedWindow {
            left: pane(Side::Left),
            right: pane(Side::Right),
            right_active: st.active == Side::Right,
            single_pane: st.single_pane,
            horizontal: st.horizontal,
            sync_browsing: st.sync_browsing,
            ui: ui.to_string(),
        }
    }
}

/// How a tab starts out.
#[derive(Default)]
struct TabStart {
    pinned: bool,
    /// Coming back from a saved window: keeps its saved view on the first
    /// load, whatever the folder's settings say.
    restored: bool,
    /// A server URL: the tab waits, unloaded, until the shell reconnects it.
    pending: String,
}

fn make_tab(
    window: &Weak<Inner>,
    registry: &Arc<VfsRegistry>,
    store: &Arc<Store>,
    conn: ConnId,
    path: &str,
    view: ViewSpec,
    start: TabStart,
) -> Tab {
    let id = NEXT_TAB_ID.fetch_add(1, Ordering::SeqCst);
    let observer = Arc::new(TabObserver { tab: id, window: window.clone() });
    let controller = PaneController::with_registry(Arc::clone(registry), observer);
    // Folders with no view settings of their own open as a plain list.
    let fallback = FolderView {
        sort: crate::sort::SortSpec { folders_first: store.settings().folders_on_top, ..Default::default() },
        ..Default::default()
    };
    controller.set_folder_views(FolderViews::shared(store.dir()), fallback);
    controller.set_initial_view(view);
    if start.restored {
        controller.keep_view_on_next_visit();
    }
    let pinned = start.pinned;
    if !start.pending.is_empty() {
        let path = paths::normalize(path, "/");
        return Tab { id, controller, pinned, conn: LOCAL, path, pending: start.pending };
    }
    let path = paths::normalize(path, &registry.home(conn));
    controller.navigate_to(conn, &path);
    Tab { id, controller, pinned, conn, path, pending: String::new() }
}

fn is_within(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    root.is_empty() || path == root || path.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::LocalFs;
    use crate::registry::{VfsRegistry, LOCAL};
    use std::fs;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::time::{Duration, Instant};

    struct Chan(Mutex<Sender<(TabId, String)>>);

    impl WindowObserver for Chan {
        fn window_changed(&self, _info: WindowInfo) {}
        fn snapshot_ready(&self, tab: TabId, _snapshot: Arc<Snapshot>, state: PaneState) {
            if state.reason == SnapshotReason::Navigated {
                let _ = self.0.lock().unwrap().send((tab, state.path));
            }
        }
        fn load_failed(&self, tab: TabId, path: String, _error: VfsError) {
            let _ = self.0.lock().unwrap().send((tab, format!("FAILED {path}")));
        }
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: String,
        win: WindowModel,
        rx: Receiver<(TabId, String)>,
        store: Arc<Store>,
        registry: Arc<VfsRegistry>,
    }

    impl Fixture {
        /// Wait until `tab` has loaded `path`.
        fn arrive(&self, tab: TabId, path: &str) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if let Ok((t, p)) = self.rx.recv_timeout(Duration::from_millis(100)) {
                    if t == tab && p == path {
                        return;
                    }
                }
            }
            panic!("tab {tab} never arrived at {path}; now at {:?}", self.path_of(tab));
        }

        fn settle(&self) {
            while self.rx.recv_timeout(Duration::from_millis(250)).is_ok() {}
        }

        /// Wait until the selected tab of `side` shows `path`.
        fn wait_path(&self, side: Side, path: &str) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                if self.cur(side).path == path {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("{side:?} never showed {path}; it shows {}", self.cur(side).path);
        }

        fn path_of(&self, tab: TabId) -> Option<String> {
            self.win.with_tab(tab, |t| t.path.clone())
        }

        fn cur(&self, side: Side) -> TabInfo {
            self.win.info().current(side).clone()
        }

        fn go(&self, side: Side, rel: &str) -> TabId {
            let id = self.cur(side).id;
            let path = format!("{}{rel}", self.root);
            self.win.navigate(id, &path);
            self.arrive(id, &path);
            id
        }
    }

    /// root/{A/{s,only_a}, B/{s}, p/{q}}; home = root.
    fn fixture(restore: Option<SavedWindow>) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap().to_str().unwrap().to_string();
        for d in ["A/s", "A/only_a", "B/s", "p/q", "support"] {
            fs::create_dir_all(format!("{root}/{d}")).unwrap();
        }
        let store = Arc::new(Store::open(format!("{root}/support"), root.clone()));
        let (tx, rx) = channel();
        let registry = Arc::new(VfsRegistry::new(Arc::new(LocalFs), root.clone()));
        let win = WindowModel::new(Arc::clone(&registry), Arc::clone(&store), Arc::new(Chan(Mutex::new(tx))), restore);
        let f = Fixture { _tmp: tmp, root, win, rx, store, registry };
        // Wait for every tab's first listing, however busy the machine is:
        // a late one would otherwise land in the middle of a test.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let info = f.win.info();
            let loaded = info.left.tabs.iter().chain(info.right.tabs.iter()).all(|t| f.win.snapshot(t.id).is_some());
            if loaded || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        f.settle();
        f
    }

    #[test]
    fn spec_pt01_fresh_window_two_panes_at_start_left_active() {
        let f = fixture(None);
        let info = f.win.info();
        assert_eq!(info.active, Side::Left);
        assert!(!info.single_pane && !info.horizontal && !info.sync_browsing);
        assert_eq!(info.left.tabs.len(), 1);
        assert_eq!(info.right.tabs.len(), 1);
        assert_eq!(info.current(Side::Left).path, f.root);
        assert_eq!(info.current(Side::Right).path, f.root);
    }

    #[test]
    fn spec_pt02_single_pane_keeps_hidden_panes_tabs() {
        let f = fixture(None);
        f.go(Side::Right, "/B");
        f.win.new_tab(Side::Right, Some(&format!("{}/p", f.root)));
        let before = f.win.info().right;
        f.win.toggle_single_pane();
        assert!(f.win.info().single_pane);
        f.win.toggle_single_pane();
        let info = f.win.info();
        assert!(!info.single_pane);
        assert_eq!(info.right, before);
    }

    #[test]
    fn spec_pt03_swap_panes_moves_tabs_and_history_and_focus() {
        let f = fixture(None);
        let left = f.go(Side::Left, "/A");
        let right = f.go(Side::Right, "/B");
        f.win.swap_panes();
        let info = f.win.info();
        assert_eq!(info.current(Side::Left).path, format!("{}/B", f.root));
        assert_eq!(info.current(Side::Right).path, format!("{}/A", f.root));
        assert_eq!(info.current(Side::Right).id, left);
        assert_eq!(info.current(Side::Left).id, right);
        assert_eq!(info.active, Side::Right);
        // History moved with the tab: Back in the (now right) tab returns to root.
        f.win.go_back(left);
        f.arrive(left, &f.root);
    }

    #[test]
    fn spec_pt04_clone_pane_shows_active_location_in_other() {
        let f = fixture(None);
        f.go(Side::Left, "/A");
        let right = f.cur(Side::Right).id;
        f.win.clone_pane();
        f.arrive(right, &format!("{}/A", f.root));
    }

    #[test]
    fn spec_pt05_sync_browsing_follows_into_child_and_parent() {
        let f = fixture(None);
        let left = f.go(Side::Left, "/A");
        f.go(Side::Right, "/B");
        f.win.toggle_sync_browsing();

        f.win.navigate(left, &format!("{}/A/s", f.root));
        f.wait_path(Side::Left, &format!("{}/A/s", f.root));
        f.wait_path(Side::Right, &format!("{}/B/s", f.root));

        f.win.go_up(left);
        f.wait_path(Side::Left, &format!("{}/A", f.root));
        f.wait_path(Side::Right, &format!("{}/B", f.root));

        // No counterpart: the other pane stays, and sync stays on.
        f.win.navigate(left, &format!("{}/A/only_a", f.root));
        f.arrive(left, &format!("{}/A/only_a", f.root));
        f.settle();
        assert_eq!(f.cur(Side::Right).path, format!("{}/B", f.root));
        assert!(f.win.info().sync_browsing);
    }

    #[test]
    fn sync_browsing_ignores_jumps_and_is_off_by_default() {
        let f = fixture(None);
        let left = f.go(Side::Left, "/A");
        f.go(Side::Right, "/B");
        f.win.navigate(left, &format!("{}/A/s", f.root));
        f.arrive(left, &format!("{}/A/s", f.root));
        f.settle();
        assert_eq!(f.cur(Side::Right).path, format!("{}/B", f.root));

        f.win.toggle_sync_browsing();
        // A jump (not a child or the parent) is not mirrored.
        f.win.navigate(left, &format!("{}/p/q", f.root));
        f.arrive(left, &format!("{}/p/q", f.root));
        f.settle();
        assert_eq!(f.cur(Side::Right).path, format!("{}/B", f.root));
    }

    #[test]
    fn spec_pt06_pinned_tab_survives_close_and_close_others() {
        let f = fixture(None);
        let pinned = f.cur(Side::Left).id;
        f.win.set_pinned(pinned, true);
        assert_eq!(f.win.close_tab(pinned), CloseOutcome::Pinned);

        let second = f.win.new_tab(Side::Left, None);
        let third = f.win.new_tab(Side::Left, None);
        f.win.close_other_tabs(Side::Left);
        let ids: Vec<_> = f.win.info().left.tabs.iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![pinned, third]);
        assert!(!ids.contains(&second));

        f.win.set_pinned(pinned, false);
        assert_eq!(f.win.close_tab(pinned), CloseOutcome::Closed);
    }

    #[test]
    fn navigating_a_pinned_tab_opens_a_new_tab_beside_it() {
        let f = fixture(None);
        let pinned = f.go(Side::Left, "/p");
        f.win.set_pinned(pinned, true);
        f.win.navigate(pinned, &format!("{}/p/q", f.root));
        let info = f.win.info();
        assert_eq!(info.left.tabs.len(), 2);
        assert_eq!(info.left.tabs[0].path, format!("{}/p", f.root));
        assert!(info.left.tabs[0].pinned);
        assert_eq!(info.left.selected, 1);
        f.arrive(info.left.tabs[1].id, &format!("{}/p/q", f.root));
        // Navigating to its own location is not "away".
        f.win.navigate(pinned, &format!("{}/p", f.root));
        assert_eq!(f.win.info().left.tabs.len(), 2);
    }

    #[test]
    fn new_tab_opens_beside_current_at_same_location() {
        let f = fixture(None);
        f.go(Side::Left, "/A");
        let first = f.cur(Side::Left).id;
        let b = f.win.new_tab(Side::Left, None);
        f.win.select_tab(first);
        let c = f.win.new_tab(Side::Left, None);
        let info = f.win.info();
        let ids: Vec<_> = info.left.tabs.iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![first, c, b]);
        assert_eq!(info.left.selected, 1);
        assert_eq!(info.left.tabs[1].path, format!("{}/A", f.root));
    }

    #[test]
    fn closing_tabs_selects_a_neighbour_and_last_tab_closes_window() {
        let f = fixture(None);
        let a = f.cur(Side::Left).id;
        let b = f.win.new_tab(Side::Left, None);
        let c = f.win.new_tab(Side::Left, None);
        assert_eq!(f.win.close_tab(c), CloseOutcome::Closed); // last: select previous
        assert_eq!(f.cur(Side::Left).id, b);
        f.win.select_tab(a);
        assert_eq!(f.win.close_tab(a), CloseOutcome::Closed); // first: select next
        assert_eq!(f.cur(Side::Left).id, b);
        assert_eq!(f.win.close_tab(b), CloseOutcome::LastTab);
        assert_eq!(f.win.info().left.tabs.len(), 1);
        assert_eq!(f.win.close_tab(9_999_999), CloseOutcome::Missing);
    }

    #[test]
    fn closing_a_tab_before_the_selection_keeps_the_selection() {
        let f = fixture(None);
        let a = f.cur(Side::Left).id;
        let _b = f.win.new_tab(Side::Left, None);
        let c = f.win.new_tab(Side::Left, None);
        assert_eq!(f.win.close_tab(a), CloseOutcome::Closed);
        assert_eq!(f.cur(Side::Left).id, c);
    }

    #[test]
    fn tab_selection_by_index_and_cycling_wraps() {
        let f = fixture(None);
        let a = f.cur(Side::Left).id;
        let b = f.win.new_tab(Side::Left, None);
        let c = f.win.new_tab(Side::Left, None);
        f.win.select_tab_index(Side::Left, 0);
        assert_eq!(f.cur(Side::Left).id, a);
        f.win.select_tab_index(Side::Left, 7); // no such tab: ignored
        assert_eq!(f.cur(Side::Left).id, a);
        f.win.cycle_tab(Side::Left, -1);
        assert_eq!(f.cur(Side::Left).id, c);
        f.win.cycle_tab(Side::Left, 1);
        assert_eq!(f.cur(Side::Left).id, a);
        f.win.cycle_tab(Side::Left, 1);
        assert_eq!(f.cur(Side::Left).id, b);
    }

    #[test]
    fn move_tab_to_other_pane_appends_selects_and_never_empties_source() {
        let f = fixture(None);
        f.go(Side::Left, "/A");
        let only = f.cur(Side::Left).id;
        f.win.move_tab_to_other_pane(only);
        let info = f.win.info();
        assert_eq!(info.right.tabs.len(), 2);
        assert_eq!(info.right.tabs[1].id, only);
        assert_eq!(info.right.selected, 1);
        // Source got a fresh tab at the same place.
        assert_eq!(info.left.tabs.len(), 1);
        assert_ne!(info.left.tabs[0].id, only);
        assert_eq!(info.left.tabs[0].path, format!("{}/A", f.root));
    }

    #[test]
    fn move_tab_reorders_within_a_pane() {
        let f = fixture(None);
        let a = f.cur(Side::Left).id;
        let b = f.win.new_tab(Side::Left, None);
        let c = f.win.new_tab(Side::Left, None);
        f.win.move_tab(c, Side::Left, 0);
        let info = f.win.info();
        let ids: Vec<_> = info.left.tabs.iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![c, a, b]);
        assert_eq!(info.left.selected, 0);
    }

    #[test]
    fn spec_pt11_go_up_selects_folder_left_and_switch_active_respects_single_pane() {
        let f = fixture(None);
        let left = f.go(Side::Left, "/A/s");
        f.win.go_up(left);
        f.arrive(left, &format!("{}/A", f.root));

        f.win.switch_active();
        assert_eq!(f.win.info().active, Side::Right);
        f.win.toggle_single_pane();
        f.win.switch_active();
        assert_eq!(f.win.info().active, Side::Right);
    }

    #[test]
    fn visits_are_recorded_as_recent_folders() {
        let f = fixture(None);
        f.go(Side::Left, "/A");
        f.go(Side::Left, "/B");
        let recents = f.store.recent_folders(&LocalFs);
        assert_eq!(recents[0], format!("{}/B", f.root));
        assert_eq!(recents[1], format!("{}/A", f.root));
    }

    #[test]
    fn spec_pt14_window_restores_tabs_pins_layout_and_views() {
        let f = fixture(None);
        f.go(Side::Left, "/A");
        let pinned = f.cur(Side::Left).id;
        f.win.set_pinned(pinned, true);
        f.win.new_tab(Side::Left, Some(&format!("{}/B", f.root)));
        f.go(Side::Right, "/p");
        let right = f.cur(Side::Right).id;
        let mut view = f.win.view(right).unwrap();
        view.sort.key = crate::sort::SortKey::Size;
        view.sort.ascending = false;
        f.win.set_view(right, view.clone());
        f.win.set_active(Side::Right);
        f.win.set_horizontal(true);
        f.win.toggle_sync_browsing();
        let saved = f.win.saved("ui-blob");

        // Through JSON, as on a real relaunch.
        let json = serde_json::to_string(&saved).unwrap();
        let restored = fixture(Some(serde_json::from_str(&json).unwrap()));
        let info = restored.win.info();
        // Paths were saved from another temp root, so compare shapes.
        assert_eq!(info.left.tabs.len(), 2);
        assert!(info.left.tabs[0].pinned && !info.left.tabs[1].pinned);
        assert_eq!(info.left.selected, 1);
        assert_eq!(info.left.tabs[0].path, format!("{}/A", f.root));
        assert_eq!(info.active, Side::Right);
        assert!(info.horizontal && info.sync_browsing && !info.single_pane);
        let rv = restored.win.view(info.current(Side::Right).id).unwrap();
        assert_eq!(rv.sort, view.sort);
        assert_eq!(restored.win.saved("x").ui, "x");
    }

    #[test]
    fn show_hidden_applies_to_every_tab() {
        let f = fixture(None);
        let extra = f.win.new_tab(Side::Right, None);
        f.win.set_show_hidden(true);
        let info = f.win.info();
        for t in info.left.tabs.iter().chain(info.right.tabs.iter()) {
            assert!(f.win.view(t.id).unwrap().show_hidden, "tab {}", t.id);
        }
        assert!(f.win.view(extra).unwrap().show_hidden);
    }

    #[test]
    fn a_tab_can_go_to_another_connection_and_back() {
        let f = fixture(None);
        // A second "server": the same disk under another connection id.
        let server = f.registry.add(Arc::new(LocalFs), "bob@example.org", format!("{}/B", f.root));
        let tab = f.cur(Side::Left).id;
        f.win.navigate_to(tab, server, "~");
        f.arrive(tab, &format!("{}/B", f.root));
        let info = f.win.info();
        assert_eq!(info.current(Side::Left).conn, server);
        assert_eq!(info.current(Side::Left).label, "bob@example.org");
        assert_eq!(info.current(Side::Right).conn, LOCAL);
        assert_eq!(info.current(Side::Right).label, "");

        // Navigating by path stays on the server; a new tab starts there too.
        f.win.navigate(tab, &format!("{}/B/s", f.root));
        f.arrive(tab, &format!("{}/B/s", f.root));
        assert_eq!(f.cur(Side::Left).conn, server);
        let second = f.win.new_tab(Side::Left, None);
        f.arrive(second, &format!("{}/B/s", f.root));
        assert_eq!(f.cur(Side::Left).conn, server);
        // Remote folders are not Recent Folders.
        assert!(!f.store.recent_folders(&LocalFs).contains(&format!("{}/B/s", f.root)));
        // A remote tab is saved as the starting directory.
        assert_eq!(f.win.saved("").left.tabs[0].path, f.root);

        // Back leaves the server for the local folder the tab came from.
        f.win.select_tab(tab);
        f.win.go_back(tab);
        f.settle();
        f.win.go_back(tab);
        f.arrive(tab, &f.root);
        assert_eq!(f.win.info().left.tabs[0].conn, LOCAL);
    }

    #[test]
    fn remote_tabs_come_back_waiting_and_reconnect_when_told() {
        let f = fixture(None);
        let server = f.registry.add(Arc::new(LocalFs), "bob@host", format!("{}/B", f.root));
        f.registry.set_url(server, "sftp://bob@host");
        let tab = f.cur(Side::Left).id;
        f.win.navigate_to(tab, server, "~/s");
        f.arrive(tab, &format!("{}/B/s", f.root));
        f.win.set_pinned(tab, true);
        // Visits on a server are its own Recent Folders.
        assert_eq!(f.store.remote_recents("sftp://bob@host"), [format!("{}/B/s", f.root)]);
        let saved = f.win.saved("");
        assert_eq!(saved.left.tabs[0].server, "sftp://bob@host");
        assert_eq!(saved.left.tabs[0].path, format!("{}/B/s", f.root));
        assert_eq!(saved.right.tabs[0].server, "");

        // Relaunch: the tab waits, unloaded, with the server named.
        let g = fixture(Some(saved.clone()));
        let waiting = g.cur(Side::Left);
        assert_eq!((waiting.pending.as_str(), waiting.label.as_str()), ("sftp://bob@host", "sftp://bob@host"));
        assert!(g.win.snapshot(waiting.id).is_none(), "nothing is loaded before reconnecting");
        assert!(waiting.pinned);
        // Saving again keeps it as it was.
        assert_eq!(g.win.saved("").left.tabs[0], saved.left.tabs[0]);
        // Clone Pane has nothing to copy from it yet.
        g.win.clone_pane();
        assert_eq!(g.cur(Side::Right).path, saved.right.tabs[0].path);
        // Reconnected: the pinned tab goes there itself, no extra tab.
        let again = g.registry.add(Arc::new(LocalFs), "bob@host", format!("{}/B", g.root));
        g.win.navigate_to(waiting.id, again, &format!("{}/B/s", g.root));
        g.arrive(waiting.id, &format!("{}/B/s", g.root));
        let info = g.win.info();
        assert_eq!(info.left.tabs.len(), 1);
        assert_eq!((info.left.tabs[0].conn, info.left.tabs[0].pending.as_str()), (again, ""));
    }

    #[test]
    fn pt15_tabs_on_a_vanished_volume_go_to_the_starting_directory() {
        let f = fixture(None);
        let left = f.go(Side::Left, "/A/s");
        f.win.set_pinned(left, true);
        let right = f.go(Side::Right, "/B");
        let extra = f.win.new_tab(Side::Right, Some(&format!("{}/A", f.root)));
        f.arrive(extra, &format!("{}/A", f.root));
        assert_eq!(f.win.leave_volume(&format!("{}/A", f.root)), 2);
        // Both move; their arrivals may come in either order.
        let deadline = Instant::now() + Duration::from_secs(10);
        while [left, extra].iter().any(|t| f.path_of(*t).as_deref() != Some(f.root.as_str())) {
            assert!(Instant::now() < deadline, "tabs did not leave the volume");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(f.path_of(right).unwrap(), format!("{}/B", f.root));
        assert_eq!(f.win.info().left.tabs.len(), 1, "a pinned tab moves too, without a new tab");
        assert_eq!(f.win.leave_volume("/nowhere"), 0);
    }

    #[test]
    fn pt16_an_open_location_is_found_active_pane_first() {
        let f = fixture(None);
        let right = f.go(Side::Right, "/B");
        assert_eq!(f.win.tab_showing(LOCAL, &format!("{}/B/", f.root)), Some(right));
        assert_eq!(f.win.tab_showing(LOCAL, &f.root), Some(f.cur(Side::Left).id));
        assert_eq!(f.win.tab_showing(LOCAL, &format!("{}/p", f.root)), None);
        assert_eq!(f.win.tab_showing(7, &format!("{}/B", f.root)), None);
    }

    #[test]
    fn closing_a_connection_sends_its_tabs_to_the_starting_directory() {
        let f = fixture(None);
        let server = f.registry.add(Arc::new(LocalFs), "srv", format!("{}/A", f.root));
        let tab = f.cur(Side::Left).id;
        f.win.navigate_to(tab, server, "~");
        f.arrive(tab, &format!("{}/A", f.root));
        let other = f.go(Side::Right, "/p");

        f.registry.remove(server);
        f.win.connection_closed(server);
        f.arrive(tab, &f.root);
        let info = f.win.info();
        assert_eq!(info.current(Side::Left).conn, LOCAL);
        // The other pane was not on that server and stays put.
        assert_eq!(f.path_of(other).unwrap(), format!("{}/p", f.root));
        // Nothing of the closed connection is left to go back to.
        f.win.go_back(tab);
        f.settle();
        assert_eq!(f.cur(Side::Left).conn, LOCAL);
    }
}
