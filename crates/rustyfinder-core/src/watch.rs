//! Watches one directory for changes to its direct children.
//!
//! Two sources feed the same callback, because neither is enough alone:
//! - FSEvents (via `notify`) reports content and metadata changes of the
//!   children, but stays silent when the watched folder itself is deleted,
//!   renamed or unmounted.
//! - A kqueue vnode watch on the folder reports exactly that, plus entries
//!   being added or removed, but not changes inside existing files.

use std::ffi::CString;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// Calls `on_change` (on a background thread) whenever something directly
/// inside the watched directory, or the directory itself, changes. Dropping
/// the watcher stops it.
pub struct DirWatcher {
    _watcher: Option<RecommendedWatcher>,
    _vnode: Option<VnodeWatch>,
}

impl DirWatcher {
    pub fn new(dir: &str, on_change: impl Fn() + Send + Sync + 'static) -> Option<Self> {
        let on_change: Arc<dyn Fn() + Send + Sync> = Arc::new(on_change);
        let vnode = VnodeWatch::new(dir, Arc::clone(&on_change));
        let events = Self::fs_events(dir, on_change);
        if vnode.is_none() && events.is_none() {
            return None;
        }
        Some(Self { _watcher: events, _vnode: vnode })
    }

    fn fs_events(dir: &str, on_change: Arc<dyn Fn() + Send + Sync>) -> Option<RecommendedWatcher> {
        // The system reports resolved paths (/private/tmp, not /tmp), so
        // compare against both spellings.
        let raw = PathBuf::from(dir);
        let resolved = std::fs::canonicalize(&raw).unwrap_or_else(|_| raw.clone());
        let roots = [raw.clone(), resolved];

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            // macOS delivers events for the whole subtree. Reloading the
            // home folder on every write deep inside ~/Library would be
            // constant churn, so only direct children count.
            if event.paths.iter().any(|p| concerns(p, &roots)) {
                on_change();
            }
        })
        .ok()?;
        watcher.watch(&raw, RecursiveMode::NonRecursive).ok()?;
        Some(watcher)
    }
}

/// kqueue watch on the directory's own vnode.
struct VnodeWatch {
    /// Write end of a pipe; writing (or closing) it stops the thread.
    stop: RawFd,
}

impl VnodeWatch {
    fn new(dir: &str, on_change: Arc<dyn Fn() + Send + Sync>) -> Option<Self> {
        let c_dir = CString::new(dir).ok()?;
        // SAFETY: plain libc calls on descriptors this function owns; every
        // failure path closes what it opened.
        unsafe {
            // O_EVTONLY: watch without keeping the volume from unmounting.
            let fd = libc::open(c_dir.as_ptr(), libc::O_EVTONLY);
            if fd < 0 {
                return None;
            }
            let kq = libc::kqueue();
            let mut pipe = [0 as RawFd; 2];
            if kq < 0 || libc::pipe(pipe.as_mut_ptr()) != 0 {
                libc::close(fd);
                if kq >= 0 {
                    libc::close(kq);
                }
                return None;
            }
            let mut changes: [libc::kevent; 2] = std::mem::zeroed();
            changes[0].ident = fd as usize;
            changes[0].filter = libc::EVFILT_VNODE;
            changes[0].flags = libc::EV_ADD | libc::EV_CLEAR;
            changes[0].fflags = libc::NOTE_WRITE | libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_REVOKE;
            changes[1].ident = pipe[0] as usize;
            changes[1].filter = libc::EVFILT_READ;
            changes[1].flags = libc::EV_ADD;
            let registered = libc::kevent(kq, changes.as_ptr(), 2, std::ptr::null_mut(), 0, std::ptr::null());
            let (read_end, write_end) = (pipe[0], pipe[1]);
            let spawned = registered == 0
                && std::thread::Builder::new()
                    .name("rustyfinder-vnode-watch".into())
                    .spawn(move || {
                        loop {
                            let mut event: libc::kevent = std::mem::zeroed();
                            let n = libc::kevent(kq, std::ptr::null(), 0, &mut event, 1, std::ptr::null());
                            if n < 0 && *libc::__error() == libc::EINTR {
                                continue;
                            }
                            if n <= 0 || event.filter == libc::EVFILT_READ {
                                break;
                            }
                            on_change();
                        }
                        libc::close(fd);
                        libc::close(kq);
                        libc::close(read_end);
                    })
                    .is_ok();
            if !spawned {
                for d in [fd, kq, read_end, write_end] {
                    libc::close(d);
                }
                return None;
            }
            Some(Self { stop: write_end })
        }
    }
}

impl Drop for VnodeWatch {
    fn drop(&mut self) {
        // SAFETY: `stop` is the pipe's write end, owned by this struct.
        // Closing it makes the read end readable (EOF), which ends the thread.
        unsafe {
            libc::close(self.stop);
        }
    }
}

fn concerns(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path == root || path.parent() == Some(root.as_path()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn only_direct_children_count() {
        let roots = [PathBuf::from("/a/b")];
        assert!(concerns(Path::new("/a/b"), &roots));
        assert!(concerns(Path::new("/a/b/file"), &roots));
        assert!(!concerns(Path::new("/a/b/sub/file"), &roots));
        assert!(!concerns(Path::new("/a"), &roots));
        assert!(!concerns(Path::new("/a/bb/file"), &roots));
    }

    #[test]
    fn reports_a_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, rx) = channel();
        let tx = std::sync::Mutex::new(tx);
        let _w = DirWatcher::new(tmp.path().to_str().unwrap(), move || {
            let _ = tx.lock().unwrap().send(());
        })
        .expect("watcher");
        // FSEvents needs a moment to start delivering.
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(tmp.path().join("new.txt"), b"x").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(10)).is_ok(), "no change event");
    }

    #[test]
    fn reports_deletion_of_the_watched_folder_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a/inner");
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = channel();
        let tx = std::sync::Mutex::new(tx);
        let _w = DirWatcher::new(dir.to_str().unwrap(), move || {
            let _ = tx.lock().unwrap().send(());
        })
        .expect("watcher");
        std::thread::sleep(Duration::from_millis(300));
        while rx.try_recv().is_ok() {}
        std::fs::remove_dir_all(tmp.path().join("a")).unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok(), "no event for deletion");
    }

    #[test]
    fn dropping_the_watcher_stops_callbacks() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, rx) = channel();
        let tx = std::sync::Mutex::new(tx);
        let w = DirWatcher::new(tmp.path().to_str().unwrap(), move || {
            let _ = tx.lock().unwrap().send(());
        })
        .expect("watcher");
        std::thread::sleep(Duration::from_millis(300));
        drop(w);
        std::thread::sleep(Duration::from_millis(200));
        while rx.try_recv().is_ok() {}
        std::fs::write(tmp.path().join("late.txt"), b"x").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(800)).is_err());
    }
}
