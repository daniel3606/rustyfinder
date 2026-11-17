//! The file systems currently reachable: the local disk plus every open
//! remote connection. A location anywhere in the app is a connection id
//! and a path on it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use crate::vfs::Vfs;

pub type ConnId = u64;

/// The local disk. Always present.
pub const LOCAL: ConnId = 0;

/// A place: which file system, and where on it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Location {
    pub conn: ConnId,
    pub path: String,
}

impl Location {
    pub fn local(path: impl Into<String>) -> Self {
        Self { conn: LOCAL, path: path.into() }
    }
}

#[derive(Clone)]
struct Conn {
    vfs: Arc<dyn Vfs>,
    /// Shown in tabs and the sidebar: "bob@example.org".
    label: String,
    /// Where `~` points on this file system.
    home: String,
    /// For an archive opened as a folder: the archive file it came from.
    origin: Option<Location>,
    /// The server's URL without password ("sftp://bob@host:22"), set by
    /// the shell after connecting; it lets tabs reconnect after a relaunch
    /// and keeps Recent Folders per server.
    url: String,
}

/// Summary of an open remote connection, for the sidebar and menus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnInfo {
    pub id: ConnId,
    pub label: String,
    pub home: String,
    /// True for an archive being browsed rather than a server.
    pub archive: bool,
}

pub struct VfsRegistry {
    conns: RwLock<HashMap<ConnId, Conn>>,
    next: AtomicU64,
    /// Host, user and port of SFTP connections, for opening a shell there.
    ssh: RwLock<HashMap<ConnId, crate::tools::SshTarget>>,
}

impl VfsRegistry {
    pub fn new(local: Arc<dyn Vfs>, home: impl Into<String>) -> Self {
        let mut conns = HashMap::new();
        conns.insert(
            LOCAL,
            Conn { vfs: local, label: String::new(), home: home.into(), origin: None, url: String::new() },
        );
        Self { conns: RwLock::new(conns), next: AtomicU64::new(1), ssh: RwLock::new(HashMap::new()) }
    }

    /// The backend for a connection, if it is still open.
    pub fn get(&self, id: ConnId) -> Option<Arc<dyn Vfs>> {
        self.conns.read().unwrap().get(&id).map(|c| Arc::clone(&c.vfs))
    }

    pub fn local(&self) -> Arc<dyn Vfs> {
        self.get(LOCAL).expect("the local file system is always registered")
    }

    pub fn home(&self, id: ConnId) -> String {
        self.conns.read().unwrap().get(&id).map(|c| c.home.clone()).unwrap_or_else(|| "/".into())
    }

    pub fn label(&self, id: ConnId) -> String {
        self.conns.read().unwrap().get(&id).map(|c| c.label.clone()).unwrap_or_default()
    }

    /// Register a newly opened connection.
    pub fn add(&self, vfs: Arc<dyn Vfs>, label: impl Into<String>, home: impl Into<String>) -> ConnId {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let conn = Conn { vfs, label: label.into(), home: home.into(), origin: None, url: String::new() };
        self.conns.write().unwrap().insert(id, conn);
        id
    }

    /// Remember which server a connection is to (see `Conn::url`).
    pub fn set_url(&self, id: ConnId, url: &str) {
        if let Some(conn) = self.conns.write().unwrap().get_mut(&id) {
            if id != LOCAL {
                conn.url = url.to_string();
            }
        }
    }

    /// The server URL of a connection; "" for the disk, archives and
    /// connections the shell has not described.
    pub fn url(&self, id: ConnId) -> String {
        self.conns.read().unwrap().get(&id).map(|c| c.url.clone()).unwrap_or_default()
    }

    /// An open connection to the server `url`, if there is one.
    pub fn find_url(&self, url: &str) -> Option<ConnId> {
        if url.is_empty() {
            return None;
        }
        let conns = self.conns.read().unwrap();
        conns.iter().filter(|(_, c)| c.url == url).map(|(id, _)| *id).min()
    }

    /// Register an archive opened as a folder. `origin` is the archive
    /// file itself: going up from the archive's top leads back to it.
    pub fn add_archive(&self, vfs: Arc<dyn Vfs>, label: impl Into<String>, origin: Location) -> ConnId {
        let id = self.add(vfs, label, "/");
        if let Some(conn) = self.conns.write().unwrap().get_mut(&id) {
            conn.origin = Some(origin);
        }
        id
    }

    /// The archive file a connection was opened from, if it is an archive.
    pub fn origin(&self, id: ConnId) -> Option<Location> {
        self.conns.read().unwrap().get(&id).and_then(|c| c.origin.clone())
    }

