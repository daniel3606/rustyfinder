//! UniFFI surface for the Swift app.
//!
//! Rules of this layer:
//! - Calls are coarse: Swift asks for whole rows, never field by field.
//! - Nothing here blocks the caller. Work runs on Rust threads and reports
//!   back through `PaneListener`, which Swift must hop to the main actor.
//! - No logic lives here. It only converts between core and FFI types.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

mod columns;
mod views;

use rustyfinder_core::pane::SnapshotReason;
use rustyfinder_core::{
    paths, ActivityInfo, ActivityManager, ActivityObserver, ActivityResult, ActivityState, CloseOutcome,
    ConflictAnswer, ConflictChoice, ConflictKind, Entry, EntryKind, ErrorChoice, FavoriteError, FinishReport, GroupKey,
    Layout, LocalFs, PaneState, Platform, Prompt, Request, SavedWindow, Side, Snapshot, SortKey, SortSpec, Store,
    TransferMode, Vfs, VfsError, VfsRegistry, ViewSpec, WindowInfo, WindowModel, WindowObserver,
};

uniffi::setup_scaffolding!();

mod area;
mod local_ops;
mod places;
pub use places::*;

/// Version of the Rust core.
#[uniffi::export]
pub fn core_version() -> String {
    rustyfinder_core::version().to_string()
}

/// Ancestors of `path` from the root down, for the path bar.
#[uniffi::export]
pub fn path_components(path: String) -> Vec<PathComponent> {
    paths::components(&path).into_iter().map(|(label, path)| PathComponent { label, path }).collect()
}

/// Permission bits as octal text: `644`, or `4755` with a special bit.
#[uniffi::export]
pub fn permission_octal(mode: u32) -> String {
    rustyfinder_core::perm::octal(mode)
}

/// Parse `644` or `4755`; `None` if the text is not valid octal permissions.
#[uniffi::export]
pub fn parse_permission_octal(text: String) -> Option<u32> {
    rustyfinder_core::perm::parse_octal(&text)
}

/// Receives the result of `RustyFinderApp::measure`. Called on a worker thread.
#[uniffi::export(with_foreign)]
pub trait MeasureListener: Send + Sync {
    /// `items` counts the folder itself and everything in it.
    fn measured(&self, path: String, items: u64, bytes: u64);
}

/// A guess at how a text file is encoded.
#[derive(Debug, Clone, uniffi::Record)]
pub struct TextEncodingGuess {
    /// IANA charset name, e.g. "UTF-8", "Shift_JIS", "macintosh".
    pub name: String,
    /// Bytes of byte-order mark to skip before decoding.
    pub bom_length: u32,
    /// False if the bytes look like binary data.
    pub is_text: bool,
}

/// Guess the encoding of a file's bytes (the whole file or its start).
#[uniffi::export]
pub fn detect_text_encoding(bytes: Vec<u8>) -> TextEncodingGuess {
    let guess = rustyfinder_core::text::detect(&bytes);
    TextEncodingGuess { name: guess.name, bom_length: guess.bom_length as u32, is_text: guess.is_text }
}

/// A path escaped so a shell takes it as one argument (Copy Path → Terminal).
#[uniffi::export]
pub fn shell_escape(path: String) -> String {
    paths::shell_escape(&path)
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PathComponent {
    pub label: String,
    pub path: String,
}

#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum RustyFinderError {
    #[error("{message}")]
    NotFound { message: String },
    #[error("{message}")]
    PermissionDenied { message: String },
    #[error("{message}")]
    NotADirectory { message: String },
    #[error("{message}")]
    AlreadyExists { message: String },
    #[error("{message}")]
    Cancelled { message: String },
    #[error("{message}")]
    Unsupported { message: String },
    #[error("{message}")]
    Io { message: String },
    #[error("{message}")]
    ReadOnly { message: String },
    #[error("{message}")]
    NoSpace { message: String },
    /// A rule was broken (e.g. a duplicate favorite name); `message` says which.
    #[error("{message}")]
    Invalid { message: String },
}

/// Lets a Swift callback that throws something unexpected surface as an
/// ordinary error instead of a panic.
impl From<uniffi::UnexpectedUniFFICallbackError> for RustyFinderError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        RustyFinderError::Io { message: e.reason }
    }
}

impl From<FavoriteError> for RustyFinderError {
    fn from(e: FavoriteError) -> Self {
        RustyFinderError::Invalid { message: e.to_string() }
    }
}

