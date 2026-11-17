//! Local-disk rules that keep listings honest and quick (edge-cases §2, §3):
//! - AppleDouble companions (`._name`) on volumes without native extended
//!   attributes are bookkeeping of macOS, never items of their own.
//! - Symlink targets are resolved with a time limit, so a link into a slow
//!   or vanished network volume can't stall a listing.

use std::collections::HashSet;
use std::ffi::CString;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// How long one symlink target may take to resolve (edge-cases §3).
pub const LINK_TIMEOUT: Duration = Duration::from_secs(2);

/// True for the file systems that keep extended attributes themselves.
/// Everywhere else macOS stores them in `._name` companion files.
pub fn has_native_xattrs(fs_type: &str) -> bool {
    matches!(fs_type, "apfs" | "hfs")
}

/// True for a companion file macOS writes on such volumes.
pub fn is_apple_double(name: &str) -> bool {
    name.starts_with("._") && name.len() > 2
}

/// The file system type of the volume holding `path` ("apfs", "exfat",
/// "msdos", "smbfs"…), or `None` if it can't be asked.
pub fn fs_type(path: &str) -> Option<String> {
    let c = CString::new(path).ok()?;
    // SAFETY: zeroed statfs is a valid out-parameter; `c` is NUL-terminated.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let bytes: Vec<u8> = st.f_fstypename.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// True if listings of the folder `dir` should leave out `._` companions:
/// it is on a volume without native extended attributes.
pub fn hides_apple_double(dir: &str) -> bool {
    fs_type(dir).is_some_and(|t| !has_native_xattrs(&t))
}

/// Outcome of resolving one item within the time limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved<T> {
    Done(T),
    /// Took longer than allowed; the listing goes on without it.
    TimedOut,
}

/// The volume-ish part of a path: its first three components
/// (`/Volumes/Share`, `/net/host/x`). Links whose targets share it with
/// one that hung are not tried again.
fn mount_key(path: &Path) -> PathBuf {
    path.components().filter(|c| !matches!(c, Component::CurDir)).take(3).collect()
}

