//! The local-disk backend.

use std::ffi::{c_char, c_int, c_void, CString};
use std::fs::{self, Metadata};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::entry::{Entry, EntryExt, EntryKind};
use crate::error::{VfsError, VfsResult};
use crate::links;
use crate::macmeta as macos;
use crate::paths;
use crate::vfs::{Space, Vfs};

/// BSD `UF_HIDDEN`: the flag Finder uses for invisible items like ~/Library.
#[cfg(target_os = "macos")]
const UF_HIDDEN: u32 = 0x8000;

#[derive(Debug, Default, Clone, Copy)]
pub struct LocalFs;

impl LocalFs {
    pub fn new() -> Self {
        Self
    }
}

impl LocalFs {
    fn list_now(&self, path: &str) -> VfsResult<Vec<Entry>> {
        let reader = fs::read_dir(path).map_err(|e| VfsError::from_io(path, &e))?;
        // `._` companions on FAT, exFAT or SMB volumes are never items.
        let hide_apple_double = links::hides_apple_double(path);
        let mut out = Vec::new();
        let mut link_rows = Vec::new();
        for item in reader {
            // An entry that vanishes or errors mid-listing is skipped rather
            // than failing the whole folder.
            let Ok(item) = item else { continue };
            let name = item.file_name().to_string_lossy().into_owned();
            if hide_apple_double && links::is_apple_double(&name) {
                continue;
            }
            let full = item.path();
            let Ok(meta) = fs::symlink_metadata(&full) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                link_rows.push((out.len(), full.clone()));
            }
            out.push(entry_from(name, &full, &meta));
        }
        resolve_links(&mut out, link_rows);
        Ok(out)
    }

    fn stat_now(&self, path: &str) -> VfsResult<Entry> {
        let meta = fs::symlink_metadata(path).map_err(|e| VfsError::from_io(path, &e))?;
        let name = paths::file_name(path).to_string();
        let mut entry = vec![entry_from(name, Path::new(path), &meta)];
        if meta.file_type().is_symlink() {
            resolve_links(&mut entry, vec![(0, PathBuf::from(path))]);
        }
        Ok(entry.remove(0))
    }
}

/// Run a call on `path` directly, or, on a network volume, with a time
/// limit so a server that went away cannot hang the caller (see netguard).
fn guarded<T: Send + 'static>(path: &str, call: impl FnOnce(String) -> VfsResult<T> + Send + 'static) -> VfsResult<T> {
    if !crate::netguard::on_network_volume(path) {
        return call(path.to_string());
    }
    let owned = path.to_string();
    crate::netguard::with_deadline(crate::netguard::NETWORK_TIMEOUT, move || call(owned)).unwrap_or_else(|| {
        Err(VfsError::Io { path: path.to_string(), message: "The server did not answer within 15 seconds.".into() })
    })
}

impl Vfs for LocalFs {
    fn added_dates(&self, dir: &str, names: &[&str]) -> Option<Vec<Option<i64>>> {
        Some(names.iter().map(|name| macos::added_ms(&Path::new(dir).join(name))).collect())
    }

    fn tag_lists(&self, dir: &str, names: &[&str]) -> Option<Vec<Vec<String>>> {
        Some(names.iter().map(|name| macos::tags(&Path::new(dir).join(name))).collect())
    }

    fn added_and_tags(&self, dir: &str, names: &[&str]) -> Option<Vec<(Option<i64>, Vec<String>)>> {
        Some(
            names
                .iter()
                .map(|name| {
                    let full = Path::new(dir).join(name);
                    (macos::added_ms(&full), macos::tags(&full))
                })
                .collect(),
        )
    }

    fn id(&self) -> String {
        "local".to_string()
    }

    fn list(&self, path: &str) -> VfsResult<Vec<Entry>> {
        guarded(path, |p| LocalFs.list_now(&p))
    }

    fn stat(&self, path: &str) -> VfsResult<Entry> {
        guarded(path, |p| LocalFs.stat_now(&p))
    }

    fn watch_path(&self, path: &str) -> Option<String> {
        Some(path.to_string())
    }