impl From<VfsError> for RustyFinderError {
    fn from(e: VfsError) -> Self {
        let message = e.to_string();
        match e {
            VfsError::NotFound(_) => RustyFinderError::NotFound { message },
            VfsError::PermissionDenied(_) => RustyFinderError::PermissionDenied { message },
            VfsError::NotADirectory(_) => RustyFinderError::NotADirectory { message },
            VfsError::AlreadyExists(_) => RustyFinderError::AlreadyExists { message },
            VfsError::Cancelled => RustyFinderError::Cancelled { message },
            VfsError::Unsupported(_) => RustyFinderError::Unsupported { message },
            VfsError::ReadOnly => RustyFinderError::ReadOnly { message },
            VfsError::NoSpace => RustyFinderError::NoSpace { message },
            VfsError::Invalid(_) => RustyFinderError::Invalid { message },
            VfsError::Io { .. } | VfsError::Throttled { .. } | VfsError::CrossDevice(_) | VfsError::NotEmpty(_) => {
                RustyFinderError::Io { message }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RowKind {
    File,
    Directory,
    Symlink,
    Other,
}

/// One table row.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Row {
    pub name: String,
    pub kind: RowKind,
    /// Folder, or symlink to a folder.
    pub is_dir: bool,
    pub size: u64,
    pub modified_ms: Option<i64>,
    pub created_ms: Option<i64>,
    pub is_hidden: bool,
    pub is_broken_link: bool,
    /// Lower-cased, no dot, "" for folders.
    pub extension: String,
    pub link_target: Option<String>,
    pub mode: Option<u32>,
    pub owner: Option<String>,
    /// Locked in Finder.
    pub locked: bool,
    pub group: Option<String>,
    /// Only in the cloud (iCloud Drive): reading it would download it.
    pub cloud_only: bool,
}

impl From<&Entry> for Row {
    fn from(e: &Entry) -> Self {
        Row {
            name: e.name.clone(),
            kind: match e.kind {
                EntryKind::File => RowKind::File,
                EntryKind::Directory => RowKind::Directory,
                EntryKind::Symlink => RowKind::Symlink,
                EntryKind::Other => RowKind::Other,
            },
            is_dir: e.is_dir,
            size: e.size,
            modified_ms: e.modified_ms,
            created_ms: e.created_ms,
            is_hidden: e.is_hidden,
            is_broken_link: e.is_broken_link,
            extension: e.extension(),
            link_target: e.link_target.clone(),
            mode: e.mode,
            owner: e.owner_name(),
            locked: e.ext.locked,
            group: e.group_name(),
            cloud_only: e.ext.cloud_only,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SortColumn {
    Name,
    Size,
    Modified,
    Created,
    Kind,
    Added,
    Tags,
    Owner,
    Group,
    Permissions,
}

/// What a list is grouped by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum GroupBy {
    None,
    Kind,
    Added,
    Modified,
    Created,
    Size,
    Tags,
}

impl From<GroupBy> for GroupKey {
    fn from(g: GroupBy) -> Self {
        match g {
            GroupBy::None => GroupKey::None,
            GroupBy::Kind => GroupKey::Kind,
            GroupBy::Added => GroupKey::Added,
            GroupBy::Modified => GroupKey::Modified,
            GroupBy::Created => GroupKey::Created,
            GroupBy::Size => GroupKey::Size,
            GroupBy::Tags => GroupKey::Tags,
        }
    }
}

impl From<GroupKey> for GroupBy {
    fn from(g: GroupKey) -> Self {
        match g {
            GroupKey::None => GroupBy::None,
            GroupKey::Kind => GroupBy::Kind,
            GroupKey::Added => GroupBy::Added,
            GroupKey::Modified => GroupBy::Modified,
            GroupKey::Created => GroupBy::Created,
            GroupKey::Size => GroupBy::Size,
            GroupKey::Tags => GroupBy::Tags,
        }
    }
}

/// A group header in a grouped list.
#[derive(Debug, Clone, uniffi::Record)]
pub struct RowGroup {
    pub title: String,
    /// First row of the group.
    pub start: u32,
    /// Kind groups of files: the extension, for the app to name the kind.
    pub ext: Option<String>,
}

/// How a tab lays its items out: icon grid, list, or columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ViewLayout {
    Icons,
    List,
    Columns,
}

/// Sort, layout, hidden-file and filter settings for a tab.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ViewOptions {
    pub layout: ViewLayout,
    pub sort: SortColumn,
    pub ascending: bool,
    pub group: GroupBy,
    pub folders_by_name: bool,
    pub folders_first: bool,
    pub show_hidden: bool,
    pub filter: String,
}

impl From<ViewOptions> for ViewSpec {
    fn from(v: ViewOptions) -> Self {
        ViewSpec {
            sort: SortSpec {
                key: match v.sort {
                    SortColumn::Name => SortKey::Name,
                    SortColumn::Size => SortKey::Size,
                    SortColumn::Modified => SortKey::Modified,
                    SortColumn::Created => SortKey::Created,
                    SortColumn::Kind => SortKey::Kind,
                    SortColumn::Added => SortKey::Added,
                    SortColumn::Tags => SortKey::Tags,
                    SortColumn::Owner => SortKey::Owner,
                    SortColumn::Group => SortKey::Group,
                    SortColumn::Permissions => SortKey::Permissions,
                },
                ascending: v.ascending,
                folders_first: v.folders_first,
            },
            group: v.group.into(),
            folders_by_name: v.folders_by_name,
            layout: match v.layout {
                ViewLayout::Icons => Layout::Icons,
                ViewLayout::List => Layout::List,
                ViewLayout::Columns => Layout::Columns,
            },
            show_hidden: v.show_hidden,
            filter: v.filter,
        }
    }
}

impl From<ViewSpec> for ViewOptions {
    fn from(v: ViewSpec) -> Self {
        ViewOptions {
            layout: match v.layout {
                Layout::Icons => ViewLayout::Icons,
                Layout::List => ViewLayout::List,
                Layout::Columns => ViewLayout::Columns,
            },
            sort: match v.sort.key {
                SortKey::Name => SortColumn::Name,
                SortKey::Size => SortColumn::Size,
                SortKey::Modified => SortColumn::Modified,
                SortKey::Created => SortColumn::Created,
                SortKey::Kind => SortColumn::Kind,
                SortKey::Added => SortColumn::Added,
                SortKey::Tags => SortColumn::Tags,
                SortKey::Owner => SortColumn::Owner,
                SortKey::Group => SortColumn::Group,
                SortKey::Permissions => SortColumn::Permissions,
            },
            ascending: v.sort.ascending,
            group: v.group.into(),
            folders_by_name: v.folders_by_name,
            folders_first: v.sort.folders_first,
            show_hidden: v.show_hidden,
            filter: v.filter,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ChangeReason {
    /// Different folder: reset selection and scroll.
    Navigated,
    /// Same folder re-read: keep selection by name.
    Reloaded,
    /// Same contents re-sorted or filtered: keep selection by name.
    ViewChanged,
}

/// Chrome state delivered with every snapshot.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PaneInfo {
    /// Connection the folder is on; 0 is the local disk.
    pub conn: u64,
    pub path: String,
    pub reason: ChangeReason,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub can_go_up: bool,
    /// Row to select once shown (e.g. the folder we came up out of).
    pub select_name: Option<String>,
    /// Entries in the folder including ones hidden by the view.
    pub total_count: u32,
    /// Set while the tab shows search results. Each row's name is then a
    /// path relative to `path`.
    pub search: Option<SearchInfo>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SearchInfo {
    pub term: String,
    /// Results are still arriving.
    pub running: bool,
    /// Something to tell the user (files skipped, engine switched).
    pub note: Option<String>,
}

/// Immutable sorted view of one folder. Swift pulls rows by index.
#[derive(Debug, uniffi::Object)]
pub struct SnapshotHandle {
    inner: Arc<Snapshot>,
}

#[uniffi::export]
impl SnapshotHandle {
    pub fn path(&self) -> String {
        self.inner.path().to_string()
    }

    pub fn count(&self) -> u32 {
        self.inner.len() as u32
    }

    /// Group headers in row order; empty when the list isn't grouped.
    pub fn groups(&self) -> Vec<RowGroup> {
        self.inner
            .groups()
            .iter()
            .map(|g| RowGroup { title: g.title.clone(), start: g.start, ext: g.ext.clone() })
            .collect()
    }

    pub fn row(&self, index: u32) -> Option<Row> {
        self.inner.get(index as usize).map(Row::from)
    }

    /// Up to `count` rows starting at `start`; fewer at the end.
    pub fn rows(&self, start: u32, count: u32) -> Vec<Row> {
        (start..start.saturating_add(count)).map_while(|i| self.inner.get(i as usize)).map(Row::from).collect()
    }

    /// Absolute path of the row.
    pub fn path_of(&self, index: u32) -> Option<String> {
        self.inner.get(index as usize).map(|e| paths::join(self.inner.path(), &e.name))
    }

    pub fn index_of(&self, name: String) -> Option<u32> {
        self.inner.index_of(&name).map(|i| i as u32)
    }

    /// Rows of the named entries, ascending. Names not shown are skipped.
    pub fn indices_of(&self, names: Vec<String>) -> Vec<u32> {
        self.inner.indices_of(&names).into_iter().map(|i| i as u32).collect()
    }

    /// Rows whose names match a Quick Select pattern: plain text
    /// ("contains", ignoring case and accents), `.ext` / `*.ext`, or a
    /// wildcard with `*` and `?`.
    pub fn match_pattern(&self, pattern: String) -> Vec<u32> {
        let pattern = rustyfinder_core::fuzzy::Pattern::parse(&pattern);
        self.inner
            .iter()
            .enumerate()
            .filter(|(_, entry)| pattern.matches(&entry.name))
            .map(|(row, _)| row as u32)
            .collect()
    }

    /// Combined size in bytes of the given rows (folders count as 0).
    pub fn total_size(&self, rows: Vec<u32>) -> u64 {
        self.inner.total_size(&rows)
    }

    /// Type-to-select: next row at or after `start` whose name starts with
    /// `prefix` (case-insensitive, wraps around).
    pub fn find_prefix(&self, prefix: String, start: u32) -> Option<u32> {
        self.inner.find_prefix(&prefix, start as usize).map(|i| i as u32)
    }
}

fn pane_info(snapshot: &Snapshot, state: PaneState) -> PaneInfo {
    PaneInfo {
        conn: state.conn,
        path: state.path,
        reason: match state.reason {
            SnapshotReason::Navigated => ChangeReason::Navigated,
            SnapshotReason::Reloaded => ChangeReason::Reloaded,
            SnapshotReason::ViewChanged => ChangeReason::ViewChanged,
        },
        can_go_back: state.can_go_back,
        can_go_forward: state.can_go_forward,
        can_go_up: state.can_go_up,
        select_name: state.select_name,
        total_count: snapshot.total_len() as u32,
        search: state.search.map(|s| SearchInfo { term: s.term, running: s.running, note: s.note }),
    }
}

// ---------------------------------------------------------------- Window

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PaneSide {
    Left,
    Right,
}

impl From<PaneSide> for Side {
    fn from(s: PaneSide) -> Self {
        match s {
            PaneSide::Left => Side::Left,
            PaneSide::Right => Side::Right,
        }
    }
}

impl From<Side> for PaneSide {
    fn from(s: Side) -> Self {
        match s {
            Side::Left => PaneSide::Left,
            Side::Right => PaneSide::Right,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct TabRecord {
    pub id: u64,
    /// Connection the tab shows; 0 is the local disk.
    pub conn: u64,
    /// Name of the remote connection ("bob@example.org"), "" for local.
    pub label: String,
    pub path: String,
    pub pinned: bool,
    /// A restored tab waiting to reconnect: the server's URL ("" otherwise).
    pub pending: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PaneTabsRecord {
    pub tabs: Vec<TabRecord>,
    pub selected: u32,
}

/// Structure of a window: which tabs each pane has and how panes are shown.
#[derive(Debug, Clone, uniffi::Record)]
pub struct WindowLayout {
    pub left: PaneTabsRecord,
    pub right: PaneTabsRecord,
    pub active: PaneSide,
    pub single_pane: bool,
    pub horizontal: bool,
    pub sync_browsing: bool,
}

impl From<WindowInfo> for WindowLayout {
    fn from(info: WindowInfo) -> Self {
        let pane = |p: rustyfinder_core::PaneTabs| PaneTabsRecord {
            tabs: p
                .tabs
                .into_iter()
                .map(|t| TabRecord {
                    id: t.id,
                    conn: t.conn,
                    label: t.label,
                    path: t.path,
                    pinned: t.pinned,
                    pending: t.pending,
                })
                .collect(),
            selected: p.selected as u32,
        };
        WindowLayout {
            left: pane(info.left),
            right: pane(info.right),
            active: info.active.into(),
            single_pane: info.single_pane,
            horizontal: info.horizontal,
            sync_browsing: info.sync_browsing,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CloseResult {
    Closed,
    /// Pinned tabs stay until unpinned.
    Pinned,
    /// The pane's last tab: close the window instead.
    LastTab,
    Missing,
}

/// Implemented in Swift. Called on Rust worker threads.
#[uniffi::export(with_foreign)]
pub trait WindowListener: Send + Sync {
    /// Tabs, focus or layout changed.
    fn window_changed(&self, layout: WindowLayout);
    fn snapshot_ready(&self, tab: u64, snapshot: Arc<SnapshotHandle>, info: PaneInfo);
    fn load_failed(&self, tab: u64, path: String, error: RustyFinderError);
}

struct WindowAdapter(Arc<dyn WindowListener>);

impl WindowObserver for WindowAdapter {
    fn window_changed(&self, info: WindowInfo) {
        self.0.window_changed(info.into());
    }

    fn snapshot_ready(&self, tab: u64, snapshot: Arc<Snapshot>, state: PaneState) {
        let info = pane_info(&snapshot, state);
        self.0.snapshot_ready(tab, Arc::new(SnapshotHandle { inner: snapshot }), info);
    }

    fn load_failed(&self, tab: u64, path: String, error: VfsError) {
        self.0.load_failed(tab, path, error.into());
    }
}

static NEXT_WINDOW_ID: AtomicU64 = AtomicU64::new(1);

/// One browser window: two panes of tabs.
#[derive(uniffi::Object)]
pub struct WindowSession {
    id: u64,
    model: WindowModel,
}

#[uniffi::export]
impl WindowSession {
    /// Identifies this window to the activity queue and the undo stack.
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn layout(&self) -> WindowLayout {
        self.model.info().into()
    }

    pub fn set_active(&self, side: PaneSide) {
        self.model.set_active(side.into());
    }

    pub fn switch_active(&self) {
        self.model.switch_active();
    }

    pub fn toggle_single_pane(&self) {
        self.model.toggle_single_pane();
    }

    pub fn set_horizontal(&self, horizontal: bool) {
        self.model.set_horizontal(horizontal);
    }

    pub fn toggle_sync_browsing(&self) {
        self.model.toggle_sync_browsing();
    }

    pub fn swap_panes(&self) {
        self.model.swap_panes();
    }

    pub fn clone_pane(&self) {
        self.model.clone_pane();
    }

    /// New tab beside the current one; `path` defaults to its location.
    pub fn new_tab(&self, side: PaneSide, path: Option<String>) -> u64 {
        self.model.new_tab(side.into(), path.as_deref())
    }

    pub fn close_tab(&self, tab: u64) -> CloseResult {
        match self.model.close_tab(tab) {
            CloseOutcome::Closed => CloseResult::Closed,
            CloseOutcome::Pinned => CloseResult::Pinned,
            CloseOutcome::LastTab => CloseResult::LastTab,
            CloseOutcome::Missing => CloseResult::Missing,
        }
    }

    pub fn close_other_tabs(&self, side: PaneSide) {
        self.model.close_other_tabs(side.into());
    }

    pub fn select_tab(&self, tab: u64) {
        self.model.select_tab(tab);
    }

    pub fn select_tab_index(&self, side: PaneSide, index: u32) {
        self.model.select_tab_index(side.into(), index as usize);
    }

    pub fn cycle_tab(&self, side: PaneSide, delta: i32) {
        self.model.cycle_tab(side.into(), delta);
    }

    pub fn move_tab(&self, tab: u64, side: PaneSide, index: u32) {
        self.model.move_tab(tab, side.into(), index as usize);
    }

    pub fn move_tab_to_other_pane(&self, tab: u64) {
        self.model.move_tab_to_other_pane(tab);
    }

    pub fn set_pinned(&self, tab: u64, pinned: bool) {
        self.model.set_pinned(tab, pinned);
    }

    pub fn navigate(&self, tab: u64, path: String) {
        self.model.navigate(tab, &path);
    }

    /// Go to `path` on a given connection (0 = the local disk). `~` is the
    /// home folder on that connection.
    pub fn navigate_to(&self, tab: u64, conn: u64, path: String) {
        self.model.navigate_to(tab, conn, &path);
    }

    /// A connection closed: its tabs go to the starting directory.
    pub fn connection_closed(&self, conn: u64) {
        self.model.connection_closed(conn);
    }

    pub fn go_up(&self, tab: u64) {
        self.model.go_up(tab);
    }

    pub fn go_back(&self, tab: u64) {
        self.model.go_back(tab);
    }

    pub fn go_forward(&self, tab: u64) {
        self.model.go_forward(tab);
    }

    pub fn reload(&self, tab: u64) {
        self.model.reload(tab);
    }

    /// Search below the tab's folder. `query_json` is a
    /// `rustyfinder_core::search::Query`. Results replace the listing and keep
    /// arriving until the search is done. Returns false if the query
    /// cannot be read.
    pub fn search(&self, tab: u64, query_json: String, use_spotlight: bool) -> bool {
        match serde_json::from_str::<rustyfinder_core::search::Query>(&query_json) {
            Ok(query) => {
                self.model.search(tab, query, use_spotlight);
                true
            }
            Err(_) => false,
        }
    }

    /// Like `search`, but below `path` on `connection` rather than the
    /// tab's folder: the tab moves there and shows the results (used for
    /// "everything with this tag" in the home folder).
    pub fn search_at(&self, tab: u64, connection: u64, path: String, query_json: String, use_spotlight: bool) -> bool {
        match serde_json::from_str::<rustyfinder_core::search::Query>(&query_json) {
            Ok(query) => {
                self.model.search_at(tab, connection, &path, query, use_spotlight);
                true
            }
            Err(_) => false,
        }
    }

    /// Stop a running search; what it found stays on show.
    pub fn stop_search(&self, tab: u64) {
        self.model.stop_search(tab);
    }

    /// Leave the search results and show the folder again.
    pub fn end_search(&self, tab: u64) {
        self.model.end_search(tab);
    }

    pub fn set_view(&self, tab: u64, view: ViewOptions) {
        self.model.set_view(tab, view.into());
    }

    pub fn view(&self, tab: u64) -> Option<ViewOptions> {
        self.model.view(tab).map(Into::into)
    }

    /// Apply the hidden-files setting to every tab in the window.
    pub fn set_show_hidden(&self, show: bool) {
        self.model.set_show_hidden(show);
    }

    /// Tell the core which row is selected, so coming back to this folder
    /// later restores it.
    pub fn remember_selection(&self, tab: u64, name: Option<String>) {
        self.model.remember_selection(tab, name.as_deref());
    }

    /// The window's tabs and layout as JSON, for `RustyFinderApp::save_session`.
    /// `ui` is the shell's own state (frame, divider...) carried along.
    pub fn saved_json(&self, ui: String) -> String {
        serde_json::to_string(&self.model.saved(&ui)).unwrap_or_default()
    }
}

// ---------------------------------------------- Quick Open, Go to Folder

/// One fuzzy match: which candidate, how good, and which of its
/// characters matched (for highlighting).
#[derive(Debug, Clone, uniffi::Record)]
pub struct FuzzyHit {
    pub index: u32,
    pub score: i64,
    pub positions: Vec<u32>,
}

/// Fuzzy-match `query` against `candidates` and return the matches, best
/// first, at most `limit`. An empty query returns the first `limit`
/// candidates in their own order.
#[uniffi::export]
pub fn fuzzy_rank(query: String, candidates: Vec<String>, limit: u32) -> Vec<FuzzyHit> {
    let mut hits: Vec<FuzzyHit> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            rustyfinder_core::fuzzy::fuzzy_match(&query, candidate).map(|m| FuzzyHit {
                index: index as u32,
                score: m.score,
                positions: m.positions,
            })
        })
        .collect();
    // Stable: equal scores keep the order the caller gave.
    hits.sort_by_key(|hit| std::cmp::Reverse(hit.score));
    hits.truncate(limit as usize);
    hits
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct GotoCompletion {
    pub name: String,
    pub is_dir: bool,
    /// What the field reads after accepting this entry.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum GotoKind {
    Folder,
    /// A file: show `parent` and select `name`. Never open it.
    File,
    Missing,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct GotoResult {
    pub kind: GotoKind,
    /// The absolute path the text stands for.
    pub path: String,
    pub parent: String,
    pub name: String,
}

// ------------------------------------------------------------ Multi-Rename

#[derive(Debug, Clone, uniffi::Record)]
pub struct RenameItem {
    pub path: String,
    pub is_dir: bool,
    pub created_ms: Option<i64>,
    pub modified_ms: Option<i64>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct RenameRow {
    pub original: String,
    pub new_name: String,
    pub changed: bool,
    pub invalid: bool,
    pub conflict: bool,
    /// Why the row is flagged, or a remark.
    pub note: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct RenamePreview {
    pub rows: Vec<RenameRow>,
    pub changed: u32,
    pub problems: u32,
    pub can_rename: bool,
    /// Footer text: "2 of 5 items will be renamed".
    pub summary: String,
    /// Indices of rules that cannot work (an invalid regex).
    pub broken_rules: Vec<u32>,
    /// The rule list could not be read at all.
    pub error: Option<String>,
}

// -------------------------------------------------------------------- Sync

#[derive(Debug, Clone, uniffi::Record)]
pub struct SyncRow {
    pub path: String,
    pub action: SyncAction,
    pub to_right: bool,
    pub is_dir: bool,
    pub size: u64,
    pub left_size: Option<u64>,
    pub left_modified_ms: Option<i64>,
    pub right_size: Option<u64>,
    pub right_modified_ms: Option<i64>,
    pub conflict: bool,
    /// Conflicts start switched off.
    pub checked: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SyncPlan {
    pub rows: Vec<SyncRow>,
    pub folders_analyzed: u64,
    /// The options could not be read, or a connection is closed.
    pub error: Option<String>,
}

/// Implemented in Swift. Called on a worker thread.
#[uniffi::export(with_foreign)]
pub trait SyncListener: Send + Sync {
    /// Folders analyzed so far.
    fn progress(&self, folders: u64);
    fn finished(&self, plan: SyncPlan);
}

// ------------------------------------------------------------------ Tags

/// Toggle a tag over a selection: off if every item has it, else on.
/// Takes and returns each item's tag list.
#[uniffi::export]
pub fn tags_toggle(items: Vec<Vec<String>>, tag: String) -> Vec<Vec<String>> {
    rustyfinder_core::tags::toggle(&items, &tag)
}

/// The tags every item has.
#[uniffi::export]
pub fn tags_common(items: Vec<Vec<String>>) -> Vec<String> {
    rustyfinder_core::tags::common(&items)
}

/// Apply an edited list of common tags to every item.
#[uniffi::export]
pub fn tags_apply_common(items: Vec<Vec<String>>, edited: Vec<String>) -> Vec<Vec<String>> {
    rustyfinder_core::tags::apply_common(&items, &edited)
}

/// Known tags starting with what was typed.
#[uniffi::export]
pub fn tags_complete(known: Vec<String>, typed: String) -> Vec<String> {
    rustyfinder_core::tags::complete(&known, &typed)
}

// ----------------------------------------------------------------- Tools

/// A user-defined shell command (Settings → Tools).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ToolRecord {
    /// 0 for a tool that has not been saved yet.
    pub id: u64,
    pub name: String,
    pub command: String,
    pub show_output: bool,
    pub per_file: bool,
    pub shortcut: String,
    pub in_toolbar: bool,
    pub in_context_menu: bool,
}

impl From<rustyfinder_core::tools::Tool> for ToolRecord {
    fn from(t: rustyfinder_core::tools::Tool) -> Self {
        ToolRecord {
            id: t.id,
            name: t.name,
            command: t.command,
            show_output: t.show_output,
            per_file: t.per_file,
            shortcut: t.shortcut,
            in_toolbar: t.in_toolbar,
            in_context_menu: t.in_context_menu,
        }
    }
}

impl From<ToolRecord> for rustyfinder_core::tools::Tool {
    fn from(t: ToolRecord) -> Self {
        rustyfinder_core::tools::Tool {
            id: t.id,
            name: t.name,
            command: t.command,
            show_output: t.show_output,
            per_file: t.per_file,
            shortcut: t.shortcut,
            in_toolbar: t.in_toolbar,
            in_context_menu: t.in_context_menu,
        }
    }
}

/// What a tool's placeholders stand for when it runs.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ToolContext {
    pub source_path: String,
    pub target_path: String,
    pub source_selection: Vec<String>,
    pub target_selection: Vec<String>,
    pub hostname: String,
    pub username: String,
}

/// A tool as a new one starts out: "Untitled", output shown.
#[uniffi::export]
pub fn new_tool() -> ToolRecord {
    rustyfinder_core::tools::Tool::default().into()
}

/// The placeholders a tool's command may use.
#[uniffi::export]
pub fn tool_placeholders() -> Vec<String> {
    rustyfinder_core::tools::PLACEHOLDERS.iter().map(|p| p.to_string()).collect()
}

// -------------------------------------------------------------- Checksum

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ChecksumAlgorithm {
    Md5,
    Sha1,
    Sha256,
    Sha512,
}

impl From<ChecksumAlgorithm> for rustyfinder_core::checksum::Algorithm {
    fn from(a: ChecksumAlgorithm) -> Self {
        use rustyfinder_core::checksum::Algorithm;
        match a {
            ChecksumAlgorithm::Md5 => Algorithm::Md5,
            ChecksumAlgorithm::Sha1 => Algorithm::Sha1,
            ChecksumAlgorithm::Sha256 => Algorithm::Sha256,
            ChecksumAlgorithm::Sha512 => Algorithm::Sha512,
        }
    }
}

/// Implemented in Swift. Called on a worker thread.
#[uniffi::export(with_foreign)]
pub trait ChecksumListener: Send + Sync {
    /// Bytes of file `index` read so far.
    fn progress(&self, index: u32, bytes: u64);
    /// File `index` is done: its digest in lowercase hex, or why not.
    fn finished(&self, index: u32, digest: Option<String>, error: Option<String>);
}

/// A running checksum computation; `cancel` stops it.
#[derive(uniffi::Object)]
pub struct ChecksumJob {
    cancelled: std::sync::atomic::AtomicBool,
}

#[uniffi::export]
impl ChecksumJob {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ChecksumRow {
    pub name: String,
    pub algorithm: ChecksumAlgorithm,
    pub digest: String,
}

/// Display name of an algorithm: "SHA-256".
#[uniffi::export]
pub fn checksum_name(algorithm: ChecksumAlgorithm) -> String {
    rustyfinder_core::checksum::Algorithm::from(algorithm).name().to_string()
}

/// Rows as CSV (`name,algorithm,digest` per line) for the pasteboard.
#[uniffi::export]
pub fn checksum_csv(rows: Vec<ChecksumRow>) -> String {
    let rows: Vec<_> = rows.into_iter().map(|r| (r.name, r.algorithm.into(), r.digest)).collect();
    rustyfinder_core::checksum::csv(&rows)
}

/// Does a digest equal pasted text, ignoring case and whitespace?
#[uniffi::export]
pub fn checksum_matches(digest: String, pasted: String) -> bool {
    rustyfinder_core::checksum::matches(&digest, &pasted)
}

// ------------------------------------------------------------------- Git

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ScmStatus {
    Ignored,
    Untracked,
    Added,
    Modified,
    Conflicted,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ScmEntry {
    pub name: String,
    pub status: ScmStatus,
}

/// Git status of the items in a local folder that are not clean. `None`
/// outside a working tree. Runs git: call it off the main thread.
#[uniffi::export]
pub fn scm_status(path: String) -> Option<Vec<ScmEntry>> {
    use rustyfinder_core::scm::Status;
    let found = rustyfinder_core::scm::status(&path)?;
    Some(
        found
            .into_iter()
            .map(|(name, status)| ScmEntry {
                name,
                status: match status {
                    Status::Ignored => ScmStatus::Ignored,
                    Status::Untracked => ScmStatus::Untracked,
                    Status::Added => ScmStatus::Added,
                    Status::Modified => ScmStatus::Modified,
                    Status::Conflicted => ScmStatus::Conflicted,
                },
            })
            .collect(),
    )
}

/// Where archives that need unpacking are unpacked, per running app.
fn archive_cache_dir() -> String {
    let dir = std::env::temp_dir().join(format!("RustyFinder-archives-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir.to_string_lossy().into_owned()
}

// ----------------------------------------------------------- Connections

mod servers;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RemoteProtocol {
    Sftp,
    Ftp,
    Ftps,
    WebDav,
    WebDavs,
    S3,
    Smb,
    Afp,
    Nfs,
    Vnc,
}

/// What a URL pasted into the Server field fills in.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ParsedConnectionUrl {
    pub protocol: RemoteProtocol,
    pub host: String,
    pub user: String,
    pub password: Option<String>,
    pub port: Option<u16>,
    pub path: String,
}

/// Parse `sftp://user:pass@host:2222/path`. `None` if the text is not a
/// URL with a known scheme (a plain host name stays as typed).
#[uniffi::export]
pub fn parse_connection_url(text: String) -> Option<ParsedConnectionUrl> {
    use rustyfinder_core::url::Protocol;
    rustyfinder_core::url::parse(&text).map(|u| ParsedConnectionUrl {
        protocol: match u.protocol {
            Protocol::Sftp => RemoteProtocol::Sftp,
            Protocol::Ftp => RemoteProtocol::Ftp,
            Protocol::Ftps => RemoteProtocol::Ftps,
            Protocol::WebDav => RemoteProtocol::WebDav,
            Protocol::WebDavs => RemoteProtocol::WebDavs,
            Protocol::S3 => RemoteProtocol::S3,
            Protocol::Smb => RemoteProtocol::Smb,
            Protocol::Afp => RemoteProtocol::Afp,
            Protocol::Nfs => RemoteProtocol::Nfs,
            Protocol::Vnc => RemoteProtocol::Vnc,
        },
        host: u.host,
        user: u.user,
        password: u.password,
        port: u.port,
        path: u.path,
    })
}

/// The whole job of the askpass helper: relay ssh's question to the running
/// app and print the answer. Returns the process exit code. The RustyFinder
/// binary calls this, and nothing else, when ssh runs it as its askpass.
#[uniffi::export]
pub fn askpass_relay(prompt: String) -> i32 {
    rustyfinder_remote::relay_main(&prompt)
}

/// What the Connect panel collected for an SFTP connection.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SftpConnectParams {
    /// Host name, address, or a Host alias from ~/.ssh/config.
    pub host: String,
    /// May be empty: the ssh config or the local user name decides.
    pub user: String,
    pub port: Option<u16>,
    pub password: Option<String>,
    pub key_file: Option<String>,
    /// Folder to start in; `None` means the login directory.
    pub remote_path: Option<String>,
    /// Path of the program ssh runs to ask questions: the app's own binary.
    pub askpass_program: String,
    /// Encoding of names on the server ("windows-1251"); `None` is UTF-8.
    pub encoding: Option<String>,
    /// After a "host key changed" warning, the new key's `SHA256:`
    /// fingerprint the user chose to accept (see
    /// `changed_host_key_fingerprint`).
    pub accept_changed_host_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PromptType {
    Password,
    Passphrase,
    /// Unknown host key: the text has the fingerprint. Answer "yes" or "no".
    HostKey,
    /// Anything else, e.g. a verification code.
    Other,
}

/// An open remote connection.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ConnectionRecord {
    pub id: u64,
    /// "bob@example.org"
    pub label: String,
    /// Folder the session started in.
    pub home: String,
    /// True for an archive opened as a folder rather than a server.
    pub archive: bool,
}

/// What it takes to connect to a server other than SFTP.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ServerParams {
    pub protocol: RemoteProtocol,
    pub host: String,
    /// `None` is the protocol's standard port.
    pub port: Option<u16>,
    pub user: String,
    pub password: Option<String>,
    pub remote_path: Option<String>,
    /// FTP: passive mode (the default).
    pub passive: bool,
    /// FTP: name encoding label; `None` is automatic.
    pub encoding: Option<String>,
    /// Go on although the server's certificate cannot be verified.
    pub accept_invalid_certs: bool,
    /// S3: the region; `None` is `us-east-1`.
    pub region: Option<String>,
    /// FTP with TLS: TLS from the first byte (port 990) instead of AUTH TLS.
    #[uniffi(default = false)]
    pub implicit_tls: bool,
    /// The SHA-256 fingerprint of a certificate to accept once although the
    /// system cannot verify it ("Continue"). Trusted certificates are found
    /// on their own (see `trust_server_certificate`).
    #[uniffi(default = None)]
    pub trusted_fingerprint: Option<String>,
    /// WebDAV: send the password over plain HTTP after the user agreed.
    #[uniffi(default = false)]
    pub allow_insecure_basic: bool,
    /// S3: `Some(true)` path-style, `Some(false)` virtual-hosted addressing,
    /// `None` automatic.
    #[uniffi(default = None)]
    pub path_style: Option<bool>,
    /// S3: storage class for uploads ("STANDARD_IA"…); `None` is STANDARD.
    #[uniffi(default = None)]
    pub storage_class: Option<String>,
}

/// Why an archive could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ArchiveFailure {
    Unrecognized,
    PasswordRequired,
    WrongPassword,
    Other,
}

/// Implemented in Swift. Called on a background thread.
#[uniffi::export(with_foreign)]
pub trait ArchiveListener: Send + Sync {
    /// The archive is open as `connection`. `blocked` names entries left
    /// out because their paths lead outside the archive.
    fn opened(&self, connection: ConnectionRecord, blocked: Vec<String>);
    fn failed(&self, reason: ArchiveFailure, message: String);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ArchiveFormat {
    Zip,
    TarGz,
    TarBz2,
    TarXz,
    SevenZip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CompressionLevel {
    Store,
    Fast,
    Normal,
    Best,
}

impl From<ArchiveFormat> for rustyfinder_core::archive::Format {
    fn from(format: ArchiveFormat) -> Self {
        use rustyfinder_core::archive::Format;
        match format {
            ArchiveFormat::Zip => Format::Zip,
            ArchiveFormat::TarGz => Format::TarGz,
            ArchiveFormat::TarBz2 => Format::TarBz2,
            ArchiveFormat::TarXz => Format::TarXz,
            ArchiveFormat::SevenZip => Format::SevenZip,
        }
    }
}

/// True if the name looks like an archive RustyFinder can open as a folder.
#[uniffi::export]
pub fn is_archive_name(name: String) -> bool {
    rustyfinder_core::archive::is_archive(&name)
}

/// File extension for an archive format, without the dot.
#[uniffi::export]
pub fn archive_extension(format: ArchiveFormat) -> String {
    rustyfinder_core::archive::Format::from(format).extension().to_string()
}

/// Name (without extension) for an archive made from these items: one
/// item gives its own name, several give "Archive".
#[uniffi::export]
pub fn default_archive_name(names: Vec<String>, first_is_dir: bool) -> String {
    let items: Vec<(String, bool)> = names.into_iter().enumerate().map(|(i, n)| (n, i == 0 && first_is_dir)).collect();
    rustyfinder_core::archive::default_name(&items)
}

/// Implemented in Swift. Called on a background thread.
#[uniffi::export(with_foreign)]
pub trait ConnectListener: Send + Sync {
    /// ssh needs an answer. Blocks until the user gives one; `None`
    /// cancels the connection attempt.
    fn prompt(&self, text: String, kind: PromptType) -> Option<String>;
    fn connected(&self, connection: ConnectionRecord);
    /// `host_key_changed` marks the one failure that deserves a strong
    /// warning rather than an ordinary error.
    fn failed(&self, message: String, host_key_changed: bool);
}

struct PromptAdapter(Arc<dyn ConnectListener>);

impl rustyfinder_remote::Prompter for PromptAdapter {
    fn prompt(&self, text: &str, kind: rustyfinder_remote::PromptKind) -> Option<String> {
        use rustyfinder_remote::PromptKind;
        let kind = match kind {
            PromptKind::Password => PromptType::Password,
            PromptKind::Passphrase => PromptType::Passphrase,
            PromptKind::HostKey => PromptType::HostKey,
            PromptKind::Other => PromptType::Other,
        };
        self.0.prompt(text.to_string(), kind)
    }
}

// ------------------------------------------------------------ Activities

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum OpMode {
    Copy,
    Move,
}

/// A file operation to run. Paths are absolute.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum FileOp {
    /// Copy or move `sources` into `dest_dir`. `rename_to` renames a single
    /// source on the way ("Copy to as…").
    Transfer {
        mode: OpMode,
        sources: Vec<String>,
        dest_dir: String,
        rename_to: Option<String>,
    },
    Duplicate {
        paths: Vec<String>,
    },
    /// Permanent: cannot be undone. The shell must confirm first.
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
    NewFolderWithSelection {
        dir: String,
        paths: Vec<String>,
    },
    Symlink {
        paths: Vec<String>,
        dest_dir: String,
    },
    /// Finder aliases (this Mac only), made by the shell's `make_alias`.
    Alias {
        paths: Vec<String>,
        dest_dir: String,
    },
    /// Set permission bits; `recursive` covers enclosed items as well.
    SetMode {
        paths: Vec<String>,
        mode: u32,
        recursive: bool,
    },
    /// Copy one file to an exact target path, replacing it without asking
    /// (sending an edited file back to its server).
    Put {
        source: String,
        target: String,
    },
    /// Rename many items at once. Swaps and chains are ordered safely.
    MultiRename {
        pairs: Vec<RenamePair>,
    },
    /// Run shell command lines in `cwd` through the login shell (git
    /// commands; custom tools go through `run_tool`).
    Shell {
        /// What failed, in messages ("git push").
        name: String,
        title: String,
        summary: String,
        commands: Vec<String>,
        cwd: String,
    },
    /// Build an archive `name` (with its extension; made unique if taken)
    /// in `dest_dir`. A password only applies to zip.
    Compress {
        sources: Vec<String>,
        dest_dir: String,
        name: String,
        format: ArchiveFormat,
        level: CompressionLevel,
        password: Option<String>,
        exclude_metadata: bool,
    },
    /// Carry out a sync plan between two roots (left on the source
    /// connection, right on the target connection).
    Sync {
        left_root: String,
        right_root: String,
        steps: Vec<SyncStepRecord>,
    },
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct RenamePair {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SyncAction {
    Add,
    Update,
    Delete,
}

/// One row of a sync plan to carry out.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SyncStepRecord {
    /// Path relative to both roots.
    pub path: String,
    pub action: SyncAction,
    /// True if the right side changes.
    pub to_right: bool,
    pub is_dir: bool,
}

impl From<FileOp> for Request {
    fn from(op: FileOp) -> Self {
        match op {
            FileOp::Transfer { mode, sources, dest_dir, rename_to } => Request::Transfer {
                mode: match mode {
                    OpMode::Copy => TransferMode::Copy,
                    OpMode::Move => TransferMode::Move,
                },
                sources,
                dest_dir,
                rename_to,
            },
            FileOp::Duplicate { paths } => Request::Duplicate { paths },
            FileOp::Delete { paths } => Request::Delete { paths },
            FileOp::Trash { paths } => Request::Trash { paths },
            FileOp::Rename { path, new_name } => Request::Rename { path, new_name },
            FileOp::NewFolder { dir } => Request::NewFolder { dir },
            FileOp::NewFile { dir } => Request::NewFile { dir },
            FileOp::NewFolderWithSelection { dir, paths } => Request::NewFolderWithSelection { dir, paths },
            FileOp::Symlink { paths, dest_dir } => Request::Symlink { paths, dest_dir },
            FileOp::Alias { paths, dest_dir } => Request::Alias { paths, dest_dir },
            FileOp::SetMode { paths, mode, recursive } => Request::SetMode { paths, mode, recursive },
            FileOp::Put { source, target } => Request::Put { source, target },
            FileOp::Shell { name, title, summary, commands, cwd } => {
                Request::Shell { name, title, summary, commands, cwd }
            }
            FileOp::Compress { sources, dest_dir, name, format, level, password, exclude_metadata } => {
                use rustyfinder_core::archive::{CompressOptions, Level};
                Request::Compress {
                    sources,
                    dest_dir,
                    name,
                    options: CompressOptions {
                        format: format.into(),
                        level: match level {
                            CompressionLevel::Store => Level::Store,
                            CompressionLevel::Fast => Level::Fast,
                            CompressionLevel::Normal => Level::Normal,
                            CompressionLevel::Best => Level::Best,
                        },
                        password,
                        exclude_metadata,
                    },
                }
            }
            FileOp::MultiRename { pairs } => {
                Request::MultiRename { pairs: pairs.into_iter().map(|p| (p.from, p.to)).collect() }
            }
            FileOp::Sync { left_root, right_root, steps } => Request::Sync {
                left_root,
                right_root,
                steps: steps
                    .into_iter()
                    .map(|s| rustyfinder_core::ops::SyncStep {
                        path: s.path,
                        action: match s.action {
                            SyncAction::Add => rustyfinder_core::sync::Action::Add,
                            SyncAction::Update => rustyfinder_core::sync::Action::Update,
                            SyncAction::Delete => rustyfinder_core::sync::Action::Delete,
                        },
                        to_right: s.to_right,
                        is_dir: s.is_dir,
                    })
                    .collect(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ActivityPhase {
    Queued,
    Preparing,
    Running,
    Paused,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ActivityRecord {
    pub id: u64,
    pub window: u64,
    pub title: String,
    pub phase: ActivityPhase,
    pub items_done: u64,
    pub items_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Name of the item being worked on.
    pub current: String,
    /// Bytes per second.
    pub speed: u64,
    pub eta_secs: Option<u64>,
    pub waiting_for_user: bool,
}

impl From<ActivityInfo> for ActivityRecord {
    fn from(a: ActivityInfo) -> Self {
        ActivityRecord {
            id: a.id,
            window: a.window,
            title: a.title,
            phase: match a.state {
                ActivityState::Queued => ActivityPhase::Queued,
                ActivityState::Preparing => ActivityPhase::Preparing,
                ActivityState::Running => ActivityPhase::Running,
                ActivityState::Paused => ActivityPhase::Paused,
            },
            items_done: a.progress.items_done,
            items_total: a.progress.items_total,
            bytes_done: a.progress.bytes_done,
            bytes_total: a.progress.bytes_total,
            current: a.progress.current,
            speed: a.speed,
            eta_secs: a.eta_secs,
            waiting_for_user: a.waiting_for_user,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ConflictType {
    FileFile,
    FolderFolder,
    /// File vs folder: Merge is not offered.
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ConflictOption {
    Replace,
    ReplaceOlder,
    KeepBoth,
    Skip,
    Stop,
    Merge,
    /// Carry on a partial file a stopped transfer left (only when offered).
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ErrorOption {
    Retry,
    Skip,
    SkipAll,
    Stop,
    /// Retry through the system's authorization prompt (only when the
    /// prompt said `can_elevate`).
    RetryAsAdmin,
}

/// Two items competing for one name.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ConflictRecord {
    pub kind: ConflictType,
    pub source_path: String,
    pub source: Row,
    pub target_path: String,
    pub target: Row,
    /// The target is what a stopped transfer of this source left: offer
    /// Resume.
    pub resumable: bool,
    /// Folder vs folder: what only the target holds. Replace would delete
    /// it, so confirm before answering Replace.
    pub target_extras: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum OpResult {
    Success,
    /// Finished, but some items failed or were skipped.
    Partial,
    Failed,
    Stopped,
}

impl From<ActivityResult> for OpResult {
    fn from(r: ActivityResult) -> Self {
        match r {
            ActivityResult::Success => OpResult::Success,
            ActivityResult::Partial => OpResult::Partial,
            ActivityResult::Failed => OpResult::Failed,
            ActivityResult::Stopped => OpResult::Stopped,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FinishRecord {
    pub id: u64,
    pub window: u64,
    pub title: String,
    pub result: OpResult,
    /// Why it could not start, or the first failure. Show it to the user.
    pub message: Option<String>,
    /// Top-level items to select afterwards.
    pub results: Vec<String>,
    /// Put the new item straight into inline rename.
    pub rename_after: bool,
    /// Worth a completion sound (transfers, deletes, undo), unlike quick edits.
    pub notable: bool,
    /// What a shell command printed (custom tools, git), if it was one.
    pub output: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct LogRecord {
    pub time_ms: i64,
    pub title: String,
    pub result: OpResult,
    pub lines: Vec<String>,
}

/// Implemented in Swift. Called on Rust worker threads.
#[uniffi::export(with_foreign)]
pub trait ActivityListener: Send + Sync {
    fn activities_changed(&self, activities: Vec<ActivityRecord>);
    /// Answer with `RustyFinderApp::answer_conflict`.
    fn conflict_prompt(&self, activity: u64, conflict: ConflictRecord);
    /// Answer with `RustyFinderApp::answer_error`.
    /// `can_elevate`: a permission problem on this Mac that "Retry as
    /// Administrator" could get past.
    fn error_prompt(&self, activity: u64, path: String, message: String, can_elevate: bool);
    fn finished(&self, report: FinishRecord);
}

/// Things only the Swift shell can do for the core.
#[uniffi::export(with_foreign)]
pub trait PlatformServices: Send + Sync {
    /// Move an item to the Trash and return its new path.
    fn trash(&self, path: String) -> Result<String, RustyFinderError>;
    /// Make a Finder alias file at the new path `alias` for `target`.
    fn make_alias(&self, target: String, alias: String) -> Result<(), RustyFinderError>;
    /// Replace an item's Finder tags (undo of a tag change).
    fn set_tags(&self, path: String, tags: Vec<String>) -> Result<(), RustyFinderError>;
    /// Replace an item's Finder comment (undo of a comment change).
    fn set_comment(&self, path: String, comment: String) -> Result<(), RustyFinderError>;
    /// Run one step through the system's authorization prompt (see
    /// `local_ops::PrivilegedStep`). Returns a trashed item's new path.
    fn elevate(&self, step: local_ops::PrivilegedStep) -> Result<String, RustyFinderError>;
}

struct ActivityAdapter(Arc<dyn ActivityListener>);

impl ActivityObserver for ActivityAdapter {
    fn activities_changed(&self, activities: Vec<ActivityInfo>) {
        self.0.activities_changed(activities.into_iter().map(Into::into).collect());
    }

    fn prompt(&self, activity: u64, prompt: Prompt) {
        match prompt {
            Prompt::Conflict(info) => self.0.conflict_prompt(
                activity,
                ConflictRecord {
                    kind: match info.kind {
                        ConflictKind::FileFile => ConflictType::FileFile,
                        ConflictKind::DirDir => ConflictType::FolderFolder,
                        ConflictKind::Mixed => ConflictType::Mixed,
                    },
                    source: Row::from(&info.source),
                    source_path: info.source_path,
                    target: Row::from(&info.target),
                    target_path: info.target_path,
                    resumable: info.resumable,
                    target_extras: info.target_extras,
                },
            ),
            Prompt::Error(info) => self.0.error_prompt(activity, info.path, info.message, info.can_elevate),
        }
    }

    fn finished(&self, report: FinishReport) {
        self.0.finished(FinishRecord {
            id: report.id,
            window: report.window,
            title: report.title,
            result: report.result.into(),
            message: report.message,
            results: report.results,
            rename_after: report.rename_after,
            notable: report.notable,
            output: report.output,
        });
    }
}

struct PlatformAdapter(Arc<dyn PlatformServices>);

impl Platform for PlatformAdapter {
    fn trash(&self, path: &str) -> Result<String, String> {
        self.0.trash(path.to_string()).map_err(|e| e.to_string())
    }

    fn make_alias(&self, target: &str, alias: &str) -> Result<(), String> {
        self.0.make_alias(target.to_string(), alias.to_string()).map_err(|e| e.to_string())
    }

    fn set_tags(&self, path: &str, tags: &[String]) -> Result<(), String> {
        self.0.set_tags(path.to_string(), tags.to_vec()).map_err(|e| e.to_string())
    }

    fn set_comment(&self, path: &str, comment: &str) -> Result<(), String> {
        self.0.set_comment(path.to_string(), comment.to_string()).map_err(|e| e.to_string())
    }

    fn elevate(&self, action: &rustyfinder_core::ops::Privileged) -> Result<String, String> {
        self.0.elevate(action.into()).map_err(|e| e.to_string())
    }
}

// ------------------------------------------------------------------- App

/// User settings (see settings-defaults spec).
#[derive(Debug, Clone, uniffi::Record)]
pub struct AppSettings {
    /// Empty means the home folder.
    pub starting_directory: String,
    pub restore_windows: bool,
    pub show_hidden: bool,
    pub always_show_tab_bar: bool,
    pub folders_on_top: bool,
    pub relative_dates: bool,
    pub alternating_rows: bool,
    pub treat_packages_as_folders: bool,
    pub treat_archives_as_folders: bool,
    pub confirm_drag_operations: bool,
    pub play_sounds: bool,
    /// Transfers that may run at once (1 to 10).
    pub max_transfers: u32,
    /// Text size in List view, in points.
    pub list_text_size: u32,
    /// Icon size in Icon view, in points.
    pub icon_size: u32,
    /// What happens when a file would replace another (never `Merge`).
    pub replace_policy: ConflictPolicyChoice,
    /// What happens when a folder meets a folder of the same name.
    pub merge_policy: ConflictPolicyChoice,
    pub error_policy: ErrorPolicyChoice,
    /// Bundle path of the Terminal application; "" is Apple's Terminal.
    pub terminal_app: String,
    /// Moving an app to the Trash opens App Deleter.
    pub detect_app_when_deleting: bool,
    /// Which pane the Connect panel and remote favorites open in.
    pub open_remote_in: RemoteSideChoice,
    /// Extract and compress with RustyFinder's own engine.
    pub handle_archives: bool,
    /// "Continue without Full Disk Access" was chosen.
    pub fda_dismissed: bool,
    /// The first-run welcome has been shown.
    pub welcome_shown: bool,
    /// Id of the theme in use.
    pub theme: String,
    /// A favorite to start at instead of `starting_directory` (0: none).
    pub starting_favorite: u64,
    /// Custom tools get the login shell's `$PATH`.
    pub tools_inherit_path: bool,
    /// Key of the external compare tool ("filemerge", "kaleidoscope"…).
    pub compare_tool: String,
    /// What happens when a stopped transfer left a partial file.
    pub resume_policy: ResumePolicyChoice,
    /// Retry failed transfers on their own, up to `max_retries` (1 to 10).
    pub auto_retry: bool,
    pub max_retries: u32,
    /// Bandwidth limits in KB/s, for all downloads (uploads) together.
    pub limit_downloads: bool,
    pub download_limit_kbps: u32,
    pub limit_uploads: bool,
    pub upload_limit_kbps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RemoteSideChoice {
    Active,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ResumePolicyChoice {
    Ask,
    Resume,
    Replace,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ConflictPolicyChoice {
    Ask,
    Replace,
    ReplaceOlder,
    KeepBoth,
    Skip,
    Merge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ErrorPolicyChoice {
    Ask,
    Skip,
    Stop,
}

impl From<rustyfinder_core::ConflictPolicy> for ConflictPolicyChoice {
    fn from(p: rustyfinder_core::ConflictPolicy) -> Self {
        use rustyfinder_core::ConflictPolicy as P;
        match p {
            P::Ask => Self::Ask,
            P::Replace => Self::Replace,
            P::ReplaceOlder => Self::ReplaceOlder,
            P::KeepBoth => Self::KeepBoth,
            P::Skip => Self::Skip,
            P::Merge => Self::Merge,
        }
    }
}

impl From<ConflictPolicyChoice> for rustyfinder_core::ConflictPolicy {
    fn from(p: ConflictPolicyChoice) -> Self {
        use ConflictPolicyChoice as C;
        match p {
            C::Ask => Self::Ask,
            C::Replace => Self::Replace,
            C::ReplaceOlder => Self::ReplaceOlder,
            C::KeepBoth => Self::KeepBoth,
            C::Skip => Self::Skip,
            C::Merge => Self::Merge,
        }
    }
}

/// A command and its shortcut, as text ("cmd+shift+n"; "" for none).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ShortcutEntry {
    pub command: String,
    pub binding: String,
}

/// The shortcuts after a change, and who stood in the way if it was
/// refused.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ShortcutResult {
    pub entries: Vec<ShortcutEntry>,
    /// The command that already has the binding; nothing changed then.
    pub conflict: Option<String>,
}

/// A binding as the menus show it: "⇧⌘N".
#[uniffi::export]
pub fn shortcut_display(binding: String) -> String {
    rustyfinder_core::shortcuts::display(&binding)
}

/// A saved window layout with a name.
#[derive(Debug, Clone, uniffi::Record)]
pub struct WorkspaceRecord {
    pub id: u64,
    pub name: String,
    pub window: SavedWindowRecord,
}

impl From<rustyfinder_core::Settings> for AppSettings {
    fn from(s: rustyfinder_core::Settings) -> Self {
        AppSettings {
            starting_directory: s.starting_directory,
            restore_windows: s.restore_windows,
            show_hidden: s.show_hidden,
            always_show_tab_bar: s.always_show_tab_bar,
            folders_on_top: s.folders_on_top,
            relative_dates: s.relative_dates,
            alternating_rows: s.alternating_rows,
            treat_packages_as_folders: s.treat_packages_as_folders,
            treat_archives_as_folders: s.treat_archives_as_folders,
            confirm_drag_operations: s.confirm_drag_operations,
            play_sounds: s.play_sounds,
            max_transfers: s.max_transfers,
            list_text_size: s.list_text_size,
            icon_size: s.icon_size,
            replace_policy: s.replace_policy.into(),
            merge_policy: s.merge_policy.into(),
            error_policy: match s.error_policy {
                rustyfinder_core::ErrorPolicy::Ask => ErrorPolicyChoice::Ask,
                rustyfinder_core::ErrorPolicy::Skip => ErrorPolicyChoice::Skip,
                rustyfinder_core::ErrorPolicy::Stop => ErrorPolicyChoice::Stop,
            },
            terminal_app: s.terminal_app,
            detect_app_when_deleting: s.detect_app_when_deleting,
            open_remote_in: match s.open_remote_in {
                rustyfinder_core::store::RemoteSide::Active => RemoteSideChoice::Active,
                rustyfinder_core::store::RemoteSide::Left => RemoteSideChoice::Left,
                rustyfinder_core::store::RemoteSide::Right => RemoteSideChoice::Right,
            },
            handle_archives: s.handle_archives,
            fda_dismissed: s.fda_dismissed,
            welcome_shown: s.welcome_shown,
            theme: s.theme,
            starting_favorite: s.starting_favorite,
            tools_inherit_path: s.tools_inherit_path,
            compare_tool: s.compare_tool,
            resume_policy: match s.resume_policy {
                rustyfinder_core::ops::ResumePolicy::Ask => ResumePolicyChoice::Ask,
                rustyfinder_core::ops::ResumePolicy::Resume => ResumePolicyChoice::Resume,
                rustyfinder_core::ops::ResumePolicy::Replace => ResumePolicyChoice::Replace,
                rustyfinder_core::ops::ResumePolicy::Skip => ResumePolicyChoice::Skip,
            },
            auto_retry: s.auto_retry,
            max_retries: s.max_retries,
            limit_downloads: s.limit_downloads,
            download_limit_kbps: s.download_limit_kbps,
            limit_uploads: s.limit_uploads,
            upload_limit_kbps: s.upload_limit_kbps,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FavoriteRecord {
    pub id: u64,
    pub name: String,
    pub path: String,
    /// SF Symbol name, an image file's path, or "" for the item's own icon.
    pub icon: String,
    pub kind: FavoriteKindRecord,
    pub server: Option<ServerSpecRecord>,
    /// Synclet, preset or saved search JSON; a workspace's id.
    pub payload: String,
    /// Paths saved under a server favorite.
    pub children: Vec<FavoritePathRecord>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FavoriteGroupRecord {
    pub id: u64,
    pub name: String,
    pub collapsed: bool,
    pub items: Vec<FavoriteRecord>,
}

impl From<rustyfinder_core::Favorite> for FavoriteRecord {
    fn from(f: rustyfinder_core::Favorite) -> Self {
        FavoriteRecord {
            id: f.id,
            name: f.name,
            path: f.path,
            icon: f.icon,
            kind: f.kind.into(),
            server: f.server.map(Into::into),
            payload: f.payload,
            children: f.children.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SpaceInfo {
    pub free: u64,
    pub total: u64,
}

/// A window saved in the last session.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SavedWindowRecord {
    /// Pass back to `RustyFinderApp::open_window` to restore it.
    pub json: String,
    /// The shell state given to `WindowSession::saved_json`.
    pub ui: String,
}

/// App-wide state: settings, favorites, recent folders, saved session.
#[derive(uniffi::Object)]
pub struct RustyFinderApp {
    store: Arc<Store>,
    /// The local disk (also connection 0 of the registry).
    vfs: Arc<dyn Vfs>,
    registry: Arc<VfsRegistry>,
    /// Set once by `attach`; file operations need it.
    activities: OnceLock<ActivityManager>,
    /// The shortcuts in force, once the shell has named its commands.
    shortcuts: std::sync::Mutex<rustyfinder_core::shortcuts::Bindings>,
}

fn shortcut_entries(bindings: &rustyfinder_core::shortcuts::Bindings) -> Vec<ShortcutEntry> {
    bindings.all().into_iter().map(|(command, binding)| ShortcutEntry { command, binding }).collect()
}

#[uniffi::export]
impl RustyFinderApp {
    /// `support_dir` is where settings and session files live.
    #[uniffi::constructor]
    pub fn new(support_dir: String, home: String) -> Arc<Self> {
        let vfs: Arc<dyn Vfs> = Arc::new(LocalFs::new());
        let store = Arc::new(Store::open(support_dir, home.clone()));
        rustyfinder_core::tools::set_inherit_path(store.settings().tools_inherit_path);
        Arc::new(RustyFinderApp {
            registry: Arc::new(VfsRegistry::new(Arc::clone(&vfs), home.clone())),
            store,
            vfs,
            activities: OnceLock::new(),
            shortcuts: std::sync::Mutex::new(rustyfinder_core::shortcuts::Bindings::default()),
        })
    }

    /// Connect the shell: where activity updates go and who moves things to
    /// the Trash. Call once at launch, before any file operation.
    pub fn attach(&self, listener: Arc<dyn ActivityListener>, platform: Arc<dyn PlatformServices>) {
        let _ = self.activities.set(ActivityManager::new(
            Arc::clone(&self.registry),
            Arc::clone(&self.store),
            Arc::new(PlatformAdapter(platform)),
            Arc::new(ActivityAdapter(listener)),
        ));
    }

    /// Start (or queue) a file operation for a window. Returns its id, or
    /// `None` before `attach`.
    pub fn submit(&self, window: u64, op: FileOp) -> Option<u64> {
        self.activities.get().map(|m| m.submit(window, op.into()))
    }

    /// Like `submit`, for operations that involve a server. `src` is the
    /// connection the source paths are on, `dst` the connection of the
    /// target folder (0 = local disk). Operations on one file system pass
    /// it as both.
    pub fn submit_between(&self, window: u64, op: FileOp, src: u64, dst: u64) -> Option<u64> {
        self.activities.get().map(|m| m.submit_on(window, op.into(), src, dst))
    }

    pub fn activities(&self) -> Vec<ActivityRecord> {
        self.activities.get().map(|m| m.activities().into_iter().map(Into::into).collect()).unwrap_or_default()
    }

    /// True if anything is running or queued, for one window or anywhere.
    pub fn has_active(&self, window: Option<u64>) -> bool {
        self.activities.get().is_some_and(|m| m.has_active(window))
    }

    pub fn pause_activity(&self, id: u64) {
        if let Some(m) = self.activities.get() {
            m.pause(id);
        }
    }

    pub fn resume_activity(&self, id: u64) {
        if let Some(m) = self.activities.get() {
            m.resume(id);
        }
    }

    pub fn stop_activity(&self, id: u64) {
        if let Some(m) = self.activities.get() {
            m.stop(id);
        }
    }

    /// Stop every activity started from `window`.
    pub fn stop_all_activities(&self, window: u64) {
        if let Some(m) = self.activities.get() {
            m.stop_all(window);
        }
    }

    /// Move a queued activity to `index` among the queued ones.
    pub fn reorder_activity(&self, id: u64, index: u32) {
        if let Some(m) = self.activities.get() {
            m.reorder(id, index as usize);
        }
    }

    pub fn answer_conflict(&self, activity: u64, choice: ConflictOption, apply_to_all: bool) {
        let choice = match choice {
            ConflictOption::Replace => ConflictChoice::Replace,
            ConflictOption::ReplaceOlder => ConflictChoice::ReplaceOlder,
            ConflictOption::KeepBoth => ConflictChoice::KeepBoth,
            ConflictOption::Skip => ConflictChoice::Skip,
            ConflictOption::Stop => ConflictChoice::Stop,
            ConflictOption::Merge => ConflictChoice::Merge,
            ConflictOption::Resume => ConflictChoice::Resume,
        };
        if let Some(m) = self.activities.get() {
            m.answer_conflict(activity, ConflictAnswer { choice, apply_to_all });
        }
    }

    pub fn answer_error(&self, activity: u64, choice: ErrorOption) {
        let choice = match choice {
            ErrorOption::Retry => ErrorChoice::Retry,
            ErrorOption::Skip => ErrorChoice::Skip,
            ErrorOption::SkipAll => ErrorChoice::SkipAll,
            ErrorOption::Stop => ErrorChoice::Stop,
            ErrorOption::RetryAsAdmin => ErrorChoice::Elevate,
        };
        if let Some(m) = self.activities.get() {
            m.answer_error(activity, choice);
        }
    }

    /// Edit-menu title such as "Undo Move to Trash", if anything can be undone.
    pub fn undo_title(&self, window: u64) -> Option<String> {
        self.activities.get().and_then(|m| m.undo_title(window))
    }

    pub fn redo_title(&self, window: u64) -> Option<String> {
        self.activities.get().and_then(|m| m.redo_title(window))
    }

    pub fn undo(&self, window: u64) -> Option<u64> {
        self.activities.get().and_then(|m| m.undo(window))
    }

    pub fn redo(&self, window: u64) -> Option<u64> {
        self.activities.get().and_then(|m| m.redo(window))
    }

    /// Finished operations, oldest first.
    pub fn operation_log(&self) -> Vec<LogRecord> {
        self.activities
            .get()
            .map(|m| {
                m.log()
                    .into_iter()
                    .map(|e| LogRecord { time_ms: e.time_ms, title: e.title, result: e.result.into(), lines: e.lines })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn settings(&self) -> AppSettings {
        self.store.settings().into()
    }

    pub fn set_settings(&self, settings: AppSettings) {
        self.store.update_settings(|s| {
            s.starting_directory = settings.starting_directory;
            s.restore_windows = settings.restore_windows;
            s.show_hidden = settings.show_hidden;
            s.always_show_tab_bar = settings.always_show_tab_bar;
            s.folders_on_top = settings.folders_on_top;
            s.relative_dates = settings.relative_dates;
            s.alternating_rows = settings.alternating_rows;
            s.treat_packages_as_folders = settings.treat_packages_as_folders;
            s.treat_archives_as_folders = settings.treat_archives_as_folders;
            s.confirm_drag_operations = settings.confirm_drag_operations;
            s.play_sounds = settings.play_sounds;
            s.max_transfers = settings.max_transfers.clamp(1, 10);
            s.list_text_size = settings.list_text_size.clamp(9, 36);
            s.icon_size = settings.icon_size.clamp(16, 512);
            // A file cannot be merged into a file.
            s.replace_policy = match settings.replace_policy {
                ConflictPolicyChoice::Merge => rustyfinder_core::ConflictPolicy::Ask,
                other => other.into(),
            };
            s.merge_policy = settings.merge_policy.into();
            s.error_policy = match settings.error_policy {
                ErrorPolicyChoice::Ask => rustyfinder_core::ErrorPolicy::Ask,
                ErrorPolicyChoice::Skip => rustyfinder_core::ErrorPolicy::Skip,
                ErrorPolicyChoice::Stop => rustyfinder_core::ErrorPolicy::Stop,
            };
            s.terminal_app = settings.terminal_app;
            s.detect_app_when_deleting = settings.detect_app_when_deleting;
            s.open_remote_in = match settings.open_remote_in {
                RemoteSideChoice::Active => rustyfinder_core::store::RemoteSide::Active,
                RemoteSideChoice::Left => rustyfinder_core::store::RemoteSide::Left,
                RemoteSideChoice::Right => rustyfinder_core::store::RemoteSide::Right,
            };
            s.handle_archives = settings.handle_archives;
            s.fda_dismissed = settings.fda_dismissed;
            s.welcome_shown = settings.welcome_shown;
            // The theme changes through `select_theme`, which checks it exists.
            s.starting_favorite = settings.starting_favorite;
            s.tools_inherit_path = settings.tools_inherit_path;
            s.compare_tool = settings.compare_tool;
            s.resume_policy = match settings.resume_policy {
                ResumePolicyChoice::Ask => rustyfinder_core::ops::ResumePolicy::Ask,
                ResumePolicyChoice::Resume => rustyfinder_core::ops::ResumePolicy::Resume,
                ResumePolicyChoice::Replace => rustyfinder_core::ops::ResumePolicy::Replace,
                ResumePolicyChoice::Skip => rustyfinder_core::ops::ResumePolicy::Skip,
            };
            s.auto_retry = settings.auto_retry;
            s.max_retries = settings.max_retries.clamp(1, 10);
            s.limit_downloads = settings.limit_downloads;
            s.download_limit_kbps = settings.download_limit_kbps.max(1);
            s.limit_uploads = settings.limit_uploads;
            s.upload_limit_kbps = settings.upload_limit_kbps.max(1);
        });
        rustyfinder_core::tools::set_inherit_path(self.store.settings().tools_inherit_path);
    }

    /// The settings as a fresh install has them.
    pub fn default_settings(&self) -> AppSettings {
        rustyfinder_core::Settings::default().into()
    }

    // --- Shortcuts

    /// Tell the core which commands exist and their default shortcuts;
    /// returns every command with the shortcut now in force (the saved
    /// changes laid over the defaults).
    pub fn shortcuts_load(&self, defaults: Vec<ShortcutEntry>) -> Vec<ShortcutEntry> {
        let bindings = rustyfinder_core::shortcuts::Bindings::new(
            defaults.into_iter().map(|e| (e.command, e.binding)),
            self.store.custom_shortcuts(),
        );
        let entries = shortcut_entries(&bindings);
        *self.shortcuts.lock().unwrap() = bindings;
        entries
    }

    /// Give a command a shortcut ("" removes it). If another command has
    /// it and `force` is false, nothing changes and that command is named.
    pub fn shortcut_assign(&self, command: String, binding: String, force: bool) -> ShortcutResult {
        let mut bindings = self.shortcuts.lock().unwrap();
        let conflict = if force {
            bindings.reassign(&command, &binding);
            None
        } else {
            bindings.assign(&command, &binding).err().map(|c| c.other)
        };
        self.store.save_custom_shortcuts(bindings.custom());
        ShortcutResult { entries: shortcut_entries(&bindings), conflict }
    }

    /// Back to the built-in shortcuts.
    pub fn shortcuts_restore(&self) -> Vec<ShortcutEntry> {
        let mut bindings = self.shortcuts.lock().unwrap();
        bindings.restore_defaults();
        self.store.save_custom_shortcuts(bindings.custom());
        shortcut_entries(&bindings)
    }

    // --- Workspaces

    pub fn workspaces(&self) -> Vec<WorkspaceRecord> {
        self.store
            .workspaces()
            .into_iter()
            .map(|w| WorkspaceRecord {
                id: w.id,
                name: w.name,
                window: SavedWindowRecord {
                    json: serde_json::to_string(&w.window).unwrap_or_default(),
                    ui: w.window.ui,
                },
            })
            .collect()
    }

    /// Save a window (from `WindowSession::saved_json`) as a workspace.
    /// `None` if the name is empty or taken.
    pub fn save_workspace(&self, name: String, window_json: String) -> Option<u64> {
        let window = serde_json::from_str::<SavedWindow>(&window_json).ok()?;
        self.store.add_workspace(&name, window)
    }

    /// Store the window as it is now over an existing workspace.
    pub fn update_workspace(&self, id: u64, window_json: String) -> bool {
        serde_json::from_str::<SavedWindow>(&window_json).is_ok_and(|window| self.store.update_workspace(id, window))
    }

    pub fn remove_workspace(&self, id: u64) -> bool {
        self.store.remove_workspace(id)
    }

    pub fn starting_directory(&self) -> String {
        self.store.starting_directory()
    }

    /// Open a window. `restore` is a `SavedWindowRecord::json`; without it
    /// both panes start at the starting directory.
    pub fn open_window(&self, listener: Arc<dyn WindowListener>, restore: Option<String>) -> Arc<WindowSession> {
        let restore = restore.and_then(|json| serde_json::from_str::<SavedWindow>(&json).ok());
        Arc::new(WindowSession {
            id: NEXT_WINDOW_ID.fetch_add(1, Ordering::SeqCst),
            model: WindowModel::new(
                Arc::clone(&self.registry),
                Arc::clone(&self.store),
                Arc::new(WindowAdapter(listener)),
                restore,
            ),
        })
    }

    pub fn saved_windows(&self) -> Vec<SavedWindowRecord> {
        self.store
            .saved_windows()
            .into_iter()
            .map(|w| SavedWindowRecord { json: serde_json::to_string(&w).unwrap_or_default(), ui: w.ui })
            .collect()
    }

    /// Persist the open windows (each from `WindowSession::saved_json`).
    pub fn save_session(&self, windows: Vec<String>) {
        let parsed: Vec<SavedWindow> = windows.iter().filter_map(|json| serde_json::from_str(json).ok()).collect();
        self.store.save_windows(&parsed);
    }

    pub fn favorite_groups(&self) -> Vec<FavoriteGroupRecord> {
        self.store
            .favorite_groups()
            .into_iter()
            .map(|g| FavoriteGroupRecord {
                id: g.id,
                name: g.name,
                collapsed: g.collapsed,
                items: g.items.into_iter().map(Into::into).collect(),
            })
            .collect()
    }

    pub fn add_favorite(&self, group: String, name: String, path: String) -> Result<FavoriteRecord, RustyFinderError> {
        Ok(self.store.add_favorite(&group, &name, &path)?.into())
    }

    pub fn remove_favorite(&self, id: u64) -> Result<(), RustyFinderError> {
        Ok(self.store.remove_favorite(id)?)
    }

    pub fn rename_favorite(&self, id: u64, name: String) -> Result<(), RustyFinderError> {
        Ok(self.store.rename_favorite(id, &name)?)
    }

    pub fn move_favorite(&self, id: u64, group_id: u64, index: u32) -> Result<(), RustyFinderError> {
        Ok(self.store.move_favorite(id, group_id, index as usize)?)
    }

    pub fn set_group_collapsed(&self, group_id: u64, collapsed: bool) {
        self.store.set_group_collapsed(group_id, collapsed);
    }

    /// Last 10 folders visited, newest first; deleted ones are dropped.
    pub fn recent_folders(&self) -> Vec<String> {
        self.store.recent_folders(&*self.vfs)
    }

    /// Open an SFTP connection in the background. The listener is asked
    /// for anything ssh needs and told the outcome.
    pub fn connect_sftp(&self, params: SftpConnectParams, listener: Arc<dyn ConnectListener>) {
        let registry = Arc::clone(&self.registry);
        std::thread::spawn(move || {
            let ssh = rustyfinder_core::tools::SshTarget {
                host: params.host.clone(),
                user: params.user.clone(),
                port: params.port,
            };
            let params = rustyfinder_remote::SftpParams {
                host: params.host,
                user: params.user,
                port: params.port,
                password: params.password.filter(|p| !p.is_empty()),
                key_file: params.key_file.filter(|k| !k.is_empty()),
                remote_path: params.remote_path.filter(|p| !p.is_empty()),
                askpass_program: params.askpass_program,
                // Scripted checks point ssh at their own config and
                // known_hosts so the user's ~/.ssh is never touched.
                config_file: std::env::var("RUSTYFINDER_SSH_CONFIG").ok(),
                known_hosts_file: std::env::var("RUSTYFINDER_KNOWN_HOSTS").ok(),
                encoding: params.encoding.filter(|e| !e.is_empty()),
                accept_changed_host_key: params.accept_changed_host_key,
            };
            let prompter = Arc::new(PromptAdapter(Arc::clone(&listener)));
            match rustyfinder_remote::connect(&params, prompter) {
                Ok(fs) => {
                    let (label, home) = (fs.label().to_string(), fs.home().to_string());
                    let id = registry.add(Arc::new(fs), label.clone(), home.clone());
                    registry.set_ssh_target(id, ssh);
                    listener.connected(ConnectionRecord { id, label, home, archive: false });
                }
                Err(error) => {
                    let changed = matches!(error, rustyfinder_remote::ConnectError::HostKeyChanged(_));
                    listener.failed(error.to_string(), changed);
                }
            }
        });
    }

    /// Connect to a server that is not SFTP (FTP, FTP with TLS, WebDAV, S3)
    /// in the background. The listener is told the outcome; a failure
    /// whose message starts with "The server’s certificate" can be tried
    /// again with `accept_invalid_certs`.
    pub fn connect_server(&self, params: ServerParams, listener: Arc<dyn ConnectListener>) {
        let registry = Arc::clone(&self.registry);
        let trust = rustyfinder_remote::trust::TrustStore::open(self.store.support_file(servers::TRUST_FILE));
        std::thread::spawn(move || {
            use rustyfinder_remote::ftp;
            // A certificate the user chose to always trust for this server,
            // unless one was given for this attempt only.
            let port = params.port.unwrap_or(servers::default_port(params.protocol, params.implicit_tls));
            let pinned = params.trusted_fingerprint.clone().or_else(|| {
                trust.certificate(&params.host, port).map(|der| rustyfinder_remote::trust::fingerprint(&der))
            });
            let result: Result<(Arc<dyn Vfs>, String, String), String> = match params.protocol {
                RemoteProtocol::Ftp | RemoteProtocol::Ftps => ftp::connect(ftp::FtpParams {
                    host: params.host,
                    port: params.port,
                    user: params.user,
                    password: params.password.unwrap_or_default(),
                    security: match (params.protocol, params.implicit_tls) {
                        (RemoteProtocol::Ftps, true) => ftp::FtpSecurity::Implicit,
                        (RemoteProtocol::Ftps, false) => ftp::FtpSecurity::Explicit,
                        _ => ftp::FtpSecurity::None,
                    },
                    passive: params.passive,
                    encoding: params.encoding.filter(|e| !e.is_empty()),
                    remote_path: params.remote_path.filter(|p| !p.is_empty()),
                    accept_invalid_certs: params.accept_invalid_certs,
                    trusted_fingerprint: pinned,
                    ..Default::default()
                })
                .map(|fs| {
                    let (label, home) = (fs.label().to_string(), fs.home().to_string());
                    (Arc::new(fs) as Arc<dyn Vfs>, label, home)
                })
                .map_err(|e| e.to_string()),
                RemoteProtocol::WebDav | RemoteProtocol::WebDavs => {
                    rustyfinder_remote::webdav::connect(rustyfinder_remote::webdav::WebDavParams {
                        host: params.host,
                        port: params.port,
                        https: params.protocol == RemoteProtocol::WebDavs,
                        user: params.user,
                        password: params.password.unwrap_or_default(),
                        remote_path: params.remote_path.filter(|p| !p.is_empty()),
                        accept_invalid_certs: params.accept_invalid_certs,
                        trusted_fingerprint: pinned,
                        allow_insecure_basic: params.allow_insecure_basic,
                    })
                    .map(|fs| {
                        let (label, home) = (fs.label().to_string(), fs.home().to_string());
                        (Arc::new(fs) as Arc<dyn Vfs>, label, home)
                    })
                    .map_err(|e| e.to_string())
                }
                RemoteProtocol::S3 => {
                    // Server is empty for Amazon itself, else the host of
                    // a compatible service; the first part of the remote
                    // path is the bucket to open in.
                    let host = params.host.trim();
                    let mut endpoint = String::new();
                    if !host.is_empty() && !host.ends_with("amazonaws.com") {
                        endpoint = host.to_string();
                        if let (Some(port), false) = (params.port, host.contains("://")) {
                            endpoint = format!("{host}:{port}");
                        }
                        // A service on this Mac (MinIO for development)
                        // is plain HTTP unless the address says otherwise.
                        let local = host == "localhost" || host.starts_with("127.");
                        if local && !host.contains("://") {
                            endpoint = format!("http://{endpoint}");
                        }
                    }
                    let bucket =
                        params.remote_path.as_deref().unwrap_or("").trim_matches('/').split('/').next().unwrap_or("");
                    rustyfinder_remote::s3::connect(rustyfinder_remote::s3::S3Params {
                        access_key: params.user,
                        secret_key: params.password.unwrap_or_default(),
                        endpoint,
                        region: params.region.unwrap_or_default(),
                        bucket: bucket.to_string(),
                        part_size: None,
                        path_style: params.path_style,
                        storage_class: params.storage_class.filter(|s| !s.is_empty()),
                    })
                    .map(|fs| {
                        let (label, home) = (fs.label().to_string(), fs.home().to_string());
                        (Arc::new(fs) as Arc<dyn Vfs>, label, home)
                    })
                    .map_err(|e| e.to_string())
                }
                _ => Err("RustyFinder can’t connect to this kind of server yet.".to_string()),
            };
            match result {
                Ok((vfs, label, home)) => {
                    let id = registry.add(vfs, label.clone(), home.clone());
                    listener.connected(ConnectionRecord { id, label, home, archive: false });
                }
                Err(message) => listener.failed(message, false),
            }
        });
    }

    /// Close a connection. Its ssh process ends once nothing uses it any
    /// more. Tell each window with `WindowSession::connection_closed`.
    pub fn disconnect(&self, connection: u64) -> bool {
        self.registry.remove(connection)
    }

    /// Open remote connections, oldest first.
    pub fn connections(&self) -> Vec<ConnectionRecord> {
        self.registry
            .remotes()
            .into_iter()
            .map(|c| ConnectionRecord { id: c.id, label: c.label, home: c.home, archive: c.archive })
            .collect()
    }

    /// The custom tools, in the user's order.
    pub fn tools(&self) -> Vec<ToolRecord> {
        self.store.tools().into_iter().map(Into::into).collect()
    }

    /// Save the tool list (order included). Tools without a name or a
    /// command are dropped; new ones get an id. Returns what was stored.
    pub fn set_tools(&self, tools: Vec<ToolRecord>) -> Vec<ToolRecord> {
        let stored = self.store.set_tools(tools.into_iter().map(Into::into).collect());
        stored.into_iter().map(Into::into).collect()
    }

    /// Run a tool as an activity in `cwd` (a folder on this Mac). The
    /// placeholders are filled from `context`, each value shell-quoted.
    /// The finish report carries what the command printed.
    pub fn run_tool(&self, window: u64, tool: ToolRecord, context: ToolContext, cwd: String) -> Option<u64> {
        let tool: rustyfinder_core::tools::Tool = tool.into();
        let context = rustyfinder_core::tools::Context {
            source_path: context.source_path,
            target_path: context.target_path,
            source_selection: context.source_selection,
            target_selection: context.target_selection,
            hostname: context.hostname,
            username: context.username,
        };
        let request = Request::Shell {
            name: tool.name.clone(),
            title: format!("Running “{}”", tool.name),
            summary: format!("Ran “{}”", tool.name),
            commands: rustyfinder_core::tools::commands(&tool, &context),
            cwd,
        };
        self.activities.get().map(|m| m.submit(window, request))
    }

    /// Compute the digest of each file in `paths` on `connection`, one
    /// after the other, in the background.
    pub fn checksum(
        &self,
        connection: u64,
        paths: Vec<String>,
        algorithm: ChecksumAlgorithm,
        listener: Arc<dyn ChecksumListener>,
    ) -> Arc<ChecksumJob> {
        let job = Arc::new(ChecksumJob { cancelled: std::sync::atomic::AtomicBool::new(false) });
        let registry = Arc::clone(&self.registry);
        let running = Arc::clone(&job);
        std::thread::spawn(move || {
            let keep_going = || !running.cancelled.load(Ordering::SeqCst);
            for (index, path) in paths.iter().enumerate() {
                if !keep_going() {
                    return;
                }
                let index = index as u32;
                let Some(vfs) = registry.get(connection) else {
                    listener.finished(index, None, Some("The connection is closed.".into()));
                    continue;
                };
                // Reported at most every 100 ms, so a big file does not
                // flood the UI.
                let mut last = std::time::Instant::now();
                let mut report = |bytes: u64| {
                    if last.elapsed() >= std::time::Duration::from_millis(100) {
                        last = std::time::Instant::now();
                        listener.progress(index, bytes);
                    }
                };
                let result = vfs.open_read(path).map_err(|e| e.to_string()).and_then(|mut reader| {
                    rustyfinder_core::checksum::digest(&mut *reader, algorithm.into(), &keep_going, &mut report)
                        .map_err(|e| e.to_string())
                });
                match result {
                    Ok(Some(digest)) => listener.finished(index, Some(digest), None),
                    Ok(None) => return,
                    Err(message) => listener.finished(index, None, Some(message)),
                }
            }
        });
        job
    }

    /// Open the archive at `local_file` (a path on this Mac) as a folder.
    /// `origin_connection` and `origin_path` say where the archive really
    /// lives (the same file for a local one; the server and path it was
    /// downloaded from otherwise): going up from the archive's top leads
    /// back there. An archive that is already open is returned as it is.
    pub fn open_archive(
        &self,
        local_file: String,
        origin_connection: u64,
        origin_path: String,
        password: Option<String>,
        listener: Arc<dyn ArchiveListener>,
    ) {
        let registry = Arc::clone(&self.registry);
        std::thread::spawn(move || {
            let origin = rustyfinder_core::Location { conn: origin_connection, path: origin_path };
            let label = rustyfinder_core::paths::file_name(&origin.path).to_string();
            let record = |id: u64| ConnectionRecord { id, label: label.clone(), home: "/".into(), archive: true };
            if let Some(id) = registry.archive_for(&origin) {
                return listener.opened(record(id), Vec::new());
            }
            use rustyfinder_core::archive::ArchiveError;
            match rustyfinder_core::archive::open(&local_file, password.as_deref(), &archive_cache_dir()) {
                Ok(opened) => {
                    let id = registry.add_archive(opened.vfs, label.clone(), origin);
                    listener.opened(record(id), opened.blocked);
                }
                Err(error) => {
                    let reason = match error {
                        ArchiveError::Unrecognized => ArchiveFailure::Unrecognized,
                        ArchiveError::PasswordRequired => ArchiveFailure::PasswordRequired,
                        ArchiveError::WrongPassword => ArchiveFailure::WrongPassword,
                        ArchiveError::Failed(_) => ArchiveFailure::Other,
                    };
                    listener.failed(reason, error.to_string());
                }
            }
        });
    }

    /// Extract an opened archive into `dest_dir` on `dest_connection`: a
    /// single top-level item as itself, several into a folder named after
    /// the archive. Runs as an activity; returns its id.
    pub fn extract(
        &self,
        window: u64,
        archive_connection: u64,
        dest_connection: u64,
        dest_dir: String,
        blocked: Vec<String>,
    ) -> Option<u64> {
        let vfs = self.registry.get(archive_connection)?;
        let archive_name = self.registry.label(archive_connection);
        let (sources, rename_to) = rustyfinder_core::archive::extract_plan(&*vfs, &archive_name).ok()?;
        let request = Request::Extract { archive_name, sources, dest_dir, rename_to, blocked };
        self.activities.get().map(|m| m.submit_on(window, request, archive_connection, dest_connection))
    }

    /// Delete what unpacked archives left in the temporary folder. Call
    /// when quitting.
    pub fn clean_up_archives(&self) {
        let _ = std::fs::remove_dir_all(archive_cache_dir());
    }

    /// Free and total bytes of the volume holding `path` on a connection
    /// (0 = local disk).
    pub fn space_on(&self, connection: u64, path: String) -> Option<SpaceInfo> {
        self.registry.get(connection)?.space(&path).map(|s| SpaceInfo { free: s.free, total: s.total })
    }

    /// Go to Folder: where typed text leads. Understands quotes, shell
    /// escapes, `~` and paths relative to `current_dir`.
    pub fn goto_resolve(&self, connection: u64, text: String, current_dir: String) -> GotoResult {
        use rustyfinder_core::goto::{parse_typed, resolve, Destination};
        let path = parse_typed(&text, &current_dir, &self.registry.home(connection));
        let missing =
            GotoResult { kind: GotoKind::Missing, path: path.clone(), parent: String::new(), name: String::new() };
        let Some(vfs) = self.registry.get(connection) else {
            return missing;
        };
        match resolve(&*vfs, &path) {
            Destination::Folder(path) => {
                GotoResult { kind: GotoKind::Folder, path, parent: String::new(), name: String::new() }
            }
            Destination::File { parent, name } => GotoResult { kind: GotoKind::File, path, parent, name },
            Destination::Missing(_) => missing,
        }
    }

    /// Go to Folder: entries that complete what has been typed so far.
    pub fn goto_complete(&self, connection: u64, text: String, current_dir: String) -> Vec<GotoCompletion> {
        let Some(vfs) = self.registry.get(connection) else {
            return Vec::new();
        };
        rustyfinder_core::goto::complete(&*vfs, &text, &current_dir, &self.registry.home(connection))
            .into_iter()
            .map(|c| GotoCompletion { name: c.name, is_dir: c.is_dir, text: c.text })
            .collect()
    }

    /// Earlier Go to Folder destinations, most recent first.
    pub fn goto_history(&self) -> Vec<String> {
        self.store.goto_history()
    }

    pub fn record_goto(&self, path: String) {
        self.store.record_goto(&path);
    }

    /// Multi-Rename preview. `rules_json` is a JSON array of rules (see
    /// `rustyfinder_core::rename::Rule`); items are in the order shown, which
    /// drives sequence numbers.
    pub fn rename_preview(&self, connection: u64, items: Vec<RenameItem>, rules_json: String) -> RenamePreview {
        area::rename_preview(self, connection, items, Vec::new(), rules_json)
    }

    /// Compare two folders in the background and report the plan.
    /// `options_json` is a `rustyfinder_core::sync::SyncOptions` as JSON.
    pub fn sync_analyze(
        &self,
        left_connection: u64,
        left_root: String,
        right_connection: u64,
        right_root: String,
        options_json: String,
        listener: Arc<dyn SyncListener>,
    ) {
        use rustyfinder_core::sync;
        let registry = Arc::clone(&self.registry);
        std::thread::spawn(move || {
            let failed = |message: String| SyncPlan { rows: Vec::new(), folders_analyzed: 0, error: Some(message) };
            let options: sync::SyncOptions = match serde_json::from_str(&options_json) {
                Ok(options) => options,
                Err(e) => return listener.finished(failed(format!("The options can’t be read: {e}"))),
            };
            let (Some(left), Some(right)) = (registry.get(left_connection), registry.get(right_connection)) else {
                return listener.finished(failed("The connection is closed.".into()));
            };
            let pair = sync::Pair {
                left: &*left,
                left_root: &left_root,
                right: &*right,
                right_root: &right_root,
                // If either side ignores case, names are matched ignoring it.
                case_insensitive: left_connection == rustyfinder_core::LOCAL
                    || right_connection == rustyfinder_core::LOCAL,
            };
            let plan = sync::analyze(&pair, &options, &|| true, &|n| listener.progress(n));
            let plan = plan.unwrap_or_default();
            listener.finished(SyncPlan {
                folders_analyzed: plan.folders_analyzed,
                rows: plan
                    .rows
                    .into_iter()
                    .map(|r| SyncRow {
                        path: r.path,
                        action: match r.action {
                            sync::Action::Add => SyncAction::Add,
                            sync::Action::Update => SyncAction::Update,
                            sync::Action::Delete => SyncAction::Delete,
                        },
                        to_right: r.target == sync::Side::Right,
                        is_dir: r.is_dir,
                        size: r.size,
                        left_size: r.left.map(|i| i.size),
                        left_modified_ms: r.left.and_then(|i| i.modified_ms),
                        right_size: r.right.map(|i| i.size),
                        right_modified_ms: r.right.and_then(|i| i.modified_ms),
                        conflict: r.conflict,
                        checked: r.checked,
                    })
                    .collect(),
                error: None,
            });
        });
    }

    /// Work out a folder's recursive size in the background and report it.
    pub fn measure(&self, path: String, listener: Arc<dyn MeasureListener>) {
        let vfs = Arc::clone(&self.vfs);
        std::thread::spawn(move || {
            if let Some((items, bytes)) = rustyfinder_core::perm::measure(&*vfs, &path, &|| true) {
                listener.measured(path, items, bytes);
            }
        });
    }

    /// What dropping `sources` onto the folder `target` does, or `None`
    /// if the drop is pointless. `option` and `command` are the modifier
    /// keys held: Option copies, Command moves; otherwise a drag moves
    /// within a volume and copies across volumes.
    pub fn drop_mode(&self, sources: Vec<String>, target: String, option: bool, command: bool) -> Option<OpMode> {
        rustyfinder_core::ops::simple::drop_mode(&*self.vfs, &sources, &target, option, command, false).map(|m| match m
        {
            TransferMode::Copy => OpMode::Copy,
            TransferMode::Move => OpMode::Move,
        })
    }

    /// Free and total bytes of the volume holding `path`.
    pub fn space(&self, path: String) -> Option<SpaceInfo> {
        self.vfs.space(&path).map(|s| SpaceInfo { free: s.free, total: s.total })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::Mutex;
    use std::time::Duration;

    enum Event {
        Layout,
        Ready(u64, Arc<SnapshotHandle>, PaneInfo),
        Failed(u64, RustyFinderError),
    }

    struct Chan(Mutex<Sender<Event>>);

    impl WindowListener for Chan {
        fn window_changed(&self, _layout: WindowLayout) {
            let _ = self.0.lock().unwrap().send(Event::Layout);
        }
        fn snapshot_ready(&self, tab: u64, snapshot: Arc<SnapshotHandle>, info: PaneInfo) {
            let _ = self.0.lock().unwrap().send(Event::Ready(tab, snapshot, info));
        }
        fn load_failed(&self, tab: u64, _path: String, error: RustyFinderError) {
            let _ = self.0.lock().unwrap().send(Event::Failed(tab, error));
        }
    }

    fn ready_at(rx: &Receiver<Event>, tab: u64, path: &str) -> (Arc<SnapshotHandle>, PaneInfo) {
        loop {
            match rx.recv_timeout(Duration::from_secs(5)).expect("no event") {
                Event::Ready(t, s, i) if t == tab && i.path == path => return (s, i),
                _ => {}
            }
        }
    }

    #[test]
    fn window_round_trip_through_ffi_types() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap().to_str().unwrap().to_string();
        std::fs::create_dir_all(format!("{root}/home/dir")).unwrap();
        std::fs::write(format!("{root}/home/b.txt"), b"abc").unwrap();
        std::fs::write(format!("{root}/home/a.TXT"), b"").unwrap();
        let home = format!("{root}/home");

        let app = RustyFinderApp::new(format!("{root}/support"), home.clone());
        assert_eq!(app.starting_directory(), home);
        let (tx, rx) = channel();
        let win = app.open_window(Arc::new(Chan(Mutex::new(tx))), None);
        let layout = win.layout();
        assert_eq!(layout.active, PaneSide::Left);
        let left = layout.left.tabs[0].id;

        let (snap, info) = ready_at(&rx, left, &home);
        assert_eq!(info.reason, ChangeReason::Navigated);
        assert_eq!(snap.count(), 3);
        let rows = snap.rows(0, 100);
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["dir", "a.TXT", "b.txt"]);
        assert_eq!(rows[1].extension, "txt");
        assert_eq!(rows[2].size, 3);
        assert_eq!(snap.path_of(2).unwrap(), format!("{home}/b.txt"));
        assert_eq!(snap.find_prefix("B".into(), 0), Some(2));
        assert_eq!(snap.indices_of(vec!["b.txt".into(), "dir".into()]), vec![0, 2]);
        assert!(snap.row(3).is_none());
        assert_eq!(snap.rows(2, u32::MAX).len(), 1);

        win.navigate(left, format!("{home}/nope"));
        loop {
            match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                Event::Failed(t, RustyFinderError::NotFound { .. }) if t == left => break,
                _ => {}
            }
        }

        let tab = win.new_tab(PaneSide::Left, Some(format!("{home}/dir")));
        ready_at(&rx, tab, &format!("{home}/dir"));
        assert_eq!(win.layout().left.tabs.len(), 2);
        assert_eq!(win.close_tab(tab), CloseResult::Closed);
        assert_eq!(win.close_tab(left), CloseResult::LastTab);

        // Session round trip.
        app.save_session(vec![win.saved_json("frame=1".into())]);
        let saved = app.saved_windows();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].ui, "frame=1");
        let (tx2, _rx2) = channel();
        let again = app.open_window(Arc::new(Chan(Mutex::new(tx2))), Some(saved[0].json.clone()));
        assert_eq!(again.layout().left.tabs[0].path, home);

        assert!(app.space(home.clone()).unwrap().total > 0);
        assert_eq!(app.recent_folders()[0], format!("{home}/dir"));
        // Opening and closing tabs announced layout changes along the way.
        assert!(rx.try_iter().any(|e| matches!(e, Event::Layout)));
    }

    #[test]
    fn favorites_and_settings_through_ffi() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_str().unwrap().to_string();
        let app = RustyFinderApp::new(format!("{dir}/support"), dir.clone());
        let fav = app.add_favorite("Work".into(), "P".into(), "/Projects".into()).unwrap();
        let err = app.add_favorite("Work".into(), "p".into(), "/x".into()).unwrap_err();
        assert!(matches!(err, RustyFinderError::Invalid { .. }));
        assert!(err.to_string().contains("already taken"));
        app.rename_favorite(fav.id, "Q".into()).unwrap();
        let groups = app.favorite_groups();
        let work = groups.iter().find(|g| g.name == "Work").unwrap();
        assert_eq!(work.items[0].name, "Q");
        app.remove_favorite(fav.id).unwrap();
        assert!(app.remove_favorite(fav.id).is_err());

        let mut s = app.settings();
        assert!(!s.show_hidden && s.restore_windows);
        s.show_hidden = true;
        app.set_settings(s);
        assert!(RustyFinderApp::new(format!("{dir}/support"), dir).settings().show_hidden);
    }

    struct Sink(Mutex<Sender<FinishRecord>>);

    impl ActivityListener for Sink {
        fn activities_changed(&self, _activities: Vec<ActivityRecord>) {}
        fn conflict_prompt(&self, _activity: u64, _conflict: ConflictRecord) {}
        fn error_prompt(&self, _activity: u64, _path: String, _message: String, _can_elevate: bool) {}
        fn finished(&self, report: FinishRecord) {
            let _ = self.0.lock().unwrap().send(report);
        }
    }

    struct MoveAside(String);

    impl PlatformServices for MoveAside {
        fn trash(&self, path: String) -> Result<String, RustyFinderError> {
            let target = format!("{}/{}", self.0, paths::file_name(&path));
            std::fs::rename(&path, &target).map_err(|e| RustyFinderError::Io { message: e.to_string() })?;
            Ok(target)
        }

        fn make_alias(&self, target: String, alias: String) -> Result<(), RustyFinderError> {
            std::fs::write(alias, target).map_err(|e| RustyFinderError::Io { message: e.to_string() })
        }

        fn set_tags(&self, path: String, tags: Vec<String>) -> Result<(), RustyFinderError> {
            std::fs::write(format!("{path}.tags"), tags.join(","))
                .map_err(|e| RustyFinderError::Io { message: e.to_string() })
        }

        fn set_comment(&self, path: String, comment: String) -> Result<(), RustyFinderError> {
            std::fs::write(format!("{path}.comment"), comment)
                .map_err(|e| RustyFinderError::Io { message: e.to_string() })
        }

        fn elevate(&self, _step: local_ops::PrivilegedStep) -> Result<String, RustyFinderError> {
            Err(RustyFinderError::Unsupported { message: "no administrator here".into() })
        }
    }

    #[test]
    fn file_operations_through_ffi() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_str().unwrap().to_string();
        for d in ["src", "dst", "trash"] {
            std::fs::create_dir(format!("{dir}/{d}")).unwrap();
        }
        std::fs::write(format!("{dir}/src/a.txt"), b"a").unwrap();
        let app = RustyFinderApp::new(format!("{dir}/support"), dir.clone());
        // Nothing can run before the shell is attached.
        assert!(app.submit(1, FileOp::NewFolder { dir: format!("{dir}/src") }).is_none());

        let (tx, rx) = channel();
        app.attach(Arc::new(Sink(Mutex::new(tx))), Arc::new(MoveAside(format!("{dir}/trash"))));
        let next = || rx.recv_timeout(Duration::from_secs(5)).expect("no finish report");

        app.submit(
            7,
            FileOp::Transfer {
                mode: OpMode::Copy,
                sources: vec![format!("{dir}/src/a.txt")],
                dest_dir: format!("{dir}/dst"),
                rename_to: None,
            },
        )
        .unwrap();
        let report = next();
        assert_eq!((report.window, report.result), (7, OpResult::Success));
        assert_eq!(report.results, vec![format!("{dir}/dst/a.txt")]);

        app.submit(7, FileOp::Trash { paths: vec![format!("{dir}/dst/a.txt")] }).unwrap();
        next();
        assert!(std::fs::metadata(format!("{dir}/trash/a.txt")).is_ok());
        assert_eq!(app.undo_title(7).as_deref(), Some("Undo Move to Trash"));
        app.undo(7).unwrap();
        next();
        assert!(std::fs::metadata(format!("{dir}/dst/a.txt")).is_ok());
        assert_eq!(app.redo_title(7).as_deref(), Some("Redo Move to Trash"));

        app.submit(7, FileOp::Rename { path: format!("{dir}/dst/a.txt"), new_name: "b:c".into() }).unwrap();
        let refused = next();
        assert_eq!(refused.result, OpResult::Failed);
        assert!(refused.message.unwrap().contains("colon"));

        let log = app.operation_log();
        assert_eq!(log.first().unwrap().title, "Copied 1 item");
        assert_eq!(log.last().unwrap().result, OpResult::Failed);
        assert!(!app.has_active(None));
        assert!(app.activities().is_empty());
    }

    #[test]
    fn path_components_for_path_bar() {
        let c = path_components("/Users/me".into());
        assert_eq!(c.len(), 3);
        assert_eq!(c[2].label, "me");
        assert_eq!(c[2].path, "/Users/me");
    }
}
