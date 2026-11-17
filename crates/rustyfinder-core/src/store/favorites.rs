//! The richer side of favorites (kinds, groups, favorite paths under a
//! server, duplicating, workspaces as favorites), recent folders on
//! servers, and the user's tag list.

use serde::{Deserialize, Serialize};

use super::{
    read_json, same_name, write_json, Favorite, FavoriteError, FavoriteGroup, FavoriteKind, FavoritePath, SavedWindow,
    ServerSpec, Store, Workspace, MAX_RECENTS,
};

/// The group workspaces land in unless the user picks another.
pub const WORKSPACES_GROUP: &str = "Workspaces";

/// Everything about a favorite except its id: what the Add to Favorites
/// sheet and the Favorite Manager edit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FavoriteDraft {
    pub name: String,
    pub path: String,
    pub icon: String,
    pub kind: FavoriteKind,
    pub server: Option<ServerSpec>,
    pub payload: String,
}

/// One tag of the user's tag list (Settings → Tags).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagEntry {
    pub name: String,
    /// Finder's colour number: 0 none, 1 gray ... 7 orange.
    #[serde(default)]
    pub color: u8,
    /// Listed in the sidebar's Tags group.
    #[serde(default = "yes")]
    pub sidebar: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct TagsFile {
    tags: Vec<TagEntry>,
    /// Tags the user deleted, so Finder's list does not bring them back.
    removed: Vec<String>,
}

impl Favorite {
    pub fn new(id: u64, name: &str, path: &str) -> Self {
        Favorite {
            id,
            name: name.to_string(),
            path: path.to_string(),
            icon: String::new(),
            kind: FavoriteKind::Path,
            server: None,
            payload: String::new(),
            children: Vec::new(),
        }
    }

    fn from_draft(id: u64, draft: FavoriteDraft) -> Self {
        Favorite {
            id,
            name: draft.name.trim().to_string(),
            path: draft.path,
            icon: draft.icon,
            kind: draft.kind,
            server: draft.server,
            payload: draft.payload,
            children: Vec::new(),
        }
    }
}

/// `name`, or `name 2`, `name 3`... whichever is free among `taken`.
fn unique_name<'a>(name: &str, taken: impl Iterator<Item = &'a str> + Clone) -> String {
    let free = |candidate: &str| !taken.clone().any(|t| same_name(t, candidate));
    if free(name) {
        return name.to_string();
    }
    (2..).map(|n| format!("{name} {n}")).find(|c| free(c)).unwrap()
}

impl Store {
    /// The favorite with this id, wherever it is.
    pub fn favorite(&self, id: u64) -> Option<Favorite> {
        self.favorites.lock().unwrap().iter().flat_map(|g| g.items.iter()).find(|f| f.id == id).cloned()
    }

    /// The group holding a favorite.
    pub fn group_of(&self, id: u64) -> Option<u64> {
        self.favorites.lock().unwrap().iter().find(|g| g.items.iter().any(|f| f.id == id)).map(|g| g.id)
    }

    /// Add a favorite of any kind to the group named `group` (created if
    /// missing). Names are unique within a group, ignoring case.
    pub fn add_favorite_draft(&self, group: &str, draft: FavoriteDraft) -> Result<Favorite, FavoriteError> {
        if draft.name.trim().is_empty() {
            return Err(FavoriteError::EmptyName);
        }
        let group = if group.trim().is_empty() { "Favorites" } else { group.trim() };
        let mut groups = self.favorites.lock().unwrap();
        let idx = match groups.iter().position(|g| same_name(&g.name, group)) {
            Some(i) => i,
            None => {
                groups.push(FavoriteGroup {
                    id: self.new_id(),
                    name: group.to_string(),
                    collapsed: false,
                    items: vec![],
                });
                groups.len() - 1
            }
        };
        if groups[idx].items.iter().any(|f| same_name(&f.name, &draft.name)) {
            return Err(FavoriteError::NameTaken(draft.name.trim().to_string()));
        }
        let favorite = Favorite::from_draft(self.new_id(), draft);
        groups[idx].items.push(favorite.clone());
        self.save_favorites(&groups);
        Ok(favorite)
    }

