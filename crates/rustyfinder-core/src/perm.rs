//! Unix permission bits as text, and measuring folders.

use crate::entry::EntryKind;
use crate::paths;
use crate::vfs::Vfs;

/// Octal text for permission bits: `644`, or four digits when a special
/// bit (setuid, setgid, sticky) is set: `4755`.
pub fn octal(mode: u32) -> String {
    let mode = mode & 0o7777;
    if mode & 0o7000 != 0 {
        format!("{mode:04o}")
    } else {
        format!("{mode:03o}")
    }
}

/// Parse `644` or `4755`. Anything else (wrong length, digits 8 or 9,
/// other characters) is rejected.
pub fn parse_octal(text: &str) -> Option<u32> {
    let text = text.trim();
    if !(3..=4).contains(&text.len()) || !text.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
        return None;
    }
    u32::from_str_radix(text, 8).ok()
}

/// `rwxr-xr-x` style text, with s/S and t/T for the special bits.
pub fn symbolic(mode: u32) -> String {
    let triplet = |shift: u32, special: u32, on: char, off: char| {
        let bits = (mode >> shift) & 0o7;
        let exec = bits & 1 != 0;
        let last = match (mode & special != 0, exec) {
            (true, true) => on,
            (true, false) => off,
            (false, true) => 'x',
            (false, false) => '-',
        };
        [if bits & 4 != 0 { 'r' } else { '-' }, if bits & 2 != 0 { 'w' } else { '-' }, last]
    };
    let mut out = String::with_capacity(9);
    out.extend(triplet(6, 0o4000, 's', 'S'));
    out.extend(triplet(3, 0o2000, 's', 'S'));
    out.extend(triplet(0, 0o1000, 't', 'T'));
    out
}

/// Items and bytes under `path`, itself included, never following
/// symlinks (so loops end and a link's target is not counted). Parts that
/// cannot be read count as far as they could be. `keep_going` is polled so
/// a long count can be abandoned; `None` means it was.
pub fn measure(vfs: &dyn Vfs, path: &str, keep_going: &dyn Fn() -> bool) -> Option<(u64, u64)> {
    if !keep_going() {
        return None;
    }
    let Ok(entry) = vfs.stat(path) else {
        return Some((1, 0));
    };
    if entry.kind != EntryKind::Directory {
        return Some((1, if entry.kind == EntryKind::Symlink { 0 } else { entry.size }));
    }
    let (mut items, mut bytes) = (1u64, 0u64);
    for child in vfs.list(path).unwrap_or_default() {
        match child.kind {
            EntryKind::Directory => {
                let (i, b) = measure(vfs, &paths::join(path, &child.name), keep_going)?;
                items += i;
                bytes += b;
            }
            EntryKind::Symlink => items += 1,
            _ => {
                items += 1;
                bytes += child.size;
            }
        }
    }
    Some((items, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::LocalFs;
    use std::fs;

    #[test]
    fn spec_pv06_octal_round_trips() {
        assert_eq!(octal(0o644), "644");
        assert_eq!(octal(0o664), "664");
        assert_eq!(octal(0o7), "007");
        assert_eq!(octal(0o4755), "4755");
        assert_eq!(octal(0o100644), "644"); // file-type bits are ignored
        assert_eq!(parse_octal("644"), Some(0o644));
        assert_eq!(parse_octal(" 0755 "), Some(0o755));
        assert_eq!(parse_octal("4755"), Some(0o4755));
        for bad in ["", "64", "64a", "888", "12345", "-44", "6 4"] {
            assert_eq!(parse_octal(bad), None, "{bad:?}");
        }
        // Ticking group-write on 644 gives 664.
        assert_eq!(octal(parse_octal("644").unwrap() | 0o020), "664");
    }

    #[test]
    fn symbolic_text() {
        assert_eq!(symbolic(0o755), "rwxr-xr-x");
        assert_eq!(symbolic(0o640), "rw-r-----");
        assert_eq!(symbolic(0o000), "---------");
        assert_eq!(symbolic(0o4755), "rwsr-xr-x");
        assert_eq!(symbolic(0o2644), "rw-r-Sr--");
        assert_eq!(symbolic(0o1777), "rwxrwxrwt");
    }

    #[test]
    fn spec_vw10_measure_counts_a_tree_but_not_link_targets() {
        let tmp = tempfile::tempdir().unwrap();
        let p = |rel: &str| tmp.path().join(rel).to_str().unwrap().to_string();
        fs::create_dir_all(p("d/sub")).unwrap();
        fs::write(p("d/a"), vec![0u8; 1000]).unwrap();
        fs::write(p("d/sub/b"), vec![0u8; 2000]).unwrap();
        fs::write(p("outside"), vec![0u8; 50_000]).unwrap();
        std::os::unix::fs::symlink(p("outside"), p("d/link")).unwrap();
        std::os::unix::fs::symlink(".", p("d/loop")).unwrap();

        // d, a, sub, b, link, loop.
        assert_eq!(measure(&LocalFs, &p("d"), &|| true), Some((6, 3000)));
        assert_eq!(measure(&LocalFs, &p("d/a"), &|| true), Some((1, 1000)));
        assert_eq!(measure(&LocalFs, &p("d/link"), &|| true), Some((1, 0)));
        assert_eq!(measure(&LocalFs, &p("d"), &|| false), None);
    }
}