    fn space(&self, path: &str) -> Option<Space> {
        let c_path = std::ffi::CString::new(path).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: c_path is a valid NUL-terminated string and st is a
        // properly sized, writable statvfs.
        if unsafe { libc::statvfs(c_path.as_ptr(), &mut st) } != 0 {
            return None;
        }
        let block = st.f_frsize as u64;
        Some(Space { free: st.f_bavail as u64 * block, total: st.f_blocks as u64 * block })
    }

    fn mkdir(&self, path: &str) -> VfsResult<()> {
        fs::create_dir(path).map_err(|e| VfsError::from_io(path, &e))
    }

    fn rename(&self, from: &str, to: &str, replace: bool) -> VfsResult<()> {
        let (c_from, c_to) = (c_string(from)?, c_string(to)?);
        // RENAME_EXCL makes "do not overwrite" atomic: no window between a
        // check and the rename in which another item could appear.
        let flags = if replace { 0 } else { libc::RENAME_EXCL };
        // SAFETY: both pointers are valid NUL-terminated strings.
        if unsafe { libc::renamex_np(c_from.as_ptr(), c_to.as_ptr(), flags) } == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        // Some file systems (FAT, some network shares) lack RENAME_EXCL.
        if !replace && err.raw_os_error() == Some(libc::ENOTSUP) {
            if fs::symlink_metadata(to).is_ok() {
                return Err(VfsError::AlreadyExists(to.to_string()));
            }
            return fs::rename(from, to).map_err(|e| VfsError::from_io(from, &e));
        }
        // The error may concern either end; name the one that is missing.
        let subject = if err.kind() == io::ErrorKind::AlreadyExists { to } else { from };
        Err(VfsError::from_io(subject, &err))
    }

    fn remove_file(&self, path: &str) -> VfsResult<()> {
        fs::remove_file(path).map_err(|e| VfsError::from_io(path, &e))
    }

    fn remove_dir(&self, path: &str) -> VfsResult<()> {
        match fs::remove_dir(path) {
            Err(e) if e.kind() == io::ErrorKind::DirectoryNotEmpty && links::hides_apple_double(path) => {
                // Only leftover `._` companions (which listings never show)
                // keep it: they go with the folder.
                let rest: Vec<_> =
                    fs::read_dir(path).map_err(|e| VfsError::from_io(path, &e))?.flatten().map(|e| e.path()).collect();
                let companions =
                    |p: &PathBuf| p.file_name().is_some_and(|n| links::is_apple_double(&n.to_string_lossy()));
                if rest.is_empty() || !rest.iter().all(companions) {
                    return Err(VfsError::from_io(path, &e));
                }
                for companion in rest {
                    let _ = fs::remove_file(companion);
                }
                fs::remove_dir(path).map_err(|e| VfsError::from_io(path, &e))
            }
            other => other.map_err(|e| VfsError::from_io(path, &e)),
        }
    }

    fn open_read(&self, path: &str) -> VfsResult<Box<dyn Read + Send>> {
        let file = fs::File::open(path).map_err(|e| VfsError::from_io(path, &e))?;
        Ok(Box::new(file))
    }

    fn create(&self, path: &str) -> VfsResult<Box<dyn Write + Send>> {
        let file =
            fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(|e| VfsError::from_io(path, &e))?;
        Ok(Box::new(file))
    }

    fn resume_support(&self) -> (bool, bool) {
        (true, true)
    }

    fn open_read_at(&self, path: &str, offset: u64) -> VfsResult<Box<dyn Read + Send>> {
        use std::io::{Seek, SeekFrom};
        let mut file = fs::File::open(path).map_err(|e| VfsError::from_io(path, &e))?;
        file.seek(SeekFrom::Start(offset)).map_err(|e| VfsError::from_io(path, &e))?;
        Ok(Box::new(file))
    }

    fn open_write_at(&self, path: &str, offset: u64) -> VfsResult<Box<dyn Write + Send>> {
        use std::io::{Seek, SeekFrom};
        let mut file = fs::OpenOptions::new().write(true).open(path).map_err(|e| VfsError::from_io(path, &e))?;
        // Cut anything past the offset, so the result is never longer than
        // what is written now.
        file.set_len(offset).map_err(|e| VfsError::from_io(path, &e))?;
        file.seek(SeekFrom::Start(offset)).map_err(|e| VfsError::from_io(path, &e))?;
        Ok(Box::new(file))
    }

