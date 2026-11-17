//! FFI for favorites of every kind, favorite groups, the tag list,
//! themes, requests from outside the app and volume rules. Conversions
//! only; the rules live in `rustyfinder_core`.

use rustyfinder_core::outside::{self, OutsideRequest};
use rustyfinder_core::store::{FavoriteDraft, FavoriteKind, FavoritePath, ServerSpec, TagEntry};
use rustyfinder_core::theme::{self, Appearance, Theme};
use rustyfinder_core::volumes::{self, EjectPlan};
use rustyfinder_core::{SavedWindow, LOCAL};

use crate::{FavoriteRecord, RustyFinderApp, RustyFinderError, WindowSession};

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FavoriteKindRecord {
    Path,
    Connection,
    Share,
    Synclet,
    RenamePreset,
    SavedSearch,
    Workspace,
}

impl From<FavoriteKind> for FavoriteKindRecord {
    fn from(k: FavoriteKind) -> Self {
        match k {
            FavoriteKind::Path => Self::Path,
            FavoriteKind::Connection => Self::Connection,
            FavoriteKind::Share => Self::Share,
            FavoriteKind::Synclet => Self::Synclet,
            FavoriteKind::RenamePreset => Self::RenamePreset,
            FavoriteKind::SavedSearch => Self::SavedSearch,
            FavoriteKind::Workspace => Self::Workspace,
        }
    }
}

impl From<FavoriteKindRecord> for FavoriteKind {
    fn from(k: FavoriteKindRecord) -> Self {
        match k {
            FavoriteKindRecord::Path => Self::Path,
            FavoriteKindRecord::Connection => Self::Connection,
            FavoriteKindRecord::Share => Self::Share,
            FavoriteKindRecord::Synclet => Self::Synclet,
            FavoriteKindRecord::RenamePreset => Self::RenamePreset,
            FavoriteKindRecord::SavedSearch => Self::SavedSearch,
            FavoriteKindRecord::Workspace => Self::Workspace,
        }
    }
}

/// A server as a favorite keeps it. No password: see `keychain`.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ServerSpecRecord {
    pub protocol: String,
    pub host: String,
    pub port: Option<u16>,
    pub user: String,
    /// Keychain account of the password, "" for none.
    pub keychain: String,
    /// Options by name ("passive", "encoding", "region"...).
    pub options: std::collections::HashMap<String, String>,
}

impl From<ServerSpec> for ServerSpecRecord {
    fn from(s: ServerSpec) -> Self {
        ServerSpecRecord {
            protocol: s.protocol,
            host: s.host,
            port: s.port,
            user: s.user,
            keychain: s.keychain,
            options: s.options.into_iter().collect(),
        }
    }
}

impl From<ServerSpecRecord> for ServerSpec {
    fn from(s: ServerSpecRecord) -> Self {
        ServerSpec {
            protocol: s.protocol,
            host: s.host,
            port: s.port,
            user: s.user,
            keychain: s.keychain,
            options: s.options.into_iter().collect(),
        }
    }
}

/// "sftp://bob@host:2222": a server's URL without path or password.
#[uniffi::export]
pub fn server_spec_url(spec: ServerSpecRecord) -> String {
    ServerSpec::from(spec).url()
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FavoritePathRecord {
    pub id: u64,
    pub name: String,
    pub path: String,
}

impl From<FavoritePath> for FavoritePathRecord {
    fn from(p: FavoritePath) -> Self {
        FavoritePathRecord { id: p.id, name: p.name, path: p.path }
    }
}

/// What the Add to Favorites sheet and the Favorite Manager edit.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FavoriteDraftRecord {
    pub name: String,
    pub path: String,
    pub icon: String,
    pub kind: FavoriteKindRecord,
    pub server: Option<ServerSpecRecord>,
    pub payload: String,
}

