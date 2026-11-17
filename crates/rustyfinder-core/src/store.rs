//! App-wide persistent state: settings, favorites, recent folders and the
//! saved window session. Each lives in its own JSON file in the app's
//! support directory, written atomically.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::ops::activity::LogEntry;
use crate::ops::engine::{ConflictPolicy, ErrorPolicy, ResumePolicy};
use crate::paths;
use crate::snapshot::ViewSpec;
use crate::tools::Tool;
use crate::vfs::Vfs;

mod favorites;
mod themes;
pub use favorites::{FavoriteDraft, TagEntry};

/// Maximum entries in Recent Folders (panes-tabs spec §5).
pub const MAX_RECENTS: usize = 10;

/// Maximum remembered Go to Folder destinations.
pub const MAX_GOTO_HISTORY: usize = 20;

/// User settings. Every field has a default, so files written by older
/// versions keep loading as fields are added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Folder new windows and tabs open at; empty means the home folder.
    pub starting_directory: String,
    pub restore_windows: bool,
    pub show_hidden: bool,
    pub always_show_tab_bar: bool,
    pub folders_on_top: bool,
    pub relative_dates: bool,
    pub alternating_rows: bool,
    pub treat_packages_as_folders: bool,
    /// Opening an archive browses it like a folder (off: the default app
    /// opens it).
    pub treat_archives_as_folders: bool,
    pub confirm_drag_operations: bool,
    pub play_sounds: bool,
    /// Text size in List view, in points.
    pub list_text_size: u32,
    /// Icon size in Icon view, in points.
    pub icon_size: u32,
    /// Transfers that may run at once (1 to 10).
    pub max_transfers: u32,
    /// What to do when a file would replace another.
    pub replace_policy: ConflictPolicy,
    /// What to do when a folder meets a folder of the same name.
    pub merge_policy: ConflictPolicy,
    pub error_policy: ErrorPolicy,
    /// The Terminal application "Open in Terminal" uses, as a bundle
    /// path; empty means Apple's Terminal.
    pub terminal_app: String,
    /// Moving an `.app` to the Trash opens App Deleter (app-deleter spec).
    pub detect_app_when_deleting: bool,
    /// Which pane a newly opened server goes to.
    pub open_remote_in: RemoteSide,
    /// Extracting and compressing use RustyFinder's own engine (off: the
    /// system's Archive Utility).
    pub handle_archives: bool,
    /// "Continue without Full Disk Access" was chosen: the general prompt
    /// stays away.
    pub fda_dismissed: bool,
    /// The first-run welcome has been shown. Missing from files written
    /// before it existed, which therefore count as shown.
    pub welcome_shown: bool,
    /// Id of the theme in use ("default" follows macOS).
    pub theme: String,
    /// A favorite to start at instead of `starting_directory` (0: none).
    pub starting_favorite: u64,
    /// Custom tools run through the login shell and get its `$PATH`.
    pub tools_inherit_path: bool,
    /// The external compare tool (`compare_tools::CompareTool` key).
    pub compare_tool: String,
    /// What to do when a stopped transfer left a partial file.
    pub resume_policy: ResumePolicy,
    /// Failed transfers are retried on their own before anyone is asked.
    pub auto_retry: bool,
    /// How often (1 to 10).
    pub max_retries: u32,
    /// Bandwidth limits in KB/s, for all downloads and all uploads together.
    pub limit_downloads: bool,
    pub download_limit_kbps: u32,
    pub limit_uploads: bool,
    pub upload_limit_kbps: u32,
}