    fn set_times(&self, path: &str, modified_ms: Option<i64>, created_ms: Option<i64>) -> VfsResult<()> {
        // Read access is enough to set times and also works for folders;
        // write access covers files that are write-only.
        let file = fs::File::open(path)
            .or_else(|_| fs::OpenOptions::new().write(true).open(path))
            .map_err(|e| VfsError::from_io(path, &e))?;
        let mut times = fs::FileTimes::new();
        if let Some(ms) = modified_ms {
            times = times.set_modified(from_ms(ms));
        }
        if let Some(ms) = created_ms {
            use std::os::macos::fs::FileTimesExt;
            times = times.set_created(from_ms(ms));
        }
        file.set_times(times).map_err(|e| VfsError::from_io(path, &e))
    }

    fn set_mode(&self, path: &str, mode: u32) -> VfsResult<()> {
        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777)).map_err(|e| VfsError::from_io(path, &e))
    }

    fn symlink(&self, target: &str, link: &str) -> VfsResult<()> {
        std::os::unix::fs::symlink(target, link).map_err(|e| VfsError::from_io(link, &e))
    }

    fn same_volume(&self, a: &str, b: &str) -> bool {
        match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev(),
            _ => false,
        }
    }

    fn same_item(&self, a: &str, b: &str) -> bool {
        match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }

    fn stored_name(&self, path: &str) -> Option<String> {
        let c_path = CString::new(path).ok()?;
        let mut buffer = vec![0u8; libc::PATH_MAX as usize];
        // SAFETY: the descriptor is opened and closed here. O_SYMLINK opens
        // a link itself rather than its target. F_GETPATH writes at most
        // PATH_MAX bytes, NUL-terminated, into the buffer.
        let ok = unsafe {
            let fd = libc::open(c_path.as_ptr(), libc::O_EVTONLY | libc::O_SYMLINK);
            if fd < 0 {
                return None;
            }
            let rc = libc::fcntl(fd, libc::F_GETPATH, buffer.as_mut_ptr());
            libc::close(fd);
            rc == 0
        };
        if !ok {
            return None;
        }
        let end = buffer.iter().position(|b| *b == 0)?;
        let real = String::from_utf8_lossy(&buffer[..end]).into_owned();
        Some(paths::file_name(&real).to_string())
    }

    fn read_only(&self, path: &str) -> bool {
        let Ok(c_path) = CString::new(path) else {
            return false;
        };
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: valid C string and a writable, correctly sized struct.
        unsafe { libc::statvfs(c_path.as_ptr(), &mut st) == 0 && st.f_flag & libc::ST_RDONLY != 0 }
    }

    fn copy_within(&self, from: &str, to: &str, progress: &mut dyn FnMut(u64) -> bool) -> VfsResult<bool> {
        copy_item(from, to, progress)?;
        Ok(true)
    }

    fn copy_dir_metadata(&self, from: &str, to: &str) -> VfsResult<()> {
        let (c_from, c_to) = (c_string(from)?, c_string(to)?);
        // SAFETY: valid C strings; a null state is allowed.
        let rc = unsafe {
            libc::copyfile(
                c_from.as_ptr(),
                c_to.as_ptr(),
                std::ptr::null_mut(),
                libc::COPYFILE_METADATA | libc::COPYFILE_NOFOLLOW,
            )
        };
        if rc != 0 {
            return Err(VfsError::from_io(to, &io::Error::last_os_error()));
        }
        copy_creation_date(from, to);
        Ok(())
    }
}

fn c_string(path: &str) -> VfsResult<CString> {
    CString::new(path).map_err(|_| VfsError::Invalid(format!("“{path}” contains a NUL character")))
}

fn from_ms(ms: i64) -> SystemTime {
    if ms >= 0 {
        UNIX_EPOCH + Duration::from_millis(ms as u64)
    } else {
        UNIX_EPOCH - Duration::from_millis(ms.unsigned_abs())
    }
}