    /// Change what a favorite is (name, icon, location, server...) and,
    /// with `group`, move it to the end of that group. Its id, kind and
    /// favorite paths stay.
    pub fn update_favorite(&self, id: u64, draft: FavoriteDraft, group: Option<u64>) -> Result<(), FavoriteError> {
        let name = draft.name.trim().to_string();
        if name.is_empty() {
            return Err(FavoriteError::EmptyName);
        }
        let mut groups = self.favorites.lock().unwrap();
        let from = groups.iter().position(|g| g.items.iter().any(|f| f.id == id)).ok_or(FavoriteError::Missing)?;
        let to = match group {
            Some(gid) => groups.iter().position(|g| g.id == gid).ok_or(FavoriteError::Missing)?,
            None => from,
        };
        if groups[to].items.iter().any(|f| f.id != id && same_name(&f.name, &name)) {
            return Err(FavoriteError::NameTaken(name));
        }
        let pos = groups[from].items.iter().position(|f| f.id == id).unwrap();
        let mut favorite = groups[from].items.remove(pos);
        favorite.name = name;
        favorite.icon = draft.icon;
        favorite.path = draft.path;
        favorite.server = draft.server;
        favorite.payload = draft.payload;
        if to == from {
            groups[from].items.insert(pos, favorite);
        } else {
            groups[to].items.push(favorite);
        }
        self.save_favorites(&groups);
        Ok(())
    }

    /// A copy of a favorite, right after it, named "<name> copy" (made
    /// unique). It opens the same thing; a workspace is copied too.
    pub fn duplicate_favorite(&self, id: u64) -> Result<Favorite, FavoriteError> {
        let original = self.favorite(id).ok_or(FavoriteError::Missing)?;
        let mut payload = original.payload.clone();
        if original.kind == FavoriteKind::Workspace {
            let source = original.payload.parse::<u64>().ok();
            let workspace =
                self.workspaces().into_iter().find(|w| Some(w.id) == source).ok_or(FavoriteError::Missing)?;
            let all = self.workspaces();
            let name = unique_name(&format!("{} copy", workspace.name), all.iter().map(|w| w.name.as_str()));
            payload = self.store_workspace(&name, workspace.window).to_string();
        }
        let mut groups = self.favorites.lock().unwrap();
        let g = groups.iter_mut().find(|g| g.items.iter().any(|f| f.id == id)).ok_or(FavoriteError::Missing)?;
        let pos = g.items.iter().position(|f| f.id == id).unwrap();
        let name = unique_name(&format!("{} copy", original.name), g.items.iter().map(|f| f.name.as_str()));
        let mut copy = Favorite { id: self.new_id(), name, payload, ..original };
        for child in &mut copy.children {
            child.id = self.new_id();
        }
        g.items.insert(pos + 1, copy.clone());
        self.save_favorites(&groups);
        Ok(copy)
    }

    pub fn set_favorite_icon(&self, id: u64, icon: &str) -> Result<(), FavoriteError> {
        let mut groups = self.favorites.lock().unwrap();
        let f =
            groups.iter_mut().flat_map(|g| g.items.iter_mut()).find(|f| f.id == id).ok_or(FavoriteError::Missing)?;
        f.icon = icon.to_string();
        self.save_favorites(&groups);
        Ok(())
    }

    // --- Groups

