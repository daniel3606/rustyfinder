use std::io::{Read, Write};

use crate::entry::Entry;
use crate::error::{VfsError, VfsResult};

/// A file system backend. Paths are absolute, '/'-separated UTF-8 strings.
///
/// The trait is blocking on purpose: callers run it on worker threads, and
/// network backends keep their own async runtime inside (see DECISIONS.md).
pub trait Vfs: Send + Sync {
    /// Short identifier, e.g. "local" or "sftp://user@host".
    fn id(&self) -> String;

    /// List the direct children of `path`. Order is unspecified.
    fn list(&self, path: &str) -> VfsResult<Vec<Entry>>;

    /// Describe a single item without following a final symlink.
    fn stat(&self, path: &str) -> VfsResult<Entry>;

    /// Local directory to watch for changes to `path`, if this backend's
    /// folders can be watched. Remote backends return `None` and are
    /// refreshed explicitly.
    fn watch_path(&self, _path: &str) -> Option<String> {
        None
    }

    /// Date added and Finder tags of items in `dir`, in the order of
    /// `names`. Reading them costs a system call per item, so they are only
    /// asked for when a view sorts by them. `None` when the backend has no
    /// such facts.
    fn added_and_tags(&self, _dir: &str, _names: &[&str]) -> Option<Vec<(Option<i64>, Vec<String>)>> {
        None
    }

    /// Only the dates added (a sort by date added needs no tags, which
    /// are the slower of the two to read).
    fn added_dates(&self, dir: &str, names: &[&str]) -> Option<Vec<Option<i64>>> {
        self.added_and_tags(dir, names).map(|v| v.into_iter().map(|(added, _)| added).collect())
    }

    /// Only the tags.
    fn tag_lists(&self, dir: &str, names: &[&str]) -> Option<Vec<Vec<String>>> {
        self.added_and_tags(dir, names).map(|v| v.into_iter().map(|(_, tags)| tags).collect())
    }

    /// Free and total bytes on the volume holding `path`, if known.
    fn space(&self, _path: &str) -> Option<Space> {
        None
    }

    // --- Writing. Backends that cannot write keep the defaults.

    /// Create a directory. Fails with `AlreadyExists` if the name is taken.
    fn mkdir(&self, path: &str) -> VfsResult<()> {
        Err(unsupported(path))
    }

    /// Rename or move within this backend. With `replace` false it fails
    /// with `AlreadyExists` instead of overwriting. `CrossDevice` means the
    /// two paths are on different volumes: copy and delete instead.
    fn rename(&self, from: &str, _to: &str, _replace: bool) -> VfsResult<()> {
        Err(unsupported(from))
    }

    /// Delete a file or a symlink (never its target).
    fn remove_file(&self, path: &str) -> VfsResult<()> {
        Err(unsupported(path))
    }

    /// Delete an empty directory.
    fn remove_dir(&self, path: &str) -> VfsResult<()> {
        Err(unsupported(path))
    }

    fn open_read(&self, path: &str) -> VfsResult<Box<dyn Read + Send>> {
        Err(unsupported(path))
    }

    /// Create a new file for writing. Fails with `AlreadyExists` if the
    /// name is taken, so nothing is ever overwritten by accident.
    fn create(&self, path: &str) -> VfsResult<Box<dyn Write + Send>> {
        Err(unsupported(path))
    }

    /// Set modification and creation time (ms since the epoch) where the
    /// backend can. A `None` leaves that time alone.
    fn set_times(&self, _path: &str, _modified_ms: Option<i64>, _created_ms: Option<i64>) -> VfsResult<()> {
        Ok(())
    }

    fn set_mode(&self, _path: &str, _mode: u32) -> VfsResult<()> {
        Ok(())
    }

    /// Create a symlink at `link` pointing to `target` (stored verbatim).
    fn symlink(&self, _target: &str, link: &str) -> VfsResult<()> {
        Err(unsupported(link))
    }

    /// True if both paths are on one volume, so a move is a plain rename.
    fn same_volume(&self, _a: &str, _b: &str) -> bool {
        true
    }

    /// True if both paths name the very same item (e.g. `a.txt` and `A.TXT`
    /// on a case-insensitive volume).
    fn same_item(&self, a: &str, b: &str) -> bool {
        a == b
    }

    /// The item's name exactly as the backend stores it. On a volume that
    /// ignores case, `stat("REPORT.TXT")` finds `report.txt`; this tells
    /// which spelling is really there. `None` if unknown.
    fn stored_name(&self, _path: &str) -> Option<String> {
        None
    }

    /// True if the volume holding `path` cannot be written to.
    fn read_only(&self, _path: &str) -> bool {
        false
    }

    /// Copy one file or symlink (not a directory's contents) from `from` to
    /// a new path `to` within this backend, keeping all metadata the
    /// backend can keep. `progress` gets the bytes copied so far and returns
    /// false to cancel. `Ok(false)` means the backend has no fast path and
    /// the caller should stream the bytes instead.
    fn copy_within(&self, _from: &str, _to: &str, _progress: &mut dyn FnMut(u64) -> bool) -> VfsResult<bool> {
        Ok(false)
    }

    /// Copy a directory's own metadata (dates, permissions, attributes) onto
    /// an existing directory. Called after its children are in place.
    fn copy_dir_metadata(&self, _from: &str, _to: &str) -> VfsResult<()> {
        Ok(())
    }

    // --- Resuming stopped transfers (conflicts spec §6).

    /// Which halves of a resume this backend can do: (read from an offset,
    /// write from an offset). Both sides of a transfer must agree.
    fn resume_support(&self) -> (bool, bool) {
        (false, false)
    }

    /// Read a file from byte `offset` on.
    fn open_read_at(&self, path: &str, _offset: u64) -> VfsResult<Box<dyn Read + Send>> {
        Err(unsupported(path))
    }

    /// Write into an existing file from byte `offset` on; whatever lay past
    /// `offset` is replaced.
    fn open_write_at(&self, path: &str, _offset: u64) -> VfsResult<Box<dyn Write + Send>> {
        Err(unsupported(path))
    }

    // --- Remote facts.

    /// True if this backend stores names as the bytes it is given (servers),
    /// so `é` precomposed and decomposed are two names unless compared after
    /// NFC normalisation. Local volumes already treat them as one.
    fn nfc_names(&self) -> bool {
        false
    }

    /// The item's address as a URL without any password, e.g.
    /// `sftp://bob@example.org/var/www/index.html`. `None` for local items.
    fn url_for(&self, _path: &str) -> Option<String> {
        None
    }

    /// Facts only this backend has, as (label, value) pairs for Get Info,
    /// e.g. ("Storage Class", "GLACIER") on S3. Asked for one item at a
    /// time, never while listing.
    fn details(&self, _path: &str) -> Vec<(String, String)> {
        Vec::new()
    }
}

fn unsupported(path: &str) -> VfsError {
    VfsError::Unsupported(path.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Space {
    pub free: u64,
    pub total: u64,
}