/// Copy one file or symlink with everything macOS attaches to it: data
/// (holes stay holes), dates, permissions, flags, ACLs, extended attributes
/// (which carry Finder tags, comments and resource forks). On APFS the same
/// volume gets an instant clone. `to` must not exist. On failure or cancel
/// the partial target is removed.
fn copy_item(from: &str, to: &str, progress: &mut dyn FnMut(u64) -> bool) -> VfsResult<()> {
    extern "C" fn status(
        what: c_int,
        stage: c_int,
        state: libc::copyfile_state_t,
        _src: *const c_char,
        _dst: *const c_char,
        ctx: *mut c_void,
    ) -> c_int {
        if what != libc::COPYFILE_COPY_DATA || stage != libc::COPYFILE_PROGRESS {
            return libc::COPYFILE_CONTINUE;
        }
        let mut copied: libc::off_t = 0;
        // SAFETY: `state` is the live state passed by copyfile; `ctx` is the
        // `&mut &mut dyn FnMut` set below and outlives the copyfile call.
        unsafe {
            libc::copyfile_state_get(state, libc::COPYFILE_STATE_COPIED as u32, (&raw mut copied).cast());
            let callback = &mut *(ctx as *mut &mut dyn FnMut(u64) -> bool);
            if callback(copied.max(0) as u64) {
                libc::COPYFILE_CONTINUE
            } else {
                libc::COPYFILE_QUIT
            }
        }
    }

    let (c_from, c_to) = (c_string(from)?, c_string(to)?);
    let mut callback: &mut dyn FnMut(u64) -> bool = progress;
    // COPYFILE_CLONE implies EXCL (never overwrite) and NOFOLLOW_SRC (a
    // symlink is copied as a link), and falls back to a real copy when the
    // volume cannot clone.
    let flags = libc::COPYFILE_METADATA | libc::COPYFILE_DATA | libc::COPYFILE_CLONE | libc::COPYFILE_DATA_SPARSE;
    // SAFETY: the state is allocated and freed here; the callback context
    // points at `callback`, which lives until after copyfile returns.
    let (rc, err) = unsafe {
        let state = libc::copyfile_state_alloc();
        let cb: libc::copyfile_callback_t = Some(status);
        libc::copyfile_state_set(
            state,
            libc::COPYFILE_STATE_STATUS_CB as u32,
            cb.map_or(std::ptr::null(), |f| f as *const c_void),
        );
        libc::copyfile_state_set(state, libc::COPYFILE_STATE_STATUS_CTX as u32, (&raw mut callback).cast());
        let rc = libc::copyfile(c_from.as_ptr(), c_to.as_ptr(), state, flags);
        let err = io::Error::last_os_error();
        libc::copyfile_state_free(state);
        (rc, err)
    };
    if rc == 0 {
        copy_creation_date(from, to);
        return Ok(());
    }
    // EEXIST means the target was there before us: it is not ours to remove.
    if err.kind() == io::ErrorKind::AlreadyExists {
        return Err(VfsError::AlreadyExists(to.to_string()));
    }
    let _ = fs::remove_file(to);
    if err.raw_os_error() == Some(libc::ECANCELED) {
        return Err(VfsError::Cancelled);
    }
    let subject = if err.kind() == io::ErrorKind::NotFound { from } else { to };
    Err(VfsError::from_io(subject, &err))
}

/// copyfile keeps the modification date but a non-clone copy gets "now" as
/// its creation date, so set it explicitly. Best effort: some file systems
/// have no creation date.
fn copy_creation_date(from: &str, to: &str) {
    let Ok(created) = fs::symlink_metadata(from).and_then(|m| m.created()) else {
        return;
    };
    let Ok(c_to) = CString::new(to) else { return };
    let since = created.duration_since(UNIX_EPOCH).unwrap_or_default();
    let mut spec =
        libc::timespec { tv_sec: since.as_secs() as libc::time_t, tv_nsec: since.subsec_nanos() as libc::c_long };
    // SAFETY: attrlist is fully initialised; the buffer is one timespec, as
    // ATTR_CMN_CRTIME requires. NOFOLLOW so a symlink itself is stamped.
    unsafe {
        let mut attrs: libc::attrlist = std::mem::zeroed();
        attrs.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
        attrs.commonattr = libc::ATTR_CMN_CRTIME;
        libc::setattrlist(
            c_to.as_ptr(),
            (&raw mut attrs).cast(),
            (&raw mut spec).cast(),
            std::mem::size_of::<libc::timespec>(),
            libc::FSOPT_NOFOLLOW,
        );
    }
}