    /// A new, empty group at the end. Group names are unique.
    pub fn add_group(&self, name: &str) -> Result<u64, FavoriteError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(FavoriteError::EmptyName);
        }
        let mut groups = self.favorites.lock().unwrap();
        if groups.iter().any(|g| same_name(&g.name, name)) {
            return Err(FavoriteError::NameTaken(name.to_string()));
        }
        let id = self.new_id();
        groups.push(FavoriteGroup { id, name: name.to_string(), collapsed: false, items: vec![] });
        self.save_favorites(&groups);
        Ok(id)
    }

    pub fn rename_group(&self, id: u64, name: &str) -> Result<(), FavoriteError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(FavoriteError::EmptyName);
        }
        let mut groups = self.favorites.lock().unwrap();
        if groups.iter().any(|g| g.id != id && same_name(&g.name, name)) {
            return Err(FavoriteError::NameTaken(name.to_string()));
        }
        let g = groups.iter_mut().find(|g| g.id == id).ok_or(FavoriteError::Missing)?;
        g.name = name.to_string();
        self.save_favorites(&groups);
        Ok(())
    }

    /// Delete a group with everything in it (workspaces included).
    /// Returns how many favorites went with it.
    pub fn remove_group(&self, id: u64) -> Result<usize, FavoriteError> {
        let mut groups = self.favorites.lock().unwrap();
        let pos = groups.iter().position(|g| g.id == id).ok_or(FavoriteError::Missing)?;
        let removed = groups.remove(pos);
        self.save_favorites(&groups);
        drop(groups);
        for f in &removed.items {
            if f.kind == FavoriteKind::Workspace {
                if let Ok(w) = f.payload.parse() {
                    self.remove_workspace(w);
                }
            }
        }
        Ok(removed.items.len())
    }

    /// Move a group to `index` among the groups (clamped).
    pub fn move_group(&self, id: u64, index: usize) -> Result<(), FavoriteError> {
        let mut groups = self.favorites.lock().unwrap();
        let pos = groups.iter().position(|g| g.id == id).ok_or(FavoriteError::Missing)?;
        let g = groups.remove(pos);
        let index = index.min(groups.len());
        groups.insert(index, g);
        self.save_favorites(&groups);
        Ok(())
    }

    // --- Favorite paths under a server favorite

    /// Save `path` (exactly as given) under the server favorite `parent`,
    /// named after its last component unless a name is given.
    pub fn add_favorite_path(&self, parent: u64, name: &str, path: &str) -> Result<FavoritePath, FavoriteError> {
        let name = if name.trim().is_empty() { crate::paths::file_name(path) } else { name.trim() };
        let name = if name.is_empty() { "/" } else { name };
        let mut groups = self.favorites.lock().unwrap();
        let f = groups
            .iter_mut()
            .flat_map(|g| g.items.iter_mut())
            .find(|f| f.id == parent)
            .ok_or(FavoriteError::Missing)?;
        if f.children.iter().any(|c| same_name(&c.name, name)) {
            return Err(FavoriteError::NameTaken(name.to_string()));
        }
        let child = FavoritePath { id: self.new_id(), name: name.to_string(), path: path.to_string() };
        f.children.push(child.clone());
        self.save_favorites(&groups);
        Ok(child)
    }

    pub fn remove_favorite_path(&self, parent: u64, child: u64) -> Result<(), FavoriteError> {
        let mut groups = self.favorites.lock().unwrap();
        let f = groups
            .iter_mut()
            .flat_map(|g| g.items.iter_mut())
            .find(|f| f.id == parent)
            .ok_or(FavoriteError::Missing)?;
        let before = f.children.len();
        f.children.retain(|c| c.id != child);
        if f.children.len() == before {
            return Err(FavoriteError::Missing);
        }
        self.save_favorites(&groups);
        Ok(())
    }

    /// The server favorite for a server URL, so "Add Path to Favorite"
    /// on a connected pane knows where the path goes.
    pub fn favorite_for_server(&self, url: &str) -> Option<Favorite> {
        self.favorites
            .lock()
            .unwrap()
            .iter()
            .flat_map(|g| g.items.iter())
            .find(|f| f.server.as_ref().is_some_and(|s| s.url() == url))
            .cloned()
    }

    // --- Workspaces as favorites

    /// Save a window as a workspace, listed as a favorite in `group`.
    /// `None` if the name is empty or another workspace has it.
    pub fn add_workspace_in(&self, group: &str, name: &str, window: SavedWindow) -> Option<u64> {
        let name = name.trim();
        if name.is_empty() || self.workspaces().iter().any(|w| same_name(&w.name, name)) {
            return None;
        }
        let id = self.store_workspace(name, window);
        self.list_workspace(group, id, name);
        Some(id)
    }

    fn store_workspace(&self, name: &str, window: SavedWindow) -> u64 {
        let mut all = self.workspaces();
        let id = all.iter().map(|w| w.id).max().unwrap_or(0) + 1;
        all.push(Workspace { id, name: name.to_string(), window });
        write_json(&self.dir.join("workspaces.json"), &all);
        id
    }

    fn list_workspace(&self, group: &str, id: u64, name: &str) {
        let taken: Vec<String> = self
            .favorite_groups()
            .iter()
            .filter(|g| same_name(&g.name, group))
            .flat_map(|g| g.items.iter().map(|f| f.name.clone()))
            .collect();
        let draft = FavoriteDraft {
            name: unique_name(name, taken.iter().map(String::as_str)),
            icon: "rectangle.split.2x1".into(),
            kind: FavoriteKind::Workspace,
            payload: id.to_string(),
            ..FavoriteDraft::default()
        };
        let _ = self.add_favorite_draft(group, draft);
    }

    /// Workspaces saved before they were favorites get one, once.
    pub(super) fn adopt_workspaces(&self) {
        let listed: Vec<u64> = self
            .favorite_groups()
            .iter()
            .flat_map(|g| g.items.iter())
            .filter(|f| f.kind == FavoriteKind::Workspace)
            .filter_map(|f| f.payload.parse().ok())
            .collect();
        for w in self.workspaces() {
            if !listed.contains(&w.id) {
                self.list_workspace(WORKSPACES_GROUP, w.id, &w.name);
            }
        }
    }

    /// Drop the favorites standing for a workspace that is gone.
    pub(super) fn unlist_workspace(&self, id: u64) {
        let mut groups = self.favorites.lock().unwrap();
        let wanted = id.to_string();
        let mut changed = false;
        for g in groups.iter_mut() {
            let before = g.items.len();
            g.items.retain(|f| !(f.kind == FavoriteKind::Workspace && f.payload == wanted));
            changed |= g.items.len() != before;
        }
        if changed {
            self.save_favorites(&groups);
        }
    }

    // --- Recent folders on servers

    /// Note a visit to `path` on the server `url`.
    pub fn record_remote_recent(&self, url: &str, path: &str) {
        let mut all = self.remote_recents.lock().unwrap();
        let list = all.entry(url.to_string()).or_default();
        if list.first().map(String::as_str) == Some(path) {
            return;
        }
        list.retain(|p| p != path);
        list.insert(0, path.to_string());
        list.truncate(MAX_RECENTS);
        write_json(&self.dir.join("remote-recents.json"), &*all);
    }

    /// Recent folders on the server `url`, newest first.
    pub fn remote_recents(&self, url: &str) -> Vec<String> {
        self.remote_recents.lock().unwrap().get(url).cloned().unwrap_or_default()
    }

    /// Forget a remote folder that turned out to be gone.
    pub fn forget_remote_recent(&self, url: &str, path: &str) {
        let mut all = self.remote_recents.lock().unwrap();
        if let Some(list) = all.get_mut(url) {
            list.retain(|p| p != path);
            write_json(&self.dir.join("remote-recents.json"), &*all);
        }
    }

    // --- Tag list

    fn tags_file(&self) -> TagsFile {
        read_json(&self.dir.join("tags.json")).unwrap_or_default()
    }

    /// The user's tags in their chosen order.
    pub fn tag_list(&self) -> Vec<TagEntry> {
        self.tags_file().tags
    }

    /// Add tags found elsewhere (the colour tags, Finder's list, tags met
    /// on items) that the list lacks and the user has not deleted.
    /// Returns the list.
    pub fn merge_tags(&self, found: &[TagEntry]) -> Vec<TagEntry> {
        let mut file = self.tags_file();
        let before = file.tags.len();
        for tag in found {
            let name = tag.name.trim();
            if name.is_empty()
                || file.tags.iter().any(|t| same_name(&t.name, name))
                || file.removed.iter().any(|r| same_name(r, name))
            {
                continue;
            }
            file.tags.push(TagEntry { name: name.to_string(), ..tag.clone() });
        }
        if file.tags.len() != before || !self.dir.join("tags.json").exists() {
            write_json(&self.dir.join("tags.json"), &file);
        }
        file.tags
    }

    /// A tag the user just assigned: listed again even if it was deleted.
    pub fn remember_tag(&self, name: &str, color: u8) -> Vec<TagEntry> {
        let mut file = self.tags_file();
        file.removed.retain(|r| !same_name(r, name));
        write_json(&self.dir.join("tags.json"), &file);
        self.merge_tags(&[TagEntry { name: name.to_string(), color, sidebar: true }])
    }

    /// Store the list as edited (order, colours, sidebar switches).
    /// Duplicate and empty names are dropped.
    pub fn set_tag_list(&self, tags: Vec<TagEntry>) -> Vec<TagEntry> {
        let mut file = self.tags_file();
        let mut kept: Vec<TagEntry> = Vec::new();
        for tag in tags {
            if !tag.name.trim().is_empty() && !kept.iter().any(|t| same_name(&t.name, &tag.name)) {
                kept.push(tag);
            }
        }
        file.tags = kept;
        write_json(&self.dir.join("tags.json"), &file);
        file.tags
    }

    /// Rename a tag in the list; if `new` is listed already the two merge.
    pub fn rename_tag(&self, old: &str, new: &str) -> Result<Vec<TagEntry>, FavoriteError> {
        let new = new.trim();
        if new.is_empty() {
            return Err(FavoriteError::EmptyName);
        }
        let mut file = self.tags_file();
        let pos = file.tags.iter().position(|t| same_name(&t.name, old)).ok_or(FavoriteError::Missing)?;
        if let Some(other) = file.tags.iter().position(|t| same_name(&t.name, new)) {
            if other != pos {
                file.tags.remove(pos);
                write_json(&self.dir.join("tags.json"), &file);
                return Ok(file.tags);
            }
        }
        file.tags[pos].name = new.to_string();
        file.removed.retain(|r| !same_name(r, new));
        write_json(&self.dir.join("tags.json"), &file);
        Ok(file.tags)
    }

    /// Take tags off the list for good (until assigned again).
    pub fn delete_tags(&self, names: &[String]) -> Vec<TagEntry> {
        let mut file = self.tags_file();
        file.tags.retain(|t| !names.iter().any(|n| same_name(n, &t.name)));
        for name in names {
            if !file.removed.iter().any(|r| same_name(r, name)) {
                file.removed.push(name.clone());
            }
        }
        write_json(&self.dir.join("tags.json"), &file);
        file.tags
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn store() -> (tempfile::TempDir, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let store = Store::open(tmp.path().join("support"), home.to_str().unwrap());
        (tmp, store)
    }

    /// FT-08: renaming an FTP-with-TLS favorite in the Favorite Manager
    /// (which edits a copy of the whole favorite) keeps passive mode and the
    /// other connection options, also across a relaunch.
    #[test]
    fn spec_ft08_renaming_a_server_favorite_keeps_its_options() {
        let (tmp, store) = store();
        let mut options = std::collections::BTreeMap::new();
        options.insert("passive".to_string(), "true".to_string());
        options.insert("implicit".to_string(), "false".to_string());
        options.insert("encoding".to_string(), "windows-1252".to_string());
        let server = ServerSpec {
            protocol: "ftps".into(),
            host: "files.example.org".into(),
            port: Some(2121),
            user: "bob".into(),
            keychain: "ftps://bob@files.example.org:2121".into(),
            options,
        };
        let draft = FavoriteDraft {
            name: "Work FTP".into(),
            path: "/pub".into(),
            icon: String::new(),
            kind: FavoriteKind::Connection,
            server: Some(server.clone()),
            payload: String::new(),
        };
        let added = store.add_favorite_draft("Servers", draft).unwrap();
        // The manager starts from the favorite as stored and changes the name.
        let stored = store.favorite_groups().into_iter().flat_map(|g| g.items).find(|f| f.id == added.id).unwrap();
        let edit = FavoriteDraft {
            name: "Office FTP".into(),
            path: stored.path.clone(),
            icon: stored.icon.clone(),
            kind: stored.kind,
            server: stored.server.clone(),
            payload: stored.payload.clone(),
        };
        store.update_favorite(added.id, edit, None).unwrap();
        let reopened = Store::open(tmp.path().join("support"), tmp.path().join("home").to_str().unwrap());
        let after = reopened.favorite_groups().into_iter().flat_map(|g| g.items).find(|f| f.id == added.id).unwrap();
        assert_eq!(after.name, "Office FTP");
        assert_eq!(after.server, Some(server));
        assert_eq!(after.path, "/pub");
    }

    fn server() -> ServerSpec {
        ServerSpec {
            protocol: "sftp".into(),
            host: "example.org".into(),
            port: Some(2222),
            user: "bob".into(),
            keychain: "sftp://bob@example.org:2222".into(),
            ..ServerSpec::default()
        }
    }

    #[test]
    fn every_kind_of_favorite_is_kept_without_passwords() {
        let (tmp, store) = store();
        let kinds = [
            (FavoriteKind::Connection, "/var/www", ""),
            (FavoriteKind::Share, "/Media", ""),
            (FavoriteKind::Synclet, "", r#"{"left":"/a","right":"/b"}"#),
            (FavoriteKind::RenamePreset, "", r#"[{"kind":"case"}]"#),
            (FavoriteKind::SavedSearch, "/a", r#"{"term":"x"}"#),
        ];
        for (i, (kind, path, payload)) in kinds.iter().enumerate() {
            let server = matches!(kind, FavoriteKind::Connection | FavoriteKind::Share).then(server);
            let draft = FavoriteDraft {
                name: format!("f{i}"),
                path: path.to_string(),
                kind: *kind,
                server,
                payload: payload.to_string(),
                icon: "server.rack".into(),
            };
            store.add_favorite_draft("Kinds", draft).unwrap();
        }
        let again = Store::open(tmp.path().join("support"), "/h");
        let group = again.favorite_groups().into_iter().find(|g| g.name == "Kinds").unwrap();
        let got: Vec<_> = group.items.iter().map(|f| (f.kind, f.path.as_str(), f.payload.as_str())).collect();
        let want: Vec<_> = kinds.iter().map(|(k, p, j)| (*k, *p, *j)).collect();
        assert_eq!(got, want);
        assert_eq!(group.items[0].server.as_ref().unwrap().url(), "sftp://bob@example.org:2222");
        let json = fs::read_to_string(tmp.path().join("support/favorites.json")).unwrap();
        assert!(!json.contains("password"));
    }

    #[test]
    fn old_favorites_files_still_load_as_path_favorites() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("support");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("favorites.json"),
            r#"{"groups":[{"id":1,"name":"G","items":[{"id":2,"name":"a","path":"/a"}]}]}"#,
        )
        .unwrap();
        let store = Store::open(&dir, "/h");
        let f = store.favorite(2).unwrap();
        assert_eq!((f.kind, f.icon.as_str(), f.children.len()), (FavoriteKind::Path, "", 0));
        // New ids do not collide with old ones.
        assert!(store.add_favorite("G", "b", "/b").unwrap().id > 2);
    }

    #[test]
    fn fv03_a_duplicate_sits_after_the_original_and_opens_the_same_place() {
        let (_tmp, store) = store();
        let a = store.add_favorite("G", "Site", "/srv").unwrap();
        let b = store.add_favorite("G", "Other", "/o").unwrap();
        let copy = store.duplicate_favorite(a.id).unwrap();
        assert_eq!((copy.name.as_str(), copy.path.as_str()), ("Site copy", "/srv"));
        assert_ne!(copy.id, a.id);
        let again = store.duplicate_favorite(a.id).unwrap();
        assert_eq!(again.name, "Site copy 2");
        let ids: Vec<u64> =
            store.favorite_groups().iter().find(|g| g.name == "G").unwrap().items.iter().map(|f| f.id).collect();
        assert_eq!(ids, vec![a.id, again.id, copy.id, b.id]);
        assert_eq!(store.duplicate_favorite(999), Err(FavoriteError::Missing));
    }

    #[test]
    fn editing_changes_name_icon_location_and_group() {
        let (_tmp, store) = store();
        let a = store.add_favorite("G", "Site", "/srv").unwrap();
        store.add_favorite("H", "Taken", "/t").unwrap();
        let h = store.favorite_groups().iter().find(|g| g.name == "H").unwrap().id;
        let draft = |name: &str| FavoriteDraft {
            name: name.into(),
            path: "/new".into(),
            icon: "film".into(),
            ..Default::default()
        };
        assert_eq!(store.update_favorite(a.id, draft("taken"), Some(h)), Err(FavoriteError::NameTaken("taken".into())));
        store.update_favorite(a.id, draft("Movies"), Some(h)).unwrap();
        let f = store.favorite(a.id).unwrap();
        assert_eq!((f.name.as_str(), f.path.as_str(), f.icon.as_str()), ("Movies", "/new", "film"));
        assert_eq!(store.group_of(a.id), Some(h));
        assert_eq!(store.update_favorite(a.id, draft(" "), None), Err(FavoriteError::EmptyName));
    }

    #[test]
    fn fv04_deleting_a_group_takes_its_favorites_and_reports_how_many() {
        let (_tmp, store) = store();
        for n in ["a", "b", "c"] {
            store.add_favorite("Doomed", n, "/x").unwrap();
        }
        let id = store.favorite_groups().iter().find(|g| g.name == "Doomed").unwrap().id;
        assert_eq!(store.remove_group(id), Ok(3));
        assert!(!store.favorite_groups().iter().any(|g| g.name == "Doomed"));
        assert_eq!(store.remove_group(id), Err(FavoriteError::Missing));
    }

    #[test]
    fn groups_are_added_renamed_and_moved_with_unique_names() {
        let (_tmp, store) = store();
        let a = store.add_group("Work").unwrap();
        assert_eq!(store.add_group("work"), Err(FavoriteError::NameTaken("work".into())));
        let b = store.add_group("Play").unwrap();
        assert!(matches!(store.rename_group(b, "WORK"), Err(FavoriteError::NameTaken(_))));
        store.rename_group(b, "Fun").unwrap();
        store.move_group(b, 0).unwrap();
        let names: Vec<String> = store.favorite_groups().into_iter().map(|g| g.name).collect();
        assert_eq!(names, ["Fun", "Favorites", "Work"]);
        let _ = a;
    }

    #[test]
    fn fv02_paths_under_a_server_favorite_open_exactly_that_path() {
        let (_tmp, store) = store();
        let draft = FavoriteDraft {
            name: "Web".into(),
            path: "/var/www".into(),
            kind: FavoriteKind::Connection,
            server: Some(server()),
            ..Default::default()
        };
        let f = store.add_favorite_draft("Servers", draft).unwrap();
        let child = store.add_favorite_path(f.id, "", "/var/www/site/logs").unwrap();
        assert_eq!((child.name.as_str(), child.path.as_str()), ("logs", "/var/www/site/logs"));
        assert!(store.add_favorite_path(f.id, "Logs", "/elsewhere").is_err(), "names are unique");
        assert_eq!(store.favorite_for_server("sftp://bob@example.org:2222").unwrap().id, f.id);
        let copy = store.duplicate_favorite(f.id).unwrap();
        assert_ne!(copy.children[0].id, child.id, "the copy's paths are its own");
        store.remove_favorite_path(f.id, child.id).unwrap();
        assert!(store.favorite(f.id).unwrap().children.is_empty());
    }

    #[test]
    fn workspaces_are_favorites_and_go_together() {
        let (tmp, store) = store();
        let id = store.add_workspace_in("Layouts", "Day", SavedWindow::default()).unwrap();
        let fav = store.favorite_groups().into_iter().find(|g| g.name == "Layouts").unwrap().items[0].clone();
        assert_eq!((fav.kind, fav.payload.clone()), (FavoriteKind::Workspace, id.to_string()));
        // Duplicating copies the workspace as well.
        let copy = store.duplicate_favorite(fav.id).unwrap();
        assert_ne!(copy.payload, fav.payload);
        assert_eq!(store.workspaces().len(), 2);
        // Removing the favorite removes the workspace.
        store.remove_favorite(fav.id).unwrap();
        assert_eq!(store.workspaces().iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["Day copy"]);
        // Removing the workspace removes its favorite.
        let other: u64 = copy.payload.parse().unwrap();
        assert!(store.remove_workspace(other));
        assert!(store.favorite(copy.id).is_none());
        // Workspaces from before favorites existed are listed on launch.
        let dir = tmp.path().join("support");
        fs::write(dir.join("workspaces.json"), r#"[{"id":7,"name":"Old","window":{}}]"#).unwrap();
        let again = Store::open(&dir, "/h");
        let listed = again.favorite_groups().into_iter().find(|g| g.name == WORKSPACES_GROUP).unwrap();
        assert_eq!(listed.items[0].payload, "7");
        assert_eq!(Store::open(&dir, "/h").favorite_groups().iter().filter(|g| g.name == WORKSPACES_GROUP).count(), 1);
    }

    #[test]
    fn recent_folders_are_kept_per_server() {
        let (tmp, store) = store();
        store.record_remote_recent("sftp://a", "/x");
        store.record_remote_recent("sftp://a", "/y");
        store.record_remote_recent("sftp://b", "/z");
        for i in 0..15 {
            store.record_remote_recent("sftp://c", &format!("/d{i}"));
        }
        let again = Store::open(tmp.path().join("support"), "/h");
        assert_eq!(again.remote_recents("sftp://a"), ["/y", "/x"]);
        assert_eq!(again.remote_recents("sftp://b"), ["/z"]);
        assert_eq!(again.remote_recents("sftp://c").len(), MAX_RECENTS);
        assert!(again.remote_recents("sftp://none").is_empty());
        again.forget_remote_recent("sftp://a", "/y");
        assert_eq!(again.remote_recents("sftp://a"), ["/x"]);
    }

    #[test]
    fn tg05_the_tag_list_merges_renames_reorders_and_deletes() {
        let (tmp, store) = store();
        let tag = |name: &str, color: u8| TagEntry { name: name.into(), color, sidebar: true };
        let list = store.merge_tags(&[tag("Red", 6), tag("Work", 0), tag("red", 0)]);
        assert_eq!(list.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Red", "Work"]);
        // Rename; to an existing name it merges.
        store.rename_tag("work", "Old").unwrap();
        assert_eq!(store.rename_tag("Old", "red").unwrap().len(), 1);
        assert_eq!(store.rename_tag("missing", "x"), Err(FavoriteError::Missing));
        store.merge_tags(&[tag("Archive", 0), tag("Blue", 4)]);
        // Reorder and switch one off in the sidebar.
        let mut edited = store.tag_list();
        edited.reverse();
        edited[0].sidebar = false;
        store.set_tag_list(edited);
        assert_eq!(store.tag_list()[0].name, "Blue");
        assert!(!store.tag_list()[0].sidebar);
        // Deleted tags stay away when found again, until assigned.
        store.delete_tags(&["Blue".into()]);
        assert!(!store.merge_tags(&[tag("Blue", 4)]).iter().any(|t| t.name == "Blue"));
        assert!(store.remember_tag("Blue", 4).iter().any(|t| t.name == "Blue"));
        let again = Store::open(tmp.path().join("support"), "/h");
        assert_eq!(again.tag_list().len(), store.tag_list().len());
    }
}