    /// The open archive made from this file, if there is one.
    pub fn archive_for(&self, origin: &Location) -> Option<ConnId> {
        let conns = self.conns.read().unwrap();
        conns.iter().find(|(_, c)| c.origin.as_ref() == Some(origin)).map(|(id, _)| *id)
    }

    /// Forget a connection. Dropping its last reference closes it. The
    /// local disk cannot be removed.
    pub fn remove(&self, id: ConnId) -> bool {
        self.ssh.write().unwrap().remove(&id);
        id != LOCAL && self.conns.write().unwrap().remove(&id).is_some()
    }

    /// Note where an SFTP connection goes.
    pub fn set_ssh_target(&self, id: ConnId, target: crate::tools::SshTarget) {
        self.ssh.write().unwrap().insert(id, target);
    }

    /// Where an SFTP connection goes; `None` for anything else.
    pub fn ssh_target(&self, id: ConnId) -> Option<crate::tools::SshTarget> {
        self.ssh.read().unwrap().get(&id).cloned()
    }

    /// True if `vfs` is an open server connection (not the local disk, not
    /// an archive), e.g. to know which way a transfer goes.
    pub fn is_server(&self, vfs: &Arc<dyn Vfs>) -> bool {
        let conns = self.conns.read().unwrap();
        conns.iter().any(|(id, c)| *id != LOCAL && c.origin.is_none() && Arc::ptr_eq(&c.vfs, vfs))
    }

    /// Open remote connections, oldest first.
    pub fn remotes(&self) -> Vec<ConnInfo> {
        let conns = self.conns.read().unwrap();
        let mut list: Vec<ConnInfo> = conns
            .iter()
            .filter(|(id, _)| **id != LOCAL)
            .map(|(id, c)| ConnInfo {
                id: *id,
                label: c.label.clone(),
                home: c.home.clone(),
                archive: c.origin.is_some(),
            })
            .collect();
        list.sort_by_key(|c| c.id);
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::LocalFs;

    #[test]
    fn local_is_always_there_and_remotes_come_and_go() {
        let reg = VfsRegistry::new(Arc::new(LocalFs), "/Users/me");
        assert!(reg.get(LOCAL).is_some());
        assert_eq!(reg.home(LOCAL), "/Users/me");
        assert!(reg.remotes().is_empty());
        assert!(!reg.remove(LOCAL));

        let a = reg.add(Arc::new(LocalFs), "bob@example.org", "/home/bob");
        let b = reg.add(Arc::new(LocalFs), "s3://bucket", "/");
        assert_ne!(a, b);
        assert_eq!(reg.label(a), "bob@example.org");
        assert_eq!(reg.home(a), "/home/bob");
        assert_eq!(reg.remotes().iter().map(|c| c.id).collect::<Vec<_>>(), vec![a, b]);

        assert!(reg.remove(a));
        assert!(!reg.remove(a));
        assert!(reg.get(a).is_none());
        assert_eq!(reg.home(a), "/"); // a closed connection has no home
        assert_eq!(reg.remotes().len(), 1);
    }

    #[test]
    fn an_archive_remembers_the_file_it_came_from() {
        let reg = VfsRegistry::new(Arc::new(LocalFs), "/Users/me");
        let origin = Location::local("/Users/me/a.zip");
        let id = reg.add_archive(Arc::new(LocalFs), "a.zip", origin.clone());
        assert_eq!(reg.origin(id), Some(origin.clone()));
        assert_eq!(reg.archive_for(&origin), Some(id));
        assert_eq!(reg.origin(LOCAL), None);
        assert!(reg.remotes()[0].archive);
        // Only servers count as the far end of a transfer.
        let archive = reg.get(id).unwrap();
        assert!(!reg.is_server(&archive));
        assert!(!reg.is_server(&reg.local()));
        let server = reg.add(Arc::new(LocalFs), "bob@example.org", "/");
        assert!(reg.is_server(&reg.get(server).unwrap()));
        reg.remove(id);
        assert_eq!(reg.archive_for(&origin), None);
    }

    #[test]
    fn connections_remember_their_server_url() {
        let reg = VfsRegistry::new(Arc::new(LocalFs), "/Users/me");
        let a = reg.add(Arc::new(LocalFs), "bob@host", "/home/bob");
        assert_eq!(reg.url(a), "");
        reg.set_url(a, "sftp://bob@host:22");
        reg.set_url(LOCAL, "sftp://nope");
        assert_eq!(reg.url(a), "sftp://bob@host:22");
        assert_eq!(reg.url(LOCAL), "");
        assert_eq!(reg.find_url("sftp://bob@host:22"), Some(a));
        assert_eq!(reg.find_url(""), None);
        reg.remove(a);
        assert_eq!(reg.find_url("sftp://bob@host:22"), None);
    }
}