fn entry_from(name: String, full: &Path, meta: &Metadata) -> Entry {
    let ft = meta.file_type();
    let kind = if ft.is_symlink() {
        EntryKind::Symlink
    } else if ft.is_dir() {
        EntryKind::Directory
    } else if ft.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };

    let is_dir = ft.is_dir();
    let size = if ft.is_file() { meta.len() } else { 0 };
    // A link's target is looked at afterwards, with a time limit
    // (`resolve_links`).
    let link_target =
        if ft.is_symlink() { fs::read_link(full).ok().map(|t| t.to_string_lossy().into_owned()) } else { None };
    let is_broken_link = false;

    Entry {
        is_hidden: name.starts_with('.') || has_hidden_flag(meta),
        name,
        kind,
        is_dir,
        size,
        modified_ms: meta.modified().ok().and_then(to_ms),
        created_ms: meta.created().ok().and_then(to_ms),
        mode: Some(meta.mode() & 0o7777),
        link_target,
        is_broken_link,
        ext: EntryExt {
            // Date added and tags are read only when a view sorts by
            // them (see `added_and_tags`): they would make listing a big
            // folder several times slower.
            added_ms: None,
            uid: Some(meta.uid()),
            gid: Some(meta.gid()),
            tags: Vec::new(),
            locked: is_locked(meta),
            cloud_only: is_dataless(meta),
            ..Default::default()
        },
    }
}

/// What a symlink points at: (is a folder, size of a file), or `None` when
/// following fails (dangling links and loops show as broken links).
type LinkTarget = Option<(bool, u64)>;

/// Follow the symlinks among `entries` (index, full path of the link), each
/// within `links::LINK_TIMEOUT`. A link that times out is listed as a
/// plain, unresolved link rather than holding the listing up.
fn resolve_links(entries: &mut [Entry], link_rows: Vec<(usize, PathBuf)>) {
    if link_rows.is_empty() {
        return;
    }
    // Where each link leads, so links into one hung mount are given up together.
    let targets: Vec<PathBuf> = link_rows
        .iter()
        .map(|(i, link)| match &entries[*i].link_target {
            Some(t) if t.starts_with('/') => PathBuf::from(t),
            Some(t) => link.parent().map(|p| p.join(t)).unwrap_or_else(|| link.clone()),
            None => link.clone(),
        })
        .collect();
    let resolved = links::resolve_all(targets, links::LINK_TIMEOUT, |target: &Path| -> LinkTarget {
        fs::metadata(target).ok().map(|m| (m.is_dir(), if m.is_file() { m.len() } else { 0 }))
    });
    for ((i, _), result) in link_rows.into_iter().zip(resolved) {
        let entry = &mut entries[i];
        match result {
            links::Resolved::Done(Some((is_dir, size))) => {
                entry.is_dir = is_dir;
                if !is_dir {
                    entry.size = size;
                }
            }
            links::Resolved::Done(None) => entry.is_broken_link = true,
            links::Resolved::TimedOut => {}
        }
    }
}

/// BSD `SF_DATALESS`: the item's content lives only in the cloud (iCloud
/// Drive and other file providers) and is fetched when read.
#[cfg(target_os = "macos")]
const SF_DATALESS: u32 = 0x4000_0000;

#[cfg(target_os = "macos")]
fn is_dataless(meta: &Metadata) -> bool {
    use std::os::macos::fs::MetadataExt as _;
    meta.st_flags() & SF_DATALESS != 0
}

#[cfg(not(target_os = "macos"))]
fn is_dataless(_meta: &Metadata) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn is_locked(meta: &Metadata) -> bool {
    use std::os::macos::fs::MetadataExt as _;
    const UF_IMMUTABLE: u32 = 0x0000_0002;
    meta.st_flags() & UF_IMMUTABLE != 0
}