/// Where the Connect panel and remote favorites open a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RemoteSide {
    #[default]
    Active,
    Left,
    Right,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            starting_directory: String::new(),
            restore_windows: true,
            show_hidden: false,
            always_show_tab_bar: false,
            folders_on_top: true,
            relative_dates: true,
            alternating_rows: false,
            treat_packages_as_folders: false,
            treat_archives_as_folders: true,
            confirm_drag_operations: false,
            play_sounds: true,
            list_text_size: 13,
            icon_size: 64,
            max_transfers: 3,
            replace_policy: ConflictPolicy::Ask,
            merge_policy: ConflictPolicy::Ask,
            error_policy: ErrorPolicy::Ask,
            terminal_app: String::new(),
            detect_app_when_deleting: false,
            open_remote_in: RemoteSide::Active,
            handle_archives: true,
            fda_dismissed: false,
            welcome_shown: true,
            theme: crate::theme::DEFAULT_THEME.to_string(),
            starting_favorite: 0,
            tools_inherit_path: true,
            compare_tool: "filemerge".into(),
            resume_policy: ResumePolicy::Ask,
            auto_retry: false,
            max_retries: 3,
            limit_downloads: false,
            download_limit_kbps: 1024,
            limit_uploads: false,
            upload_limit_kbps: 1024,
        }
    }
}

/// A saved window layout the user gave a name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: u64,
    pub name: String,
    pub window: SavedWindow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Favorite {
    pub id: u64,
    pub name: String,
    /// A local file or folder; for a server, the remote path it opens at.
    pub path: String,
    /// SF Symbol name, or the absolute path of an image file; empty means
    /// the item's own icon.
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub kind: FavoriteKind,
    /// The server of a connection or share favorite. Never a password:
    /// that stays in the keychain under `ServerSpec::keychain`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerSpec>,
    /// What a synclet, rename preset or saved search holds, as the window
    /// that opens it understands it (JSON); a workspace's id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub payload: String,
    /// Paths saved under a server favorite ("favorites within favorites").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<FavoritePath>,
}

/// What a favorite opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FavoriteKind {
    /// A local folder or file (which one is decided when it is opened).
    #[default]
    Path,
    /// A server spoken to by RustyFinder (SFTP, FTP, WebDAV, S3).
    Connection,
    /// A network share mounted by the system (SMB, AFP, NFS).
    Share,
    Synclet,
    RenamePreset,
    SavedSearch,
    Workspace,
}

/// A server, as a favorite remembers it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerSpec {
    /// "sftp", "ftp", "ftps", "webdav", "webdavs", "s3", "smb", "afp", "nfs".
    pub protocol: String,
    pub host: String,
    pub port: Option<u16>,
    pub user: String,
    /// Keychain account holding the password; empty when there is none.
    pub keychain: String,
    /// Protocol options the Connect panel offers (passive mode, encoding,
    /// region...), by name.
    pub options: std::collections::BTreeMap<String, String>,
}

impl ServerSpec {
    /// "sftp://bob@example.org:2222": the server, with no path or password.
    pub fn url(&self) -> String {
        let mut text = format!("{}://", self.protocol);
        if !self.user.is_empty() {
            text.push_str(&self.user);
            text.push('@');
        }
        text.push_str(&self.host);
        if let Some(port) = self.port {
            text.push_str(&format!(":{port}"));
        }
        text
    }
}

