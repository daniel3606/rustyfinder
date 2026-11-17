//! Saved server connections: what the Connect panel collected, so a
//! connection can be opened again later ("Add to Favorites" in the panel).
//! Passwords are never kept here; they live in the macOS keychain.
//!
//! This is the store only. Favorites that point at a saved connection
//! refer to it by `id`.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedConnection {
    /// 0 for one not saved yet.
    pub id: u64,
    /// Shown in favorites; the host when left empty.
    pub name: String,
    /// URL scheme of the protocol: "sftp", "ftp", "ftps", "http", "https",
    /// "s3", "smb", "afp", "nfs", "vnc".
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    pub user: String,
    /// Folder (or bucket) to open on the server.
    pub remote_path: String,
    /// A folder on this Mac paired with the remote one.
    pub local_path: String,
    /// Tint for the tab and favorite, `#RRGGBB`; empty for none.
    pub color: String,
    /// Name encoding ("windows-1251"); empty is automatic / UTF-8.
    pub encoding: String,
    pub passive: bool,
    pub implicit_tls: bool,
    /// S3 options.
    pub region: String,
    pub path_style: Option<bool>,
    pub storage_class: String,
}

impl SavedConnection {
    /// The name to show: the given one, else `user@host`, else the host.
    pub fn display_name(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.trim().to_string()
        } else if self.user.is_empty() {
            self.host.clone()
        } else {
            format!("{}@{}", self.user, self.host)
        }
    }
}

pub struct SavedConnections {
    file: PathBuf,
    list: Mutex<Vec<SavedConnection>>,
}

impl SavedConnections {
    pub fn open(file: impl Into<PathBuf>) -> Self {
        let file = file.into();
        let list = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Self { file, list: Mutex::new(list) }
    }

    pub fn all(&self) -> Vec<SavedConnection> {
        self.list.lock().unwrap().clone()
    }

    pub fn get(&self, id: u64) -> Option<SavedConnection> {
        self.list.lock().unwrap().iter().find(|c| c.id == id).cloned()
    }

    /// Save a new connection (id 0) or replace the one with its id. Every
    /// field is kept as given: editing only the name never changes, say,
    /// passive mode (FT-08). Returns it as stored.
    pub fn save(&self, mut connection: SavedConnection) -> SavedConnection {
        let mut list = self.list.lock().unwrap();
        match list.iter_mut().find(|c| c.id == connection.id && connection.id != 0) {
            Some(existing) => *existing = connection.clone(),
            None => {
                connection.id = list.iter().map(|c| c.id).max().unwrap_or(0) + 1;
                list.push(connection.clone());
            }
        }
        self.write(&list);
        connection
    }

    pub fn remove(&self, id: u64) -> bool {
        let mut list = self.list.lock().unwrap();
        let before = list.len();
        list.retain(|c| c.id != id);
        let removed = list.len() != before;
        if removed {
            self.write(&list);
        }
        removed
    }

    fn write(&self, list: &[SavedConnection]) {
        let Ok(bytes) = serde_json::to_vec_pretty(list) else { return };
        let tmp = self.file.with_extension("json.tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, &self.file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connections_are_saved_edited_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("connections.json");
        let store = SavedConnections::open(&file);
        let ftp = store.save(SavedConnection {
            scheme: "ftps".into(),
            host: "files.example.org".into(),
            user: "bob".into(),
            passive: true,
            color: "#FF8800".into(),
            local_path: "/Users/bob/Sites".into(),
            ..Default::default()
        });
        assert_eq!(ftp.id, 1);
        assert_eq!(ftp.display_name(), "bob@files.example.org");
        let s3 = store.save(SavedConnection { scheme: "s3".into(), name: "Backups".into(), ..Default::default() });
        assert_eq!(s3.id, 2);

        // FT-08: renaming keeps every other field, passive mode included.
        let renamed = store.save(SavedConnection { name: "Work".into(), ..ftp.clone() });
        assert_eq!(renamed.id, 1);
        let reloaded = SavedConnections::open(&file);
        let back = reloaded.get(1).unwrap();
        assert_eq!((back.name.as_str(), back.passive, back.color.as_str()), ("Work", true, "#FF8800"));
        assert_eq!(reloaded.all().len(), 2);

        assert!(reloaded.remove(2));
        assert!(!reloaded.remove(2));
        assert_eq!(SavedConnections::open(&file).all().len(), 1);
    }
}