impl From<FavoriteDraftRecord> for FavoriteDraft {
    fn from(d: FavoriteDraftRecord) -> Self {
        FavoriteDraft {
            name: d.name,
            path: d.path,
            icon: d.icon,
            kind: d.kind.into(),
            server: d.server.map(Into::into),
            payload: d.payload,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct TagEntryRecord {
    pub name: String,
    /// Finder's colour number: 0 none, 1 gray, 2 green, 3 purple, 4 blue,
    /// 5 yellow, 6 red, 7 orange.
    pub color: u8,
    pub sidebar: bool,
}

impl From<TagEntry> for TagEntryRecord {
    fn from(t: TagEntry) -> Self {
        TagEntryRecord { name: t.name, color: t.color, sidebar: t.sidebar }
    }
}

impl From<TagEntryRecord> for TagEntry {
    fn from(t: TagEntryRecord) -> Self {
        TagEntry { name: t.name, color: t.color, sidebar: t.sidebar }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ThemeAppearance {
    System,
    Light,
    Dark,
}

/// A theme; colours are "#RRGGBB" or "" for the system colour.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ThemeRecord {
    pub id: String,
    pub name: String,
    pub builtin: bool,
    pub appearance: ThemeAppearance,
    pub base: String,
    pub background: String,
    pub alternate_background: String,
    pub text: String,
    pub secondary_text: String,
    pub selection: String,
    pub inactive_selection: String,
    pub sidebar_background: String,
    pub sidebar_text: String,
    pub sidebar_selection: String,
}

impl From<Theme> for ThemeRecord {
    fn from(t: Theme) -> Self {
        ThemeRecord {
            id: t.id,
            name: t.name,
            builtin: t.builtin,
            appearance: match t.appearance {
                Appearance::System => ThemeAppearance::System,
                Appearance::Light => ThemeAppearance::Light,
                Appearance::Dark => ThemeAppearance::Dark,
            },
            base: t.base,
            background: t.background,
            alternate_background: t.alternate_background,
            text: t.text,
            secondary_text: t.secondary_text,
            selection: t.selection,
            inactive_selection: t.inactive_selection,
            sidebar_background: t.sidebar_background,
            sidebar_text: t.sidebar_text,
            sidebar_selection: t.sidebar_selection,
        }
    }
}

impl From<ThemeRecord> for Theme {
    fn from(t: ThemeRecord) -> Self {
        Theme {
            id: t.id,
            name: t.name,
            builtin: t.builtin,
            appearance: match t.appearance {
                ThemeAppearance::System => Appearance::System,
                ThemeAppearance::Light => Appearance::Light,
                ThemeAppearance::Dark => Appearance::Dark,
            },
            base: t.base,
            background: t.background,
            alternate_background: t.alternate_background,
            text: t.text,
            secondary_text: t.secondary_text,
            selection: t.selection,
            inactive_selection: t.inactive_selection,
            sidebar_background: t.sidebar_background,
            sidebar_text: t.sidebar_text,
            sidebar_selection: t.sidebar_selection,
        }
    }
}

/// "#abc" or "aabbcc" as "#AABBCC"; "" stays ""; `None` if not a colour.
#[uniffi::export]
pub fn normalize_theme_color(text: String) -> Option<String> {
    theme::normalize_color(&text)
}

/// What another app asked RustyFinder to show.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum OutsideRequestRecord {
    Open { path: String },
    Reveal { path: String },
    Favorite { id: u64 },
    Workspace { id: u64 },
}

impl From<OutsideRequest> for OutsideRequestRecord {
    fn from(r: OutsideRequest) -> Self {
        match r {
            OutsideRequest::Open(path) => Self::Open { path },
            OutsideRequest::Reveal(path) => Self::Reveal { path },
            OutsideRequest::Favorite(id) => Self::Favorite { id },
            OutsideRequest::Workspace(id) => Self::Workspace { id },
        }
    }
}

/// Read a `rustyfinder://` link.
#[uniffi::export]
pub fn parse_app_url(url: String) -> Option<OutsideRequestRecord> {
    outside::parse_app_url(&url).map(Into::into)
}

/// The link that opens a favorite.
#[uniffi::export]
pub fn favorite_url(id: u64) -> String {
    outside::favorite_url(id)
}

/// What the "Reveal in RustyFinder" service was given (file URLs, paths,
/// links), as requests. Folders open; other items are revealed.
#[uniffi::export]
pub fn service_requests(items: Vec<String>, home: String) -> Vec<OutsideRequestRecord> {
    outside::service_requests(&items, &home, |p| std::path::Path::new(p).is_dir()).into_iter().map(Into::into).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EjectPlanRecord {
    Ask,
    One,
    All,
}

/// PT-13: what Eject does for a volume on a disk with `volumes_on_disk`
/// mounted volumes, given the modifier keys held.
#[uniffi::export]
pub fn eject_plan(volumes_on_disk: u32, option: bool, control: bool) -> EjectPlanRecord {
    match volumes::eject_plan(volumes_on_disk as usize, option, control) {
        EjectPlan::Ask => EjectPlanRecord::Ask,
        EjectPlan::One => EjectPlanRecord::One,
        EjectPlan::All => EjectPlanRecord::All,
    }
}

/// The disk shares in `smbutil view` output, without `$` admin shares.
#[uniffi::export]
pub fn smb_shares(output: String) -> Vec<String> {
    volumes::smb_shares(&output)
}

/// True for folders macOS guards behind Full Disk Access.
#[uniffi::export]
pub fn is_privacy_protected(path: String, home: String) -> bool {
    volumes::is_privacy_protected(&path, &home)
}

/// A folder only apps with Full Disk Access can list.
#[uniffi::export]
pub fn full_disk_access_probe(home: String) -> String {
    volumes::full_disk_access_probe(&home)
}

#[uniffi::export]
impl WindowSession {
    /// A volume went away or is about to be ejected: its tabs go to the
    /// starting directory. Returns how many moved.
    pub fn leave_volume(&self, root: String) -> u32 {
        self.model.leave_volume(&root) as u32
    }

    /// A tab already showing `path` on `conn`, active pane first.
    pub fn tab_showing(&self, conn: u64, path: String) -> Option<u64> {
        self.model.tab_showing(conn, &path)
    }
}

#[uniffi::export]
impl RustyFinderApp {
    // --- Favorites

    pub fn favorite(&self, id: u64) -> Option<FavoriteRecord> {
        self.store.favorite(id).map(Into::into)
    }

    /// Id of the group a favorite is in.
    pub fn favorite_group_of(&self, id: u64) -> Option<u64> {
        self.store.group_of(id)
    }

    pub fn add_favorite_draft(
        &self,
        group: String,
        draft: FavoriteDraftRecord,
    ) -> Result<FavoriteRecord, RustyFinderError> {
        Ok(self.store.add_favorite_draft(&group, draft.into())?.into())
    }

    /// Change a favorite; with `group`, move it there too.
    pub fn update_favorite(
        &self,
        id: u64,
        draft: FavoriteDraftRecord,
        group: Option<u64>,
    ) -> Result<(), RustyFinderError> {
        Ok(self.store.update_favorite(id, draft.into(), group)?)
    }

    pub fn duplicate_favorite(&self, id: u64) -> Result<FavoriteRecord, RustyFinderError> {
        Ok(self.store.duplicate_favorite(id)?.into())
    }

    pub fn set_favorite_icon(&self, id: u64, icon: String) -> Result<(), RustyFinderError> {
        Ok(self.store.set_favorite_icon(id, &icon)?)
    }

    pub fn add_favorite_group(&self, name: String) -> Result<u64, RustyFinderError> {
        Ok(self.store.add_group(&name)?)
    }

    pub fn rename_favorite_group(&self, id: u64, name: String) -> Result<(), RustyFinderError> {
        Ok(self.store.rename_group(id, &name)?)
    }

    /// Delete a group and what it holds; returns how many favorites went.
    pub fn remove_favorite_group(&self, id: u64) -> Result<u32, RustyFinderError> {
        Ok(self.store.remove_group(id)? as u32)
    }

    pub fn move_favorite_group(&self, id: u64, index: u32) -> Result<(), RustyFinderError> {
        Ok(self.store.move_group(id, index as usize)?)
    }

    /// Save a remote path under a server favorite (Add Path to Favorite).
    pub fn add_favorite_path(
        &self,
        parent: u64,
        name: String,
        path: String,
    ) -> Result<FavoritePathRecord, RustyFinderError> {
        Ok(self.store.add_favorite_path(parent, &name, &path)?.into())
    }

    pub fn remove_favorite_path(&self, parent: u64, child: u64) -> Result<(), RustyFinderError> {
        Ok(self.store.remove_favorite_path(parent, child)?)
    }

    /// The server favorite for a server URL.
    pub fn favorite_for_server(&self, url: String) -> Option<FavoriteRecord> {
        self.store.favorite_for_server(&url).map(Into::into)
    }

    /// Save a window as a workspace listed in `group`.
    pub fn save_workspace_in(&self, group: String, name: String, window_json: String) -> Option<u64> {
        let window = serde_json::from_str::<SavedWindow>(&window_json).ok()?;
        self.store.add_workspace_in(&group, &name, window)
    }

    // --- Servers

    /// Remember a connection's server URL (no password), so its tabs can
    /// reconnect after a relaunch and its Recent Folders are its own.
    pub fn set_connection_url(&self, connection: u64, url: String) {
        self.registry.set_url(connection, &url);
    }

    pub fn connection_url(&self, connection: u64) -> String {
        self.registry.url(connection)
    }

    /// An open connection to this server, if any.
    pub fn connection_for_url(&self, url: String) -> Option<u64> {
        self.registry.find_url(&url)
    }

    /// Recent Folders of a data source: the disk (0) or a server.
    pub fn recent_folders_on(&self, connection: u64) -> Vec<String> {
        if connection == LOCAL {
            return self.store.recent_folders(&*self.vfs);
        }
        let url = self.registry.url(connection);
        if url.is_empty() {
            Vec::new()
        } else {
            self.store.remote_recents(&url)
        }
    }

    // --- Tags

    pub fn tag_list(&self) -> Vec<TagEntryRecord> {
        self.store.tag_list().into_iter().map(Into::into).collect()
    }

    /// Add tags found elsewhere that the list lacks (deleted ones stay out).
    pub fn merge_tags(&self, found: Vec<TagEntryRecord>) -> Vec<TagEntryRecord> {
        let found: Vec<TagEntry> = found.into_iter().map(Into::into).collect();
        self.store.merge_tags(&found).into_iter().map(Into::into).collect()
    }

    /// A tag was assigned: it is listed (again).
    pub fn remember_tag(&self, name: String, color: u8) -> Vec<TagEntryRecord> {
        self.store.remember_tag(&name, color).into_iter().map(Into::into).collect()
    }

    pub fn set_tag_list(&self, tags: Vec<TagEntryRecord>) -> Vec<TagEntryRecord> {
        self.store.set_tag_list(tags.into_iter().map(Into::into).collect()).into_iter().map(Into::into).collect()
    }

    pub fn rename_tag_in_list(&self, old: String, new: String) -> Result<Vec<TagEntryRecord>, RustyFinderError> {
        Ok(self.store.rename_tag(&old, &new)?.into_iter().map(Into::into).collect())
    }

    pub fn delete_tags(&self, names: Vec<String>) -> Vec<TagEntryRecord> {
        self.store.delete_tags(&names).into_iter().map(Into::into).collect()
    }

    // --- Themes

    pub fn themes(&self) -> Vec<ThemeRecord> {
        self.store.themes().into_iter().map(Into::into).collect()
    }

    pub fn current_theme(&self) -> ThemeRecord {
        self.store.current_theme().into()
    }

    pub fn select_theme(&self, id: String) -> bool {
        self.store.select_theme(&id)
    }

    pub fn duplicate_theme(&self, id: String) -> Option<ThemeRecord> {
        self.store.duplicate_theme(&id).map(Into::into)
    }

    pub fn save_theme(&self, theme: ThemeRecord) -> bool {
        self.store.save_theme(theme.into())
    }

    pub fn rename_theme(&self, id: String, name: String) -> Result<(), RustyFinderError> {
        Ok(self.store.rename_theme(&id, &name)?)
    }

    pub fn delete_theme(&self, id: String) -> bool {
        self.store.delete_theme(&id)
    }
}

/// An item's tags with `old` renamed to `new`; `None` if it lacks `old`.
#[uniffi::export]
pub fn tags_rename(tags: Vec<String>, old: String, new: String) -> Option<Vec<String>> {
    rustyfinder_core::tags::rename(&tags, &old, &new)
}

/// An item's tags with `tag` added once.
#[uniffi::export]
pub fn tags_add(tags: Vec<String>, tag: String) -> Vec<String> {
    rustyfinder_core::tags::add(&tags, &tag)
}