/// A remote path saved under a server favorite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FavoritePath {
    pub id: u64,
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FavoriteGroup {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default)]
    pub items: Vec<Favorite>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FavoriteError {
    #[error("The name “{0}” is already taken.")]
    NameTaken(String),
    #[error("The name can’t be empty.")]
    EmptyName,
    #[error("That favorite no longer exists.")]
    Missing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SavedTab {
    pub path: String,
    pub pinned: bool,
    pub view: ViewSpec,
    /// For a tab on a server: the server's URL (no password); `path` is
    /// then the path there. The tab reconnects when it is first shown.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub server: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SavedPane {
    pub tabs: Vec<SavedTab>,
    pub selected: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SavedWindow {
    pub left: SavedPane,
    pub right: SavedPane,
    pub right_active: bool,
    pub single_pane: bool,
    pub horizontal: bool,
    pub sync_browsing: bool,
    /// Opaque UI state owned by the shell (frame, divider, sidebar...).
    pub ui: String,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct FavoritesFile {
    groups: Vec<FavoriteGroup>,
}

pub struct Store {
    dir: PathBuf,
    home: String,
    settings: Mutex<Settings>,
    favorites: Mutex<Vec<FavoriteGroup>>,
    recents: Mutex<Vec<String>>,
    /// Recent folders on servers, by server URL.
    remote_recents: Mutex<std::collections::BTreeMap<String, Vec<String>>>,
    next_id: AtomicU64,
}

impl Store {
    /// Load everything from `dir` (created if missing). Unreadable or
    /// corrupt files fall back to defaults rather than failing the launch.
    pub fn open(dir: impl Into<PathBuf>, home: impl Into<String>) -> Self {
        let dir = dir.into();
        let home = home.into();
        let _ = fs::create_dir_all(&dir);
        let fresh = !dir.join("settings.json").exists();
        let mut settings: Settings = read_json(&dir.join("settings.json")).unwrap_or_default();
        // A fresh install has not seen the welcome yet; older settings
        // files, which lack the field, have.
        if fresh {
            settings.welcome_shown = false;
        }
        let recents: Vec<String> = read_json(&dir.join("recents.json")).unwrap_or_default();
        let favorites = match read_json::<FavoritesFile>(&dir.join("favorites.json")) {
            Some(file) => file.groups,
            None => Vec::new(),
        };
        let remote_recents = read_json(&dir.join("remote-recents.json")).unwrap_or_default();
        let max_id = favorites
            .iter()
            .flat_map(|g| {
                std::iter::once(g.id)
                    .chain(g.items.iter().map(|f| f.id))
                    .chain(g.items.iter().flat_map(|f| f.children.iter().map(|c| c.id)))
            })
            .max()
            .unwrap_or(0);
        let store = Self {
            dir,
            home,
            settings: Mutex::new(settings),
            favorites: Mutex::new(favorites),
            recents: Mutex::new(recents),
            remote_recents: Mutex::new(remote_recents),
            next_id: AtomicU64::new(max_id + 1),
        };
        store.seed_favorites();
        store.adopt_workspaces();
        store
    }

    pub fn home(&self) -> &str {
        &self.home
    }

    /// The folder the settings files live in.
    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    // --- Settings

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    pub fn update_settings(&self, change: impl FnOnce(&mut Settings)) -> Settings {
        let mut s = self.settings.lock().unwrap();
        change(&mut s);
        write_json(&self.dir.join("settings.json"), &*s);
        s.clone()
    }

    /// The file where stopped transfers record their partial files.
    pub fn partials_file(&self) -> PathBuf {
        self.support_file("partials.json")
    }

    /// A file of this name in the app's support folder, for state kept by
    /// other parts (saved connections, trusted certificates).
    pub fn support_file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Where new windows and tabs open: the chosen favorite if it is a
    /// local folder that still is one, else the chosen folder, else home.
    pub fn starting_directory(&self) -> String {
        let (configured, favorite) = {
            let s = self.settings.lock().unwrap();
            (s.starting_directory.clone(), s.starting_favorite)
        };
        if favorite != 0 {
            if let Some(f) = self.favorite(favorite) {
                if f.kind == FavoriteKind::Path && Path::new(&f.path).is_dir() {
                    return f.path;
                }
            }
        }
        if configured.is_empty() {
            self.home.clone()
        } else {
            paths::normalize(&configured, &self.home)
        }
    }

    // --- Favorites

    /// First launch: one "Favorites" group with the usual home folders.
    fn seed_favorites(&self) {
        let mut groups = self.favorites.lock().unwrap();
        if !groups.is_empty() || self.dir.join("favorites.json").exists() {
            return;
        }
        let home = self.home.trim_end_matches('/');
        let candidates = [
            ("Home", home.to_string(), "house"),
            ("Desktop", format!("{home}/Desktop"), "menubar.dock.rectangle"),
            ("Documents", format!("{home}/Documents"), "doc"),
            ("Downloads", format!("{home}/Downloads"), "arrow.down.circle"),
            ("Applications", "/Applications".to_string(), "square.grid.2x2"),
        ];
        let items = candidates
            .into_iter()
            .filter(|(_, path, _)| Path::new(path).is_dir())
            .map(|(name, path, icon)| Favorite { icon: icon.to_string(), ..Favorite::new(self.new_id(), name, &path) })
            .collect();
        groups.push(FavoriteGroup { id: self.new_id(), name: "Favorites".to_string(), collapsed: false, items });
        self.save_favorites(&groups);
    }

    pub fn favorite_groups(&self) -> Vec<FavoriteGroup> {
        self.favorites.lock().unwrap().clone()
    }

    /// Add a favorite to the group named `group` (created if missing).
    /// Names must be unique within a group, ignoring case.
    pub fn add_favorite(&self, group: &str, name: &str, path: &str) -> Result<Favorite, FavoriteError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(FavoriteError::EmptyName);
        }
        let mut groups = self.favorites.lock().unwrap();
        let idx = match groups.iter().position(|g| g.name == group) {
            Some(i) => i,
            None => {
                groups.push(FavoriteGroup {
                    id: self.new_id(),
                    name: group.to_string(),
                    collapsed: false,
                    items: Vec::new(),
                });
                groups.len() - 1
            }
        };
        if groups[idx].items.iter().any(|f| same_name(&f.name, name)) {
            return Err(FavoriteError::NameTaken(name.to_string()));
        }
        let favorite = Favorite::new(self.new_id(), name, path);
        groups[idx].items.push(favorite.clone());
        self.save_favorites(&groups);
        Ok(favorite)
    }

    /// Remove a favorite. A workspace favorite takes its workspace along.
    pub fn remove_favorite(&self, id: u64) -> Result<(), FavoriteError> {
        let mut groups = self.favorites.lock().unwrap();
        let mut removed = None;
        for g in groups.iter_mut() {
            if let Some(pos) = g.items.iter().position(|f| f.id == id) {
                removed = Some(g.items.remove(pos));
            }
        }
        let removed = removed.ok_or(FavoriteError::Missing)?;
        self.save_favorites(&groups);
        drop(groups);
        if removed.kind == FavoriteKind::Workspace {
            if let Ok(workspace) = removed.payload.parse() {
                self.remove_workspace(workspace);
            }
        }
        Ok(())
    }

    pub fn rename_favorite(&self, id: u64, name: &str) -> Result<(), FavoriteError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(FavoriteError::EmptyName);
        }
        let mut groups = self.favorites.lock().unwrap();
        let group = groups.iter_mut().find(|g| g.items.iter().any(|f| f.id == id)).ok_or(FavoriteError::Missing)?;
        if group.items.iter().any(|f| f.id != id && same_name(&f.name, name)) {
            return Err(FavoriteError::NameTaken(name.to_string()));
        }
        if let Some(f) = group.items.iter_mut().find(|f| f.id == id) {
            f.name = name.to_string();
        }
        self.save_favorites(&groups);
        Ok(())
    }

    /// Move a favorite to `index` within the group `group_id` (reorder, or
    /// move between groups). `index` is clamped.
    pub fn move_favorite(&self, id: u64, group_id: u64, index: usize) -> Result<(), FavoriteError> {
        let mut groups = self.favorites.lock().unwrap();
        if !groups.iter().any(|g| g.id == group_id) {
            return Err(FavoriteError::Missing);
        }
        let mut moved = None;
        for g in groups.iter_mut() {
            if let Some(pos) = g.items.iter().position(|f| f.id == id) {
                moved = Some(g.items.remove(pos));
                break;
            }
        }
        let moved = moved.ok_or(FavoriteError::Missing)?;
        let target = groups.iter_mut().find(|g| g.id == group_id).unwrap();
        let index = index.min(target.items.len());
        target.items.insert(index, moved);
        self.save_favorites(&groups);
        Ok(())
    }

    pub fn set_group_collapsed(&self, group_id: u64, collapsed: bool) {
        let mut groups = self.favorites.lock().unwrap();
        if let Some(g) = groups.iter_mut().find(|g| g.id == group_id) {
            g.collapsed = collapsed;
            self.save_favorites(&groups);
        }
    }

    fn save_favorites(&self, groups: &[FavoriteGroup]) {
        write_json(&self.dir.join("favorites.json"), &FavoritesFile { groups: groups.to_vec() });
    }

    fn new_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    // --- Recent folders

    /// Note a visit. The newest is first, each folder appears once, and the
    /// list is capped at `MAX_RECENTS`.
    pub fn record_recent(&self, path: &str) {
        let mut recents = self.recents.lock().unwrap();
        if recents.first().map(String::as_str) == Some(path) {
            return;
        }
        recents.retain(|p| p != path);
        recents.insert(0, path.to_string());
        recents.truncate(MAX_RECENTS);
        write_json(&self.dir.join("recents.json"), &*recents);
    }

    /// Recent folders, newest first. Folders that no longer exist are
    /// dropped from the list for good.
    pub fn recent_folders(&self, vfs: &dyn Vfs) -> Vec<String> {
        let mut recents = self.recents.lock().unwrap();
        let before = recents.len();
        recents.retain(|p| vfs.stat(p).is_ok_and(|e| e.is_dir));
        if recents.len() != before {
            write_json(&self.dir.join("recents.json"), &*recents);
        }
        recents.clone()
    }

    // --- Go to Folder history

    /// Destinations reached through Go to Folder, most recent first.
    pub fn goto_history(&self) -> Vec<String> {
        read_json(&self.dir.join("goto.json")).unwrap_or_default()
    }

    /// Remember a destination: it moves to the front, appears once, and
    /// the list keeps the latest `MAX_GOTO_HISTORY`.
    pub fn record_goto(&self, path: &str) {
        let mut history = self.goto_history();
        history.retain(|p| p != path);
        history.insert(0, path.to_string());
        history.truncate(MAX_GOTO_HISTORY);
        write_json(&self.dir.join("goto.json"), &history);
    }

    // --- Custom tools

    pub fn tools(&self) -> Vec<Tool> {
        read_json(&self.dir.join("tools.json")).unwrap_or_default()
    }

    /// Save the tool list in this order. New tools (id 0) get an id of
    /// their own; tools without a name or a command are not saved. Returns
    /// the list as stored.
    pub fn set_tools(&self, tools: Vec<Tool>) -> Vec<Tool> {
        let mut next = tools.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        let mut seen = std::collections::HashSet::new();
        let stored: Vec<Tool> = tools
            .into_iter()
            .filter(Tool::is_valid)
            .map(|mut tool| {
                if tool.id == 0 || !seen.insert(tool.id) {
                    tool.id = next;
                    next += 1;
                    seen.insert(tool.id);
                }
                tool
            })
            .collect();
        write_json(&self.dir.join("tools.json"), &stored);
        stored
    }

    // --- Operation log

    pub fn load_log(&self) -> Vec<LogEntry> {
        read_json(&self.dir.join("log.json")).unwrap_or_default()
    }

    pub fn save_log(&self, entries: &[LogEntry]) {
        write_json(&self.dir.join("log.json"), &entries);
    }

    // --- Session

    pub fn saved_windows(&self) -> Vec<SavedWindow> {
        read_json(&self.dir.join("session.json")).unwrap_or_default()
    }

    pub fn save_windows(&self, windows: &[SavedWindow]) {
        write_json(&self.dir.join("session.json"), &windows);
    }

    // --- Workspaces

    pub fn workspaces(&self) -> Vec<Workspace> {
        read_json(&self.dir.join("workspaces.json")).unwrap_or_default()
    }

    /// Save a window under a new name. `None` if the name is empty or
    /// already taken (names compare without case or accents).
    pub fn add_workspace(&self, name: &str, window: SavedWindow) -> Option<u64> {
        self.add_workspace_in(favorites::WORKSPACES_GROUP, name, window)
    }

    /// Replace what a workspace holds with the window as it is now.
    pub fn update_workspace(&self, id: u64, window: SavedWindow) -> bool {
        let mut all = self.workspaces();
        let Some(workspace) = all.iter_mut().find(|w| w.id == id) else { return false };
        workspace.window = window;
        write_json(&self.dir.join("workspaces.json"), &all);
        true
    }

    pub fn remove_workspace(&self, id: u64) -> bool {
        let mut all = self.workspaces();
        let before = all.len();
        all.retain(|w| w.id != id);
        if all.len() == before {
            return false;
        }
        write_json(&self.dir.join("workspaces.json"), &all);
        self.unlist_workspace(id);
        true
    }

    // --- Shortcuts

    /// The user's changes to the default shortcuts (command to binding).
    pub fn custom_shortcuts(&self) -> std::collections::BTreeMap<String, String> {
        read_json(&self.dir.join("shortcuts.json")).unwrap_or_default()
    }

    pub fn save_custom_shortcuts(&self, custom: &std::collections::BTreeMap<String, String>) {
        write_json(&self.dir.join("shortcuts.json"), custom);
    }

    // --- Editors (Settings → Editing)

    pub fn editors(&self) -> Vec<crate::editors::Editor> {
        read_json(&self.dir.join("editors.json")).unwrap_or_default()
    }

    /// Save the list, tidied (see `editors::normalize`). Returns what was saved.
    pub fn set_editors(&self, editors: Vec<crate::editors::Editor>) -> Vec<crate::editors::Editor> {
        let tidy = crate::editors::normalize(editors);
        write_json(&self.dir.join("editors.json"), &tidy);
        tidy
    }
}