/// Run `resolve` on every target, each with `timeout`, on helper threads
/// (the caller never blocks longer than that per hanging target). When one
/// hangs, its thread is left to finish on its own, and the others whose
/// targets lie under the same mount are reported as timed out at once
/// instead of each waiting out the limit.
pub fn resolve_all<T, F>(targets: Vec<PathBuf>, timeout: Duration, resolve: F) -> Vec<Resolved<T>>
where
    T: Send + 'static,
    F: Fn(&Path) -> T + Send + Sync + 'static,
{
    let resolve = std::sync::Arc::new(resolve);
    let mut results: Vec<Option<Resolved<T>>> = (0..targets.len()).map(|_| None).collect();
    let mut hung: HashSet<PathBuf> = HashSet::new();
    let mut next = 0;
    while next < targets.len() {
        // One worker takes the remaining targets in order.
        let (tx, rx) = mpsc::channel::<(usize, T)>();
        let todo: Vec<(usize, PathBuf)> = (next..targets.len())
            .filter(|i| !hung.contains(&mount_key(&targets[*i])))
            .map(|i| (i, targets[i].clone()))
            .collect();
        for i in next..targets.len() {
            if hung.contains(&mount_key(&targets[i])) {
                results[i] = Some(Resolved::TimedOut);
            }
        }
        if todo.is_empty() {
            break;
        }
        let work = std::sync::Arc::clone(&resolve);
        let order: Vec<usize> = todo.iter().map(|(i, _)| *i).collect();
        let spawned = thread::Builder::new().name("rustyfinder-links".into()).spawn(move || {
            for (i, path) in todo {
                if tx.send((i, work(&path))).is_err() {
                    return;
                }
            }
        });
        if spawned.is_err() {
            // No thread to spare: resolve here rather than not at all.
            for i in order {
                results[i] = Some(Resolved::Done(resolve(&targets[i])));
            }
            break;
        }
        next = targets.len();
        for i in order {
            match rx.recv_timeout(timeout) {
                Ok((done, value)) => results[done] = Some(Resolved::Done(value)),
                Err(_) => {
                    // This one hangs. Give up on it (and its mount) and
                    // carry on with a fresh worker after it.
                    results[i] = Some(Resolved::TimedOut);
                    hung.insert(mount_key(&targets[i]));
                    next = i + 1;
                    break;
                }
            }
        }
    }
    results.into_iter().map(|r| r.unwrap_or(Resolved::TimedOut)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn native_xattr_volumes() {
        assert!(has_native_xattrs("apfs"));
        assert!(has_native_xattrs("hfs"));
        for other in ["exfat", "msdos", "smbfs", "ntfs", "webdav"] {
            assert!(!has_native_xattrs(other), "{other}");
        }
        assert!(is_apple_double("._photo.jpg"));
        assert!(!is_apple_double("._"));
        assert!(!is_apple_double(".photo"));
        assert!(!is_apple_double("photo._"));
    }

    #[test]
    fn the_local_disk_is_apfs_and_keeps_companions_visible() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_str().unwrap();
        assert!(fs_type(dir).is_some());
        assert!(!hides_apple_double(dir), "{:?}", fs_type(dir));
        assert_eq!(fs_type("/definitely/not/here"), None);
        assert!(!hides_apple_double("/definitely/not/here"));
    }

    #[test]
    fn quick_targets_all_resolve_in_order() {
        let targets: Vec<PathBuf> = (0..20).map(|i| PathBuf::from(format!("/t/{i}"))).collect();
        let out = resolve_all(targets, Duration::from_millis(500), |p: &Path| p.to_string_lossy().into_owned());
        assert_eq!(out.len(), 20);
        for (i, r) in out.iter().enumerate() {
            assert_eq!(*r, Resolved::Done(format!("/t/{i}")));
        }
    }

    /// A target on a hanging mount times out once; others on that mount
    /// are skipped without waiting; targets elsewhere still resolve.
    #[test]
    fn a_hanging_mount_costs_one_timeout() {
        let targets = vec![
            PathBuf::from("/Users/me/a"),
            PathBuf::from("/Volumes/Dead/x"),
            PathBuf::from("/Users/me/b"),
            PathBuf::from("/Volumes/Dead/y"),
            PathBuf::from("/Volumes/Dead/z"),
            PathBuf::from("/Volumes/Live/w"),
        ];
        let started = Instant::now();
        let out = resolve_all(targets, Duration::from_millis(200), |p: &Path| {
            if p.starts_with("/Volumes/Dead") {
                thread::sleep(Duration::from_secs(3));
            }
            p.to_string_lossy().into_owned()
        });
        let took = started.elapsed();
        assert!(took < Duration::from_millis(1500), "took {took:?}");
        assert_eq!(out[0], Resolved::Done("/Users/me/a".into()));
        assert_eq!(out[1], Resolved::TimedOut);
        assert_eq!(out[2], Resolved::Done("/Users/me/b".into()));
        assert_eq!(out[3], Resolved::TimedOut);
        assert_eq!(out[4], Resolved::TimedOut);
        assert_eq!(out[5], Resolved::Done("/Volumes/Live/w".into()));
    }

    /// An exFAT disk image, mounted privately for one test.
    struct ExFat {
        _dir: tempfile::TempDir,
        mount: PathBuf,
    }

    impl ExFat {
        fn new() -> Self {
            use std::process::Command;
            let dir = tempfile::tempdir().unwrap();
            let image = dir.path().join("v.dmg");
            let mount = dir.path().join("mnt");
            std::fs::create_dir(&mount).unwrap();
            let made = Command::new("hdiutil")
                .args(["create", "-quiet", "-size", "4m", "-fs", "ExFAT", "-volname", "RFTest", "-layout", "NONE"])
                .arg(&image)
                .status()
                .unwrap();
            assert!(made.success(), "hdiutil create failed");
            let attached = Command::new("hdiutil")
                .args(["attach", "-quiet", "-nobrowse", "-mountpoint"])
                .arg(&mount)
                .arg(&image)
                .status()
                .unwrap();
            assert!(attached.success(), "hdiutil attach failed");
            Self { _dir: dir, mount }
        }

        fn path(&self, rel: &str) -> String {
            self.mount.join(rel).to_string_lossy().into_owned()
        }
    }

    impl Drop for ExFat {
        fn drop(&mut self) {
            let _ =
                std::process::Command::new("hdiutil").args(["detach", "-quiet", "-force"]).arg(&self.mount).status();
        }
    }

    /// EC-06: on exFAT the `._` companions macOS writes for extended
    /// attributes are never listed, counted or left behind.
    #[test]
    fn ec06_apple_double_companions_are_invisible_on_exfat() {
        use crate::local::LocalFs;
        use crate::vfs::Vfs;
        let vol = ExFat::new();
        assert_eq!(fs_type(&vol.path("")).as_deref(), Some("exfat"));
        assert!(hides_apple_double(&vol.path("")));
        std::fs::create_dir(vol.path("dir")).unwrap();
        std::fs::write(vol.path("dir/tagged.txt"), "x").unwrap();
        let set = std::process::Command::new("xattr")
            .args(["-w", "com.apple.metadata:_kMDItemUserTags", "Red"])
            .arg(vol.path("dir/tagged.txt"))
            .status()
            .unwrap();
        assert!(set.success());
        let raw: Vec<String> = std::fs::read_dir(vol.path("dir"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(raw.contains(&"._tagged.txt".to_string()), "macOS wrote no companion: {raw:?}");

        let listed: Vec<String> = LocalFs.list(&vol.path("dir")).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(listed, vec!["tagged.txt"]);
        // Counted once for progress totals.
        assert_eq!(crate::perm::measure(&LocalFs, &vol.path("dir"), &|| true).unwrap().0, 2);

        // A companion whose file is gone does not keep its folder alive.
        std::fs::create_dir(vol.path("orphan")).unwrap();
        std::fs::write(vol.path("orphan/._lost"), "x").unwrap();
        assert!(LocalFs.list(&vol.path("orphan")).unwrap().is_empty());
        LocalFs.remove_dir(&vol.path("orphan")).unwrap();
        // A real item still does.
        std::fs::create_dir(vol.path("full")).unwrap();
        std::fs::write(vol.path("full/real"), "x").unwrap();
        std::fs::write(vol.path("full/._real"), "x").unwrap();
        assert!(LocalFs.remove_dir(&vol.path("full")).is_err());
        assert!(std::path::Path::new(&vol.path("full/real")).exists());
    }

    #[test]
    fn nothing_to_resolve() {
        let out: Vec<Resolved<()>> = resolve_all(Vec::new(), LINK_TIMEOUT, |_| ());
        assert!(out.is_empty());
    }
}
