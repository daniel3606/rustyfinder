//! A time limit on file-system calls into network volumes (edge-cases
//! spec §5, remote-smb-nfs §2): an SMB, NFS, AFP or WebDAV mount whose
//! server went away can block a `readdir` or `stat` for minutes. Such
//! calls run on a helper thread, and the caller gives up after 15 s with
//! an error instead of hanging. (A call stuck in the kernel cannot be
//! cancelled; its thread is left to finish whenever the system lets it.)

use std::ffi::CStr;
use std::sync::mpsc;
use std::time::Duration;

/// How long a call into a network volume may take.
pub const NETWORK_TIMEOUT: Duration = Duration::from_secs(15);

/// File-system types that are served over the network.
const NETWORK_TYPES: &[&str] = &["smbfs", "nfs", "afpfs", "webdav", "cifs", "ftp", "autofs_nfs"];

/// The mount point and file-system type of the volume holding `path`,
/// from a mount table of (mount point, type).
pub fn volume_for<'a>(path: &str, mounts: &'a [(String, String)]) -> Option<&'a (String, String)> {
    mounts
        .iter()
        .filter(|(point, _)| {
            point == "/"
                || path == point
                || (path.starts_with(point.as_str()) && path.as_bytes().get(point.len()) == Some(&b'/'))
        })
        .max_by_key(|(point, _)| point.len())
}

/// The system's mount table, as last cached by the kernel: asking does not
/// wait for any server.
fn mounts() -> Vec<(String, String)> {
    let mut list: *mut libc::statfs = std::ptr::null_mut();
    // MNT_NOWAIT: the cached table, never a round trip to a server.
    let count = unsafe { libc::getmntinfo(&mut list, libc::MNT_NOWAIT) };
    if count <= 0 || list.is_null() {
        return Vec::new();
    }
    let entries = unsafe { std::slice::from_raw_parts(list, count as usize) };
    entries
        .iter()
        .map(|e| {
            let point = unsafe { CStr::from_ptr(e.f_mntonname.as_ptr()) }.to_string_lossy().into_owned();
            let kind = unsafe { CStr::from_ptr(e.f_fstypename.as_ptr()) }.to_string_lossy().into_owned();
            (point, kind)
        })
        .collect()
}

/// True if `path` lies on a volume served over the network. The mount
/// table is read at most once a second: a copy stats every item.
pub fn on_network_volume(path: &str) -> bool {
    use std::sync::Mutex;
    use std::time::Instant;
    type Table = Vec<(String, String)>;
    static CACHE: Mutex<Option<(Instant, Table)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.as_ref().is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(1)) {
        *cache = Some((Instant::now(), mounts()));
    }
    let table = &cache.as_ref().expect("filled above").1;
    volume_for(path, table).is_some_and(|(_, kind)| NETWORK_TYPES.contains(&kind.as_str()))
}

/// Run `work` on a helper thread and wait at most `timeout` for it.
/// `None` if it did not finish in time.
pub fn with_deadline<T: Send + 'static>(timeout: Duration, work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new().name("rustyfinder-netguard".into()).spawn(move || {
        let _ = tx.send(work());
    });
    if spawned.is_err() {
        return None;
    }
    rx.recv_timeout(timeout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn the_deepest_mount_point_holding_the_path_wins() {
        let table: Vec<(String, String)> =
            [("/", "apfs"), ("/Volumes/share", "smbfs"), ("/Volumes/share2", "nfs"), ("/Volumes/share/inner", "apfs")]
                .iter()
                .map(|(p, k)| (p.to_string(), k.to_string()))
                .collect();
        assert_eq!(volume_for("/Volumes/share/docs", &table).unwrap().1, "smbfs");
        assert_eq!(volume_for("/Volumes/share", &table).unwrap().1, "smbfs");
        assert_eq!(volume_for("/Volumes/share2/x", &table).unwrap().1, "nfs", "whole components only");
        assert_eq!(volume_for("/Volumes/share/inner/x", &table).unwrap().1, "apfs");
        assert_eq!(volume_for("/Users/me", &table).unwrap().1, "apfs");
    }

    #[test]
    fn the_local_disk_is_not_a_network_volume() {
        assert!(!on_network_volume("/"));
        assert!(!on_network_volume(&std::env::temp_dir().to_string_lossy()));
    }

    #[test]
    fn a_call_that_hangs_is_given_up_on_in_time() {
        let started = Instant::now();
        let result = with_deadline(Duration::from_millis(100), || {
            std::thread::sleep(Duration::from_secs(5));
            1
        });
        assert_eq!(result, None);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(with_deadline(Duration::from_secs(5), || 7), Some(7));
    }
}