#[cfg(not(target_os = "macos"))]
fn is_locked(_meta: &Metadata) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn has_hidden_flag(meta: &Metadata) -> bool {
    use std::os::macos::fs::MetadataExt as _;
    meta.st_flags() & UF_HIDDEN != 0
}

#[cfg(not(target_os = "macos"))]
fn has_hidden_flag(_meta: &Metadata) -> bool {
    false
}

fn to_ms(t: SystemTime) -> Option<i64> {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).ok(),
        // Dates before 1970 are negative.
        Err(e) => i64::try_from(e.duration().as_millis()).ok().map(|ms| -ms),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn list_sorted(dir: &Path) -> Vec<Entry> {
        let mut v = LocalFs.list(dir.to_str().unwrap()).unwrap();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    #[test]
    fn lists_files_dirs_and_sizes() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), b"hello").unwrap();
        fs::write(tmp.path().join("empty"), b"").unwrap();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join(".dot"), b"x").unwrap();

        let v = list_sorted(tmp.path());
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec![".dot", "a.txt", "empty", "sub"]);
        assert!(v[0].is_hidden);
        assert_eq!(v[1].size, 5);
        assert_eq!(v[1].kind, EntryKind::File);
        assert!(v[1].modified_ms.unwrap() > 0);
        assert_eq!(v[2].size, 0);
        assert!(v[3].is_dir);
        assert_eq!(v[3].kind, EntryKind::Directory);
    }

    #[test]
    fn symlinks_to_dir_file_nowhere_and_loop() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        fs::create_dir(p.join("real_dir")).unwrap();
        fs::write(p.join("real_file"), b"12345678").unwrap();
        symlink(p.join("real_dir"), p.join("to_dir")).unwrap();
        symlink(p.join("real_file"), p.join("to_file")).unwrap();
        symlink(p.join("missing"), p.join("dangling")).unwrap();
        symlink(p.join("loop_b"), p.join("loop_a")).unwrap();
        symlink(p.join("loop_a"), p.join("loop_b")).unwrap();

        let v = list_sorted(p);
        let get = |n: &str| v.iter().find(|e| e.name == n).unwrap();

        let d = get("to_dir");
        assert_eq!(d.kind, EntryKind::Symlink);
        assert!(d.is_dir && !d.is_broken_link);

        let f = get("to_file");
        assert!(!f.is_dir);
        assert_eq!(f.size, 8);
        assert!(f.link_target.as_deref().unwrap().ends_with("real_file"));

        assert!(get("dangling").is_broken_link);
        assert!(get("loop_a").is_broken_link);
        assert!(get("loop_b").is_broken_link);
    }

    #[test]
    fn unicode_names_are_reported_verbatim() {
        let tmp = tempfile::tempdir().unwrap();
        let nfd = "cafe\u{301}.txt";
        fs::write(tmp.path().join(nfd), b"").unwrap();
        fs::write(tmp.path().join("日本語 📁.txt"), b"").unwrap();
        let v = list_sorted(tmp.path());
        assert_eq!(v.len(), 2);
        // APFS preserves the form it was given; we must not alter it either,
        // or later operations on the name would miss the file.
        assert!(v.iter().any(|e| e.name == nfd));
        assert!(v.iter().any(|e| e.name == "日本語 📁.txt"));
    }

    #[test]
    fn errors_are_mapped() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        assert!(matches!(LocalFs.list(missing.to_str().unwrap()), Err(VfsError::NotFound(_))));

        let file = tmp.path().join("f");
        fs::write(&file, b"").unwrap();
        assert!(matches!(LocalFs.list(file.to_str().unwrap()), Err(VfsError::NotADirectory(_))));

        let locked = tmp.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let res = LocalFs.list(locked.to_str().unwrap());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        // Root ignores permission bits, so only assert when not root.
        if unsafe { libc::geteuid() } != 0 {
            assert!(matches!(res, Err(VfsError::PermissionDenied(_))));
        }
    }

    #[test]
    fn space_is_reported_for_real_paths_only() {
        let tmp = tempfile::tempdir().unwrap();
        let s = LocalFs.space(tmp.path().to_str().unwrap()).unwrap();
        assert!(s.total > 0 && s.free <= s.total);
        assert!(LocalFs.space("/definitely/not/here").is_none());
    }

    fn p(tmp: &tempfile::TempDir, name: &str) -> String {
        tmp.path().join(name).to_str().unwrap().to_string()
    }

    #[test]
    fn rename_refuses_to_overwrite_unless_asked() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(p(&tmp, "a"), b"A").unwrap();
        fs::write(p(&tmp, "b"), b"B").unwrap();
        assert!(matches!(LocalFs.rename(&p(&tmp, "a"), &p(&tmp, "b"), false), Err(VfsError::AlreadyExists(_))));
        assert_eq!(fs::read(p(&tmp, "b")).unwrap(), b"B");
        LocalFs.rename(&p(&tmp, "a"), &p(&tmp, "b"), true).unwrap();
        assert_eq!(fs::read(p(&tmp, "b")).unwrap(), b"A");
        assert!(matches!(LocalFs.rename(&p(&tmp, "gone"), &p(&tmp, "x"), false), Err(VfsError::NotFound(_))));
    }

    #[test]
    fn create_is_exclusive_and_mkdir_reports_existing() {
        let tmp = tempfile::tempdir().unwrap();
        LocalFs.create(&p(&tmp, "f")).unwrap().write_all(b"x").unwrap();
        assert!(matches!(LocalFs.create(&p(&tmp, "f")), Err(VfsError::AlreadyExists(_))));
        LocalFs.mkdir(&p(&tmp, "d")).unwrap();
        assert!(matches!(LocalFs.mkdir(&p(&tmp, "d")), Err(VfsError::AlreadyExists(_))));
        fs::write(p(&tmp, "d/child"), b"").unwrap();
        assert!(matches!(LocalFs.remove_dir(&p(&tmp, "d")), Err(VfsError::NotEmpty(_))));
    }

    #[test]
    fn same_item_sees_through_case_on_case_insensitive_volumes() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(p(&tmp, "photo.jpg"), b"x").unwrap();
        fs::write(p(&tmp, "other.jpg"), b"x").unwrap();
        assert!(LocalFs.same_item(&p(&tmp, "photo.jpg"), &p(&tmp, "photo.jpg")));
        assert!(!LocalFs.same_item(&p(&tmp, "photo.jpg"), &p(&tmp, "other.jpg")));
        // Only meaningful where the volume folds case (the macOS default).
        if fs::metadata(p(&tmp, "PHOTO.JPG")).is_ok() {
            assert!(LocalFs.same_item(&p(&tmp, "photo.jpg"), &p(&tmp, "PHOTO.JPG")));
        }
        assert!(LocalFs.same_volume(&p(&tmp, "photo.jpg"), tmp.path().to_str().unwrap()));
    }

    #[test]
    fn stored_name_reveals_the_spelling_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(p(&tmp, "Report.txt"), b"x").unwrap();
        assert_eq!(LocalFs.stored_name(&p(&tmp, "Report.txt")).as_deref(), Some("Report.txt"));
        if fs::metadata(p(&tmp, "REPORT.TXT")).is_ok() {
            assert_eq!(LocalFs.stored_name(&p(&tmp, "REPORT.TXT")).as_deref(), Some("Report.txt"));
        }
        assert_eq!(LocalFs.stored_name(&p(&tmp, "missing")), None);
        // A symlink reports its own name, not its target's.
        symlink("Report.txt", p(&tmp, "Link")).unwrap();
        assert_eq!(LocalFs.stored_name(&p(&tmp, "Link")).as_deref(), Some("Link"));
    }

    #[test]
    fn set_times_works_on_folders() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(p(&tmp, "d")).unwrap();
        LocalFs.set_times(&p(&tmp, "d"), Some(1_000_000_000_000), None).unwrap();
        assert_eq!(LocalFs.stat(&p(&tmp, "d")).unwrap().modified_ms, Some(1_000_000_000_000));
    }

    #[test]
    fn spec_fo01_copy_keeps_bytes_dates_mode_and_attributes() {
        let tmp = tempfile::tempdir().unwrap();
        let (src, dst) = (p(&tmp, "a.txt"), p(&tmp, "b.txt"));
        fs::write(&src, b"hello world").unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o640)).unwrap();
        // An old modification and creation date, to tell "kept" from "now".
        LocalFs.set_times(&src, Some(1_000_000_000_000), Some(900_000_000_000)).unwrap();
        let attr = std::process::Command::new("xattr")
            .args(["-w", "com.apple.metadata:kMDItemComment", "note", &src])
            .status()
            .unwrap();
        assert!(attr.success());

        let mut seen = 0;
        assert!(LocalFs
            .copy_within(&src, &dst, &mut |n| {
                seen = n;
                true
            })
            .unwrap());
        let (a, b) = (LocalFs.stat(&src).unwrap(), LocalFs.stat(&dst).unwrap());
        assert_eq!(fs::read(&dst).unwrap(), b"hello world");
        assert_eq!(b.modified_ms, Some(1_000_000_000_000));
        assert_eq!(b.created_ms, a.created_ms);
        assert_eq!(b.created_ms, Some(900_000_000_000));
        assert_eq!(b.mode, Some(0o640));
        let out = std::process::Command::new("xattr")
            .args(["-p", "com.apple.metadata:kMDItemComment", &dst])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "note");
        let _ = seen; // clones report no progress; a real copy does

        // Never overwrites.
        assert!(matches!(LocalFs.copy_within(&src, &dst, &mut |_| true), Err(VfsError::AlreadyExists(_))));
        assert_eq!(fs::read(&dst).unwrap(), b"hello world");
    }

    #[test]
    fn copy_keeps_symlinks_as_links_even_when_broken() {
        let tmp = tempfile::tempdir().unwrap();
        symlink("missing-target", p(&tmp, "link")).unwrap();
        LocalFs.copy_within(&p(&tmp, "link"), &p(&tmp, "link2"), &mut |_| true).unwrap();
        let e = LocalFs.stat(&p(&tmp, "link2")).unwrap();
        assert_eq!(e.kind, EntryKind::Symlink);
        assert_eq!(e.link_target.as_deref(), Some("missing-target"));
    }

    #[test]
    fn spec_fo03_sparse_files_stay_sparse() {
        let tmp = tempfile::tempdir().unwrap();
        let (src, dst) = (p(&tmp, "sparse"), p(&tmp, "copy"));
        {
            use std::io::{Seek, SeekFrom};
            let mut f = fs::File::create(&src).unwrap();
            f.seek(SeekFrom::Start(64 * 1024 * 1024)).unwrap();
            f.write_all(b"tail").unwrap();
        }
        LocalFs.copy_within(&src, &dst, &mut |_| true).unwrap();
        let (a, b) = (fs::metadata(&src).unwrap(), fs::metadata(&dst).unwrap());
        assert_eq!(a.len(), b.len());
        // 512-byte blocks actually allocated: far below the 64 MB length.
        assert!(b.blocks() * 512 < 8 * 1024 * 1024, "copy uses {} bytes", b.blocks() * 512);
    }

    #[test]
    fn dir_metadata_copy_keeps_dates() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (p(&tmp, "a"), p(&tmp, "b"));
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let old = filetime_ms(&a) - 86_400_000;
        let f = fs::File::open(&a).unwrap();
        f.set_modified(from_ms(old)).unwrap();
        LocalFs.copy_dir_metadata(&a, &b).unwrap();
        assert_eq!(filetime_ms(&b), old);
    }

    fn filetime_ms(path: &str) -> i64 {
        LocalFs.stat(path).unwrap().modified_ms.unwrap()
    }

    #[test]
    fn stat_reports_one_item() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("x.bin");
        fs::write(&f, vec![0u8; 1024]).unwrap();
        let e = LocalFs.stat(f.to_str().unwrap()).unwrap();
        assert_eq!(e.name, "x.bin");
        assert_eq!(e.size, 1024);
        assert_eq!(e.mode.unwrap() & 0o777, fs::metadata(&f).unwrap().permissions().mode() & 0o777);
    }
}