fn same_name(a: &str, b: &str) -> bool {
    crate::sort::fold(a) == crate::sort::fold(b)
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Write via a temp file and rename, so a crash mid-write never leaves a
/// truncated file behind.
fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) {
    let Ok(bytes) = serde_json::to_vec_pretty(value) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, bytes).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspaces_are_saved_by_name_updated_and_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("support"), "/h");
        let window = |ui: &str| SavedWindow { ui: ui.to_string(), ..SavedWindow::default() };
        let a = store.add_workspace(" Work ", window("one")).unwrap();
        assert_eq!(store.add_workspace("work", window("x")), None, "the name is taken");
        assert_eq!(store.add_workspace("  ", window("x")), None);
        let b = store.add_workspace("Home", window("two")).unwrap();
        assert_ne!(a, b);
        assert!(store.update_workspace(a, window("changed")));
        assert!(!store.update_workspace(99, window("x")));
        // Still there after a relaunch.
        let again = Store::open(tmp.path().join("support"), "/h").workspaces();
        assert_eq!(
            again.iter().map(|w| (w.name.as_str(), w.window.ui.as_str())).collect::<Vec<_>>(),
            [("Work", "changed"), ("Home", "two")]
        );
        assert!(store.remove_workspace(a) && !store.remove_workspace(a));
        assert_eq!(store.workspaces().len(), 1);
    }

    #[test]
    fn custom_shortcuts_are_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("support"), "/h");
        assert!(store.custom_shortcuts().is_empty());
        let custom = std::collections::BTreeMap::from([("Commands/Copy to".to_string(), "f9".to_string())]);
        store.save_custom_shortcuts(&custom);
        assert_eq!(Store::open(tmp.path().join("support"), "/h").custom_shortcuts(), custom);
    }

    #[test]
    fn tools_keep_their_order_get_ids_and_drop_empty_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("support"), "/h");
        assert!(store.tools().is_empty());
        let tool = |name: &str, command: &str, id: u64| Tool {
            id,
            name: name.into(),
            command: command.into(),
            ..Tool::default()
        };
        let saved =
            store.set_tools(vec![tool("b", "ls", 0), tool("a", "pwd", 7), tool("empty", "", 0), tool("dup", "x", 7)]);
        assert_eq!(saved.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["b", "a", "dup"]);
        let ids: Vec<u64> = saved.iter().map(|t| t.id).collect();
        assert_eq!(ids[1], 7);
        assert!(ids[0] != 0 && ids[0] != 7 && ids[2] != 7 && ids[0] != ids[2]);
        // Still there after a relaunch.
        assert_eq!(Store::open(tmp.path().join("support"), "/h").tools(), saved);
    }
    use crate::local::LocalFs;

    fn store() -> (tempfile::TempDir, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join("Documents")).unwrap();
        let store = Store::open(tmp.path().join("support"), home.to_str().unwrap());
        (tmp, store)
    }

    #[test]
    fn spec_sd01_defaults() {
        let (_tmp, store) = store();
        let s = store.settings();
        assert!(s.restore_windows && s.folders_on_top && s.relative_dates);
        assert!(!s.show_hidden && !s.always_show_tab_bar && !s.alternating_rows);
        assert!(!s.treat_packages_as_folders && !s.confirm_drag_operations);
        assert!(s.play_sounds);
        assert_eq!(s.max_transfers, 3);
        assert_eq!(s.replace_policy, ConflictPolicy::Ask);
        assert_eq!(s.merge_policy, ConflictPolicy::Ask);
        assert_eq!(s.error_policy, ErrorPolicy::Ask);
        assert_eq!(store.starting_directory(), store.home());
    }

    #[test]
    fn settings_persist_and_tolerate_unknown_or_missing_fields() {
        let (tmp, store) = store();
        store.update_settings(|s| s.show_hidden = true);
        drop(store);
        let dir = tmp.path().join("support");
        let again = Store::open(&dir, "/h");
        assert!(again.settings().show_hidden);

        fs::write(dir.join("settings.json"), br#"{"always_show_tab_bar":true,"future_field":1}"#).unwrap();
        let s = Store::open(&dir, "/h").settings();
        assert!(s.always_show_tab_bar && s.restore_windows);

        fs::write(dir.join("settings.json"), b"{ not json").unwrap();
        assert_eq!(Store::open(&dir, "/h").settings(), Settings::default());
    }

    #[test]
    fn favorites_are_seeded_once_with_existing_folders_only() {
        let (tmp, store) = store();
        let groups = store.favorite_groups();
        assert_eq!(groups.len(), 1);
        let names: Vec<_> = groups[0].items.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"Home") && names.contains(&"Documents"));
        assert!(!names.contains(&"Desktop")); // not created in the fixture

        // Removing everything must not re-seed on next launch.
        for f in &groups[0].items {
            store.remove_favorite(f.id).unwrap();
        }
        drop(store);
        let again = Store::open(tmp.path().join("support"), "/h");
        assert!(again.favorite_groups()[0].items.is_empty());
    }

    #[test]
    fn spec_fv01_add_favorite_to_named_group() {
        let (_tmp, store) = store();
        let f = store.add_favorite("Work", "P", "/Projects").unwrap();
        let groups = store.favorite_groups();
        let work = groups.iter().find(|g| g.name == "Work").unwrap();
        assert_eq!(work.items, vec![f]);
    }

    #[test]
    fn duplicate_names_in_a_group_are_rejected() {
        let (_tmp, store) = store();
        store.add_favorite("Work", "Site", "/a").unwrap();
        assert_eq!(store.add_favorite("Work", " site ", "/b"), Err(FavoriteError::NameTaken("site".into())));
        assert!(store.add_favorite("Other", "Site", "/b").is_ok());
        assert_eq!(store.add_favorite("Work", "  ", "/c"), Err(FavoriteError::EmptyName));
        let id = store.add_favorite("Work", "Two", "/c").unwrap().id;
        assert!(matches!(store.rename_favorite(id, "SITE"), Err(FavoriteError::NameTaken(_))));
        assert!(store.rename_favorite(id, "Two").is_ok());
    }

    #[test]
    fn favorites_can_be_reordered_and_moved_between_groups() {
        let (_tmp, store) = store();
        let a = store.add_favorite("G", "a", "/a").unwrap();
        let b = store.add_favorite("G", "b", "/b").unwrap();
        let c = store.add_favorite("H", "c", "/c").unwrap();
        let gid = |name: &str| store.favorite_groups().iter().find(|g| g.name == name).unwrap().id;
        store.move_favorite(b.id, gid("G"), 0).unwrap();
        store.move_favorite(c.id, gid("G"), 99).unwrap();
        let groups = store.favorite_groups();
        let g = groups.iter().find(|g| g.name == "G").unwrap();
        let ids: Vec<_> = g.items.iter().map(|f| f.id).collect();
        assert_eq!(ids, vec![b.id, a.id, c.id]);
        assert!(groups.iter().find(|g| g.name == "H").unwrap().items.is_empty());
        assert_eq!(store.move_favorite(999, gid("G"), 0), Err(FavoriteError::Missing));
    }

    #[test]
    fn spec_pt12_recents_keep_last_ten_newest_first_and_drop_deleted() {
        let (tmp, store) = store();
        let mut dirs = Vec::new();
        for i in 0..12 {
            let d = tmp.path().join(format!("d{i}"));
            fs::create_dir(&d).unwrap();
            let d = d.to_str().unwrap().to_string();
            store.record_recent(&d);
            dirs.push(d);
        }
        let recents = store.recent_folders(&LocalFs);
        assert_eq!(recents.len(), 10);
        assert_eq!(recents[0], dirs[11]);
        assert_eq!(recents[9], dirs[2]);

        // Revisiting moves a folder to the front without duplicating it.
        store.record_recent(&dirs[5]);
        let recents = store.recent_folders(&LocalFs);
        assert_eq!(recents[0], dirs[5]);
        assert_eq!(recents.iter().filter(|p| **p == dirs[5]).count(), 1);

        fs::remove_dir(&dirs[11]).unwrap();
        let recents = store.recent_folders(&LocalFs);
        assert_eq!(recents.len(), 9);
        assert!(!recents.contains(&dirs[11]));
    }

    #[test]
    fn spec_qo07_goto_history_is_recent_first_without_duplicates() {
        let (_tmp, store) = store();
        assert!(store.goto_history().is_empty());
        store.record_goto("/a");
        store.record_goto("/b");
        assert_eq!(store.goto_history(), vec!["/b", "/a"]);
        store.record_goto("/a");
        assert_eq!(store.goto_history(), vec!["/a", "/b"]);
        for i in 0..30 {
            store.record_goto(&format!("/d{i}"));
        }
        let history = store.goto_history();
        assert_eq!(history.len(), MAX_GOTO_HISTORY);
        assert_eq!(history[0], "/d29");
    }

    #[test]
    fn editors_round_trip_tidied() {
        let (_tmp, store) = store();
        assert!(store.editors().is_empty());
        let ed = |app: &str, ext: &str, d: bool| crate::editors::Editor {
            app: app.into(),
            extensions: vec![ext.into()],
            is_default: d,
        };
        let saved = store.set_editors(vec![ed("/A.app", ".MD", true), ed("/B.app", "rs", true)]);
        assert_eq!(saved, vec![ed("/A.app", "md", true), ed("/B.app", "rs", false)]);
        assert_eq!(store.editors(), saved);
    }

    #[test]
    fn session_round_trips() {
        let (_tmp, store) = store();
        assert!(store.saved_windows().is_empty());
        let w = SavedWindow {
            left: SavedPane {
                tabs: vec![SavedTab { path: "/a".into(), pinned: true, ..SavedTab::default() }],
                selected: 0,
            },
            right_active: true,
            ui: "frame".into(),
            ..Default::default()
        };
        store.save_windows(std::slice::from_ref(&w));
        assert_eq!(store.saved_windows(), vec![w]);
    }
}
