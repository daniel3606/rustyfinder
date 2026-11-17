//! Small one-step operations: rename, new folder, new file, symlink.

use crate::error::{VfsError, VfsResult};
use crate::ops::engine::TransferMode;
use crate::ops::names;
use crate::paths;
use crate::vfs::Vfs;

/// Rename `path` to the name the user typed. Returns the new path.
///
/// A change that only differs in case (`photo.jpg` -> `Photo.jpg`) is never
/// a conflict with the item itself. Where the volume would treat it as a
/// no-op, the rename goes through a temporary name.
pub fn rename(vfs: &dyn Vfs, path: &str, typed: &str, local: bool) -> VfsResult<String> {
    let name = names::validate(typed, local)?;
    let dir = paths::parent(path).ok_or_else(|| VfsError::Invalid("This item can’t be renamed.".into()))?;
    let target = paths::join(&dir, &name);
    if name == paths::file_name(path) {
        return Ok(target);
    }
    rename_to_path(vfs, path, &target).map_err(|e| match e {
        // Report the name as the user typed it.
        VfsError::AlreadyExists(_) => VfsError::AlreadyExists(typed.to_string()),
        other => other,
    })?;
    Ok(target)
}

/// Rename `path` to the exact path `target` in the same folder, without
/// ever overwriting another item. Handles a pure spelling change of the
/// same item (case, Unicode form).
pub fn rename_to_path(vfs: &dyn Vfs, path: &str, target: &str) -> VfsResult<()> {
    if vfs.stat(target).is_err() {
        return vfs.rename(path, target, false);
    }
    if !vfs.same_item(path, target) {
        return Err(VfsError::AlreadyExists(paths::file_name(target).to_string()));
    }
    let name = paths::file_name(target);
    if vfs.rename(path, target, true).is_ok() && vfs.stored_name(target).is_none_or(|n| n == name) {
        return Ok(());
    }
    // The volume treated it as a no-op: go through a temporary name.
    let dir = paths::parent(target).unwrap_or_else(|| "/".to_string());
    let hop = paths::join(&dir, &format!(".rustyfinder-rename-{}", std::process::id()));
    vfs.rename(path, &hop, false)?;
    vfs.rename(&hop, target, false).inspect_err(|_| {
        let _ = vfs.rename(&hop, path, false);
    })
}

/// Create `base` (or `base 2`, `base 3`…) in `dir`. Returns the new path.
pub fn new_folder(vfs: &dyn Vfs, dir: &str, base: &str) -> VfsResult<String> {
    create_fresh(vfs, dir, base, true, |path| vfs.mkdir(path))
}

/// Create an empty file `base` (numbered if taken) in `dir`.
pub fn new_file(vfs: &dyn Vfs, dir: &str, base: &str) -> VfsResult<String> {
    create_fresh(vfs, dir, base, false, |path| vfs.create(path).map(drop))
}

