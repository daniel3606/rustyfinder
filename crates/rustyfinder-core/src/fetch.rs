//! Copy an item from any file system to a local path, for previews, Quick
//! Look, thumbnails and drags out of archives. Unlike a transfer this is
//! not an activity: nothing is logged and nothing can be undone, because
//! the copy only ever lands in a temporary folder or a promised drop.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use crate::entry::EntryKind;
use crate::paths;
use crate::vfs::Vfs;

/// Total bytes of the files at and below `path`.
pub fn total_size(vfs: &dyn Vfs, path: &str) -> u64 {
    match vfs.stat(path) {
        Ok(entry) if entry.kind == EntryKind::Directory => vfs
            .list(path)
            .unwrap_or_default()
            .iter()
            .map(|child| total_size(vfs, &paths::join(path, &child.name)))
            .sum(),
        Ok(entry) if entry.kind == EntryKind::File => entry.size,
        _ => 0,
    }
}

/// Copy `path` (a file, a link or a whole folder) to the local `dest`,
/// which must not exist yet. Files are written under a temporary name and
/// renamed when complete, so `dest` never holds half a file. `progress`
/// gets the bytes copied so far; `keep_going` returning false stops (and
/// what was written is removed).
pub fn copy_out(
    vfs: &dyn Vfs,
    path: &str,
    dest: &Path,
    keep_going: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(u64),
) -> Result<(), String> {
    if fs::symlink_metadata(dest).is_ok() {
        return Err(format!("“{}” already exists.", dest.display()));
    }
    let mut copied = 0u64;
    let result = copy_item(vfs, path, dest, keep_going, &mut |n| {
        copied += n;
        progress(copied);
    });
    if result.is_err() {
        let _ = fs::remove_dir_all(dest).or_else(|_| fs::remove_file(dest));
    }
    result
}

fn copy_item(
    vfs: &dyn Vfs,
    path: &str,
    dest: &Path,
    keep_going: &dyn Fn() -> bool,
    add: &mut dyn FnMut(u64),
) -> Result<(), String> {
    if !keep_going() {
        return Err("Stopped.".into());
    }
    let entry = vfs.stat(path).map_err(|e| e.to_string())?;
    match entry.kind {
        EntryKind::Directory => {
            fs::create_dir(dest).map_err(|e| e.to_string())?;
            for child in vfs.list(path).map_err(|e| e.to_string())? {
                // Names come from another file system: never let one climb out.
                if child.name.is_empty() || child.name == "." || child.name == ".." || child.name.contains('/') {
                    continue;
                }
                copy_item(vfs, &paths::join(path, &child.name), &dest.join(&child.name), keep_going, add)?;
            }
        }
        EntryKind::Symlink => {
            let target = entry.link_target.unwrap_or_default();
            std::os::unix::fs::symlink(target, dest).map_err(|e| e.to_string())?;
            return Ok(());
        }
        _ => {
            let partial = dest.with_file_name(format!(
                "{}.rustyfinder-partial",
                dest.file_name().and_then(|n| n.to_str()).unwrap_or("item")
            ));
            let written = (|| -> Result<(), String> {
                let mut input = vfs.open_read(path).map_err(|e| e.to_string())?;
                let mut out = fs::File::create(&partial).map_err(|e| e.to_string())?;
                let mut buffer = vec![0u8; 256 * 1024];
                loop {
                    if !keep_going() {
                        return Err("Stopped.".into());
                    }
                    let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    out.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
                    add(n as u64);
                }
                out.flush().map_err(|e| e.to_string())
            })();
            if let Err(e) = written {
                let _ = fs::remove_file(&partial);
                return Err(e);
            }
            fs::rename(&partial, dest).map_err(|e| e.to_string())?;
        }
    }
    if let Some(ms) = entry.modified_ms {
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms.max(0) as u64);
        if let Ok(file) = fs::File::open(dest) {
            let _ = file.set_modified(when);
        }
    }
    if let Some(mode) = entry.mode.filter(|_| entry.kind != EntryKind::Directory) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dest, fs::Permissions::from_mode(mode & 0o777));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::LocalFs;

    #[test]
    fn a_folder_inside_a_zip_comes_out_whole_with_its_dates() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(src.join("docs/deep")).unwrap();
        fs::write(src.join("docs/readme.txt"), "read me").unwrap();
        fs::write(src.join("docs/deep/x.bin"), vec![7u8; 300_000]).unwrap();
        let status = std::process::Command::new("/usr/bin/zip")
            .args(["-q", "-r", "../a.zip", "docs"])
            .current_dir(&src)
            .status()
            .unwrap();
        assert!(status.success());
        let cache = dir.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let opened =
            crate::archive::open(dir.path().join("a.zip").to_str().unwrap(), None, cache.to_str().unwrap()).unwrap();
        let vfs = &*opened.vfs;
        assert_eq!(total_size(vfs, "/docs"), 300_007);

        let out = dir.path().join("out");
        let mut seen = 0;
        copy_out(vfs, "/docs", &out, &|| true, &mut |n| seen = n).unwrap();
        assert_eq!(seen, 300_007);
        assert_eq!(fs::read_to_string(out.join("readme.txt")).unwrap(), "read me");
        assert_eq!(fs::read(out.join("deep/x.bin")).unwrap().len(), 300_000);
        let original = fs::metadata(src.join("docs/readme.txt")).unwrap().modified().unwrap();
        let copied = fs::metadata(out.join("readme.txt")).unwrap().modified().unwrap();
        let apart = original.duration_since(copied).or_else(|_| copied.duration_since(original)).unwrap();
        assert!(apart.as_secs() <= 2, "archived date kept");
        assert!(fs::read_dir(&out).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().contains("partial")));
    }

    #[test]
    fn it_never_replaces_anything_and_a_stop_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), vec![1u8; 1_000_000]).unwrap();
        fs::write(dir.path().join("taken"), "mine").unwrap();
        let src = dir.path().join("a.txt");
        let src = src.to_str().unwrap();
        assert!(copy_out(&LocalFs, src, &dir.path().join("taken"), &|| true, &mut |_| {}).is_err());
        assert_eq!(fs::read_to_string(dir.path().join("taken")).unwrap(), "mine");
        let calls = std::cell::Cell::new(0);
        let stop = || {
            calls.set(calls.get() + 1);
            calls.get() < 3
        };
        assert!(copy_out(&LocalFs, src, &dir.path().join("copy"), &stop, &mut |_| {}).is_err());
        let left: Vec<_> = fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left.len(), 2, "{left:?}");
    }
}
