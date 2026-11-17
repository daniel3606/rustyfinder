//! View settings per folder (View Options, ⌘J).
//!
//! A folder's own settings win; otherwise the nearest enclosing folder
//! whose settings were applied "to this folder and subfolders"; otherwise
//! the defaults ("Use as Defaults"); otherwise the built-in view. Remote
//! folders are keyed by their server, so `/var/www` on two servers are
//! different folders.
//!
//! Only the presentation is kept (layout, sort, grouping, folder order);
//! showing hidden files is a global setting and the filter is per visit.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::group::GroupKey;
use crate::snapshot::{Layout, ViewSpec};
use crate::sort::SortSpec;

/// The part of a view that is remembered per folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FolderView {
    pub layout: Layout,
    pub sort: SortSpec,
    pub group: GroupKey,
    pub folders_by_name: bool,
    /// Also applies to every folder below (unless one has its own).
    pub recursive: bool,
}

impl FolderView {
    pub fn of(view: &ViewSpec) -> Self {
        FolderView {
            layout: view.layout,
            sort: view.sort,
            group: view.group,
            folders_by_name: view.folders_by_name,
            recursive: false,
        }
    }

    /// `base` with this presentation; hidden files and the filter stay.
    pub fn apply_to(&self, base: &ViewSpec) -> ViewSpec {
        ViewSpec {
            layout: self.layout,
            sort: self.sort,
            group: self.group,
            folders_by_name: self.folders_by_name,
            ..base.clone()
        }
    }