fn create_fresh(
    vfs: &dyn Vfs,
    dir: &str,
    base: &str,
    is_dir: bool,
    make: impl Fn(&str) -> VfsResult<()>,
) -> VfsResult<String> {
    // Another process can take the name between the check and the create;
    // the create is exclusive, so just pick the next name and try again.
    for _ in 0..100 {
        let taken = |name: &str| vfs.stat(&paths::join(dir, name)).is_ok();
        let path = paths::join(dir, &names::fresh_name(base, is_dir, &taken));
        match make(&path) {
            Ok(()) => return Ok(path),
            Err(VfsError::AlreadyExists(_)) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(VfsError::AlreadyExists(base.to_string()))
}

/// Create `<name> symlink` for `path` in `dest_dir`. The link is relative
/// when both are on one volume, so a folder holding both can be moved or
/// copied elsewhere and the link still works; otherwise absolute.
pub fn make_symlink(vfs: &dyn Vfs, path: &str, dest_dir: &str) -> VfsResult<String> {
    let name = paths::file_name(path);
    let taken = |candidate: &str| vfs.stat(&paths::join(dest_dir, candidate)).is_ok();
    let link = paths::join(dest_dir, &names::suffixed_name(name, "symlink", &taken));
    let target = if vfs.same_volume(path, dest_dir) { relative_path(dest_dir, path) } else { path.to_string() };
    vfs.symlink(&target, &link)?;
    Ok(link)
}

/// `to` as seen from the folder `from_dir`, both absolute: `../b/c`.
/// Whole path components are compared, so `/a/bc` is not inside `/a/b`.
pub fn relative_path(from_dir: &str, to: &str) -> String {
    let split = |p: &str| p.split('/').filter(|c| !c.is_empty()).map(str::to_string).collect::<Vec<_>>();
    let (from, to) = (split(from_dir), split(to));
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_string(); from.len() - common];
    parts.extend(to[common..].iter().cloned());
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// What dropping `sources` onto the folder `target` should do, or `None`
/// if the drop makes no sense (a folder into itself, or a move of an item
/// into the folder it is already in).
///
/// A plain drag moves within one volume and copies across volumes.
/// `force_copy` (Option held) always copies; `force_move` (Command) always
/// moves. `remote` is true when either side is not a local disk: those
/// drags copy unless Command is held.
pub fn drop_mode(
    vfs: &dyn Vfs,
    sources: &[String],
    target: &str,
    force_copy: bool,
    force_move: bool,
    remote: bool,
) -> Option<TransferMode> {
    if sources.is_empty() {
        return None;
    }
    // Option beats Command. With neither, a drag within one local volume
    // is a move; everything else is a copy.
    let same_local_volume = || !remote && sources.iter().all(|s| vfs.same_volume(s, target));
    let moves = !force_copy && (force_move || same_local_volume());
    let mode = if moves { TransferMode::Move } else { TransferMode::Copy };
    let pointless = sources.iter().any(|source| {
        // Whole path components, so "/a/b" is not inside "/a/bc".
        let into_itself =
            target == source || target.strip_prefix(source.as_str()).is_some_and(|rest| rest.starts_with('/'));
        let already_there = paths::parent(source).as_deref() == Some(target);
        into_itself || (mode == TransferMode::Move && already_there)
    });
    (!pointless).then_some(mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::LocalFs;

    /// Reports every pair of paths as being on different volumes.
    struct TwoVolumes;

    impl Vfs for TwoVolumes {
        fn id(&self) -> String {
            "two".into()
        }
        fn list(&self, path: &str) -> VfsResult<Vec<crate::entry::Entry>> {
            LocalFs.list(path)
        }
        fn stat(&self, path: &str) -> VfsResult<crate::entry::Entry> {
            LocalFs.stat(path)
        }
        fn same_volume(&self, _a: &str, _b: &str) -> bool {
            false
        }
    }

    #[test]
    fn spec_fo20_fo21_drop_rules() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(p(&tmp, "src")).unwrap();
        fs::create_dir(p(&tmp, "dst")).unwrap();
        fs::write(p(&tmp, "src/f"), "f").unwrap();
        let (f, src, dst) = (p(&tmp, "src/f"), p(&tmp, "src"), p(&tmp, "dst"));
        let drop = |vfs: &dyn Vfs, copy, mv, remote| drop_mode(vfs, std::slice::from_ref(&f), &dst, copy, mv, remote);

        // Same volume: plain moves, Option copies, Command moves.
        assert_eq!(drop(&LocalFs, false, false, false), Some(TransferMode::Move));
        assert_eq!(drop(&LocalFs, true, false, false), Some(TransferMode::Copy));
        assert_eq!(drop(&LocalFs, false, true, false), Some(TransferMode::Move));
        // Other volume: plain copies, Command moves.
        assert_eq!(drop(&TwoVolumes, false, false, false), Some(TransferMode::Copy));
        assert_eq!(drop(&TwoVolumes, false, true, false), Some(TransferMode::Move));
        // Anything remote: plain copies even "on the same volume".
        assert_eq!(drop(&LocalFs, false, false, true), Some(TransferMode::Copy));
        assert_eq!(drop(&LocalFs, false, true, true), Some(TransferMode::Move));
        // Option wins over Command.
        assert_eq!(drop(&LocalFs, true, true, false), Some(TransferMode::Copy));

        // Moving an item into its own folder does nothing; copying there
        // makes a duplicate, so it is allowed.
        assert_eq!(drop_mode(&LocalFs, std::slice::from_ref(&f), &src, false, false, false), None);
        assert_eq!(drop_mode(&LocalFs, std::slice::from_ref(&f), &src, true, false, false), Some(TransferMode::Copy));
        // A folder never goes into itself or below itself.
        let inner = p(&tmp, "src/inner");
        fs::create_dir(&inner).unwrap();
        assert_eq!(drop_mode(&LocalFs, std::slice::from_ref(&src), &src, true, false, false), None);
        assert_eq!(drop_mode(&LocalFs, std::slice::from_ref(&src), &inner, true, false, false), None);
        // A sibling whose name merely starts the same is fine.
        let sibling = p(&tmp, "srcX");
        fs::create_dir(&sibling).unwrap();
        assert!(drop_mode(&LocalFs, std::slice::from_ref(&src), &sibling, false, false, false).is_some());
        assert_eq!(drop_mode(&LocalFs, &[], &dst, false, false, false), None);
    }

    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn p(tmp: &tempfile::TempDir, rel: &str) -> String {
        tmp.path().join(rel).to_str().unwrap().to_string()
    }

    fn names_in(tmp: &tempfile::TempDir) -> Vec<String> {
        let mut v: Vec<String> =
            fs::read_dir(tmp.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn rename_basic_and_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(p(&tmp, "a.txt"), "a").unwrap();
        fs::write(p(&tmp, "b.txt"), "b").unwrap();
        assert_eq!(rename(&LocalFs, &p(&tmp, "a.txt"), "c.txt", true).unwrap(), p(&tmp, "c.txt"));
        // A taken name is refused and nothing is overwritten.
        assert!(matches!(rename(&LocalFs, &p(&tmp, "c.txt"), "b.txt", true), Err(VfsError::AlreadyExists(_))));
        assert_eq!(fs::read_to_string(p(&tmp, "b.txt")).unwrap(), "b");
        // Same name is a no-op, not an error.
        assert_eq!(rename(&LocalFs, &p(&tmp, "c.txt"), "c.txt", true).unwrap(), p(&tmp, "c.txt"));
        assert!(rename(&LocalFs, &p(&tmp, "c.txt"), "", true).is_err());
        assert_eq!(names_in(&tmp), vec!["b.txt", "c.txt"]);
    }

    #[test]
    fn spec_ec01_case_only_rename_is_not_a_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(p(&tmp, "photo.jpg"), "x").unwrap();
        let new = rename(&LocalFs, &p(&tmp, "photo.jpg"), "Photo.jpg", true).unwrap();
        assert_eq!(new, p(&tmp, "Photo.jpg"));
        assert_eq!(names_in(&tmp), vec!["Photo.jpg"]);
        assert_eq!(fs::read_to_string(&new).unwrap(), "x");
    }

    #[test]
    fn rename_between_unicode_forms_of_the_same_name() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(p(&tmp, "cafe\u{301}"), "x").unwrap(); // NFD
        let new = rename(&LocalFs, &p(&tmp, "cafe\u{301}"), "caf\u{e9}", true).unwrap(); // NFC
        assert_eq!(fs::read_to_string(&new).unwrap(), "x");
        assert_eq!(names_in(&tmp).len(), 1);
    }

    #[test]
    fn slash_typed_by_the_user_is_stored_as_colon() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(p(&tmp, "a"), "x").unwrap();
        rename(&LocalFs, &p(&tmp, "a"), "12/24", true).unwrap();
        assert_eq!(names_in(&tmp), vec!["12:24"]);
    }

    #[test]
    fn a_read_only_folder_can_be_renamed_in_a_writable_parent() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(p(&tmp, "locked")).unwrap();
        fs::write(p(&tmp, "locked/inner"), "i").unwrap();
        fs::set_permissions(p(&tmp, "locked"), fs::Permissions::from_mode(0o555)).unwrap();
        let new = rename(&LocalFs, &p(&tmp, "locked"), "renamed", true).unwrap();
        fs::set_permissions(&new, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(fs::read_to_string(p(&tmp, "renamed/inner")).unwrap(), "i");
    }

    #[test]
    fn spec_fo13_new_folders_and_files_are_numbered() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_str().unwrap();
        assert_eq!(new_folder(&LocalFs, dir, "untitled folder").unwrap(), p(&tmp, "untitled folder"));
        assert_eq!(new_folder(&LocalFs, dir, "untitled folder").unwrap(), p(&tmp, "untitled folder 2"));
        assert_eq!(new_file(&LocalFs, dir, "untitled file.txt").unwrap(), p(&tmp, "untitled file.txt"));
        assert_eq!(new_file(&LocalFs, dir, "untitled file.txt").unwrap(), p(&tmp, "untitled file 2.txt"));
        assert_eq!(fs::metadata(p(&tmp, "untitled file 2.txt")).unwrap().len(), 0);
        assert!(matches!(new_folder(&LocalFs, "/nonexistent-dir", "x"), Err(VfsError::NotFound(_))));
    }

    #[test]
    fn relative_paths_use_whole_components() {
        assert_eq!(relative_path("/a/b", "/a/b/c.txt"), "c.txt");
        assert_eq!(relative_path("/a/b", "/a/x/c.txt"), "../x/c.txt");
        assert_eq!(relative_path("/a/b", "/a/bc/d"), "../bc/d");
        assert_eq!(relative_path("/a/b/c", "/x"), "../../../x");
        assert_eq!(relative_path("/", "/x/y"), "x/y");
        assert_eq!(relative_path("/a", "/a"), ".");
    }

    #[test]
    fn symlinks_are_relative_on_one_volume() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(p(&tmp, "doc.pdf"), "d").unwrap();
        fs::create_dir(p(&tmp, "other")).unwrap();
        let dir = tmp.path().to_str().unwrap();

        let beside = make_symlink(&LocalFs, &p(&tmp, "doc.pdf"), dir).unwrap();
        assert_eq!(beside, p(&tmp, "doc.pdf symlink"));
        assert_eq!(fs::read_link(&beside).unwrap().to_str().unwrap(), "doc.pdf");
        let second = make_symlink(&LocalFs, &p(&tmp, "doc.pdf"), dir).unwrap();
        assert_eq!(second, p(&tmp, "doc.pdf symlink 2"));

        // In another folder (the other pane): still relative, still works.
        let elsewhere = make_symlink(&LocalFs, &p(&tmp, "doc.pdf"), &p(&tmp, "other")).unwrap();
        assert_eq!(fs::read_link(&elsewhere).unwrap().to_str().unwrap(), "../doc.pdf");
        assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "d");
    }
}