    /// Same presentation, ignoring the recursive flag.
    pub fn same_look(&self, other: &FolderView) -> bool {
        FolderView { recursive: false, ..*self } == FolderView { recursive: false, ..*other }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct File {
    defaults: Option<FolderView>,
    /// Key: `<source>\t<path>`, see [`key`].
    folders: BTreeMap<String, FolderView>,
}

pub struct FolderViews {
    file: Option<PathBuf>,
    state: Mutex<File>,
}

/// Map key for a folder on a source ("local" or a server id).
pub fn key(source: &str, path: &str) -> String {
    format!("{source}\t{path}")
}

fn split(key: &str) -> (&str, &str) {
    key.split_once('\t').unwrap_or(("", key))
}

fn is_below(path: &str, folder: &str) -> bool {
    folder == "/" && path != "/" || path.starts_with(folder) && path.as_bytes().get(folder.len()) == Some(&b'/')
}

impl FolderViews {
    /// Settings kept in `folder_views.json` inside `dir`.
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        let file = dir.into().join("folder_views.json");
        let state = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        FolderViews { file: Some(file), state: Mutex::new(state) }
    }

    /// The one instance for a settings folder, shared by every tab.
    pub fn shared(dir: &std::path::Path) -> std::sync::Arc<FolderViews> {
        use std::collections::HashMap;
        use std::sync::{Arc, OnceLock};
        static ALL: OnceLock<Mutex<HashMap<PathBuf, Arc<FolderViews>>>> = OnceLock::new();
        let mut all = ALL.get_or_init(Default::default).lock().unwrap();
        Arc::clone(all.entry(dir.to_path_buf()).or_insert_with(|| Arc::new(FolderViews::open(dir))))
    }

    /// Not saved anywhere (tests, scratch panes).
    pub fn in_memory() -> Self {
        FolderViews { file: None, state: Mutex::new(File::default()) }
    }

    /// The view `key`'s folder should open with, if anything is set for it
    /// or its defaults.
    pub fn lookup(&self, key: &str) -> Option<FolderView> {
        let st = self.state.lock().unwrap();
        if let Some(own) = st.folders.get(key) {
            return Some(*own);
        }
        let (source, path) = split(key);
        // Nearest enclosing folder applied to its subfolders.
        st.folders
            .iter()
            .filter(|(k, v)| {
                let (s, p) = split(k);
                v.recursive && s == source && is_below(path, p)
            })
            .max_by_key(|(k, _)| split(k).1.len())
            .map(|(_, v)| *v)
            .or(st.defaults)
    }

    /// The folder's own settings (not inherited or default ones).
    pub fn own(&self, key: &str) -> Option<FolderView> {
        self.state.lock().unwrap().folders.get(key).copied()
    }

    /// Remember `view` for the folder. Applied recursively, it also clears
    /// the own settings of folders below, so the whole subtree follows.
    pub fn remember(&self, key: &str, view: FolderView) {
        let mut st = self.state.lock().unwrap();
        if view.recursive {
            let (source, path) = split(key);
            st.folders.retain(|k, _| {
                let (s, p) = split(k);
                !(s == source && is_below(p, path))
            });
        }
        st.folders.insert(key.to_string(), view);
        self.save(&st);
    }

    /// Set or clear the "subfolders too" flag of a folder's own settings.
    pub fn remember_flag(&self, key: &str, recursive: bool) {
        let mut st = self.state.lock().unwrap();
        if let Some(v) = st.folders.get_mut(key) {
            v.recursive = recursive;
            self.save(&st);
        }
    }

    /// Restore Default View Settings: drop the folder's own settings.
    pub fn forget(&self, key: &str) {
        let mut st = self.state.lock().unwrap();
        if st.folders.remove(key).is_some() {
            self.save(&st);
        }
    }

    pub fn defaults(&self) -> Option<FolderView> {
        self.state.lock().unwrap().defaults
    }

    /// Use as Defaults: folders with nothing of their own open like this.
    pub fn set_defaults(&self, view: FolderView) {
        let mut st = self.state.lock().unwrap();
        st.defaults = Some(FolderView { recursive: false, ..view });
        self.save(&st);
    }

    fn save(&self, st: &File) {
        let Some(file) = &self.file else { return };
        if let Ok(json) = serde_json::to_vec_pretty(st) {
            // Write a sibling and rename, so a crash never leaves half a file.
            let tmp = file.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, file);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sort::SortKey;

    fn icons() -> FolderView {
        FolderView { layout: Layout::Icons, ..Default::default() }
    }

    fn by_size(recursive: bool) -> FolderView {
        FolderView {
            sort: SortSpec { key: SortKey::Size, ascending: false, folders_first: true },
            recursive,
            ..Default::default()
        }
    }

    /// VW-07: a folder keeps its own view; its siblings don't get it.
    #[test]
    fn this_folder_only() {
        let v = FolderViews::in_memory();
        v.remember(&key("local", "/p/a"), icons());
        assert_eq!(v.lookup(&key("local", "/p/a")), Some(icons()));
        assert_eq!(v.lookup(&key("local", "/p/b")), None);
        assert_eq!(v.lookup(&key("local", "/p/a/inner")), None);
    }

    /// VW-08: applied to subfolders, the deeper folders follow.
    #[test]
    fn this_folder_and_subfolders() {
        let v = FolderViews::in_memory();
        v.remember(&key("local", "/p/q/old"), icons());
        v.remember(&key("local", "/p"), by_size(true));
        assert_eq!(v.lookup(&key("local", "/p/q")), Some(by_size(true)));
        // The subtree's own settings were replaced.
        assert_eq!(v.lookup(&key("local", "/p/q/old")), Some(by_size(true)));
        assert_eq!(v.lookup(&key("local", "/pq")), None, "a sibling with a longer name is not below");
        // A later own setting below still wins.
        v.remember(&key("local", "/p/q/new"), icons());
        assert_eq!(v.lookup(&key("local", "/p/q/new")), Some(icons()));
    }

    #[test]
    fn nearest_recursive_folder_wins() {
        let v = FolderViews::in_memory();
        v.remember(&key("local", "/"), by_size(true));
        v.remember(&key("local", "/a"), FolderView { recursive: true, ..icons() });
        assert_eq!(v.lookup(&key("local", "/a/b/c")).map(|f| f.layout), Some(Layout::Icons));
        assert_eq!(v.lookup(&key("local", "/z")).map(|f| f.sort.key), Some(SortKey::Size));
    }

    #[test]
    fn servers_are_kept_apart() {
        let v = FolderViews::in_memory();
        v.remember(&key("sftp://me@one", "/var/www"), icons());
        assert_eq!(v.lookup(&key("sftp://me@two", "/var/www")), None);
        assert_eq!(v.lookup(&key("local", "/var/www")), None);
    }

    #[test]
    fn defaults_and_restore() {
        let v = FolderViews::in_memory();
        v.set_defaults(by_size(true));
        assert_eq!(v.lookup(&key("local", "/x")), Some(by_size(false)));
        v.remember(&key("local", "/x"), icons());
        v.forget(&key("local", "/x"));
        assert_eq!(v.lookup(&key("local", "/x")), Some(by_size(false)));
    }

    #[test]
    fn survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        FolderViews::open(dir.path()).remember(&key("local", "/a"), icons());
        assert_eq!(FolderViews::open(dir.path()).lookup(&key("local", "/a")), Some(icons()));
        // A corrupt file starts empty instead of failing.
        std::fs::write(dir.path().join("folder_views.json"), "{not json").unwrap();
        assert_eq!(FolderViews::open(dir.path()).lookup(&key("local", "/a")), None);
    }

    #[test]
    fn hidden_files_and_filter_are_not_per_folder() {
        let base = ViewSpec { show_hidden: true, filter: "x".into(), ..Default::default() };
        let applied = icons().apply_to(&base);
        assert!(applied.show_hidden && applied.filter == "x" && applied.layout == Layout::Icons);
    }
}
