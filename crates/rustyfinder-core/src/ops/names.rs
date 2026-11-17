//! Naming rules: free names for duplicates and "keep both", and validation
//! of names the user types.

use crate::error::VfsError;

/// Longest file name most file systems accept, in UTF-8 bytes.
pub const MAX_NAME_BYTES: usize = 255;

/// Split a name into stem and extension (with its dot). Folders and
/// dot-files have no extension: "notes.txt" -> ("notes", ".txt"),
/// ".zshrc" -> (".zshrc", ""), "project.v2" as a folder -> ("project.v2", "").
pub fn split_ext(name: &str, is_dir: bool) -> (&str, &str) {
    if is_dir {
        return (name, "");
    }
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name.split_at(i),
        _ => (name, ""),
    }
}

/// First free name in the series `<stem> <word><ext>`, `<stem> <word> 2<ext>`…
/// With an empty `word` the series is `<stem> 2<ext>`, `<stem> 3<ext>`…
fn numbered(stem: &str, word: &str, ext: &str, taken: &dyn Fn(&str) -> bool) -> String {
    if !word.is_empty() {
        let first = format!("{stem} {word}{ext}");
        if !taken(&first) {
            return first;
        }
    }
    let prefix = if word.is_empty() { stem.to_string() } else { format!("{stem} {word}") };
    (2u64..)
        .map(|n| format!("{prefix} {n}{ext}"))
        .find(|candidate| !taken(candidate))
        .expect("an unbounded series always has a free name")
}

/// Name for a duplicate: `photo copy.jpg`, then `photo copy 2.jpg`…
pub fn duplicate_name(name: &str, is_dir: bool, taken: &dyn Fn(&str) -> bool) -> String {
    let (stem, ext) = split_ext(name, is_dir);
    numbered(stem, "copy", ext, taken)
}

/// Name for "Keep Both": `a 2.txt`, then `a 3.txt`…
pub fn keep_both_name(name: &str, is_dir: bool, taken: &dyn Fn(&str) -> bool) -> String {
    let (stem, ext) = split_ext(name, is_dir);
    numbered(stem, "", ext, taken)
}

/// Name for a new item: `untitled folder`, then `untitled folder 2`…
/// `base` may carry an extension (`untitled file.txt` -> `untitled file 2.txt`).
pub fn fresh_name(base: &str, is_dir: bool, taken: &dyn Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    keep_both_name(base, is_dir, taken)
}

/// Name with a suffix word, for aliases and symlinks: `<name> alias`.
pub fn suffixed_name(name: &str, word: &str, taken: &dyn Fn(&str) -> bool) -> String {
    numbered(name, word, "", taken)
}

/// Check a name the user typed. `local` applies the macOS convention: a
/// typed "/" is stored as ":" on disk, and a typed ":" is refused.
/// Returns the name to store.
pub fn validate(typed: &str, local: bool) -> Result<String, VfsError> {
    let invalid = |why: &str| Err(VfsError::Invalid(why.to_string()));
    if typed.is_empty() {
        return invalid("The name can’t be empty.");
    }
    if typed == "." || typed == ".." {
        return invalid("That name is reserved.");
    }
    if typed.contains('\0') {
        return invalid("The name contains a character that can’t be used.");
    }
    let stored = if local {
        if typed.contains(':') {
            return invalid("The name can’t contain a colon.");
        }
        typed.replace('/', ":")
    } else {
        if typed.contains('/') {
            return invalid("The name can’t contain a slash.");
        }
        typed.to_string()
    };
    if stored.len() > MAX_NAME_BYTES {
        return invalid("The name is too long.");
    }
    Ok(stored)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken<'a>(names: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |n| names.contains(&n)
    }

    #[test]
    fn extension_split() {
        assert_eq!(split_ext("photo.jpg", false), ("photo", ".jpg"));
        assert_eq!(split_ext("archive.tar.gz", false), ("archive.tar", ".gz"));
        assert_eq!(split_ext(".zshrc", false), (".zshrc", ""));
        assert_eq!(split_ext("Makefile", false), ("Makefile", ""));
        assert_eq!(split_ext("trailing.", false), ("trailing.", ""));
        // A folder's dot is part of its name, never an extension.
        assert_eq!(split_ext("project.v2", true), ("project.v2", ""));
    }

    #[test]
    fn spec_fo12_duplicate_names() {
        assert_eq!(duplicate_name("photo.jpg", false, &taken(&[])), "photo copy.jpg");
        assert_eq!(duplicate_name("photo.jpg", false, &taken(&["photo copy.jpg"])), "photo copy 2.jpg");
        assert_eq!(
            duplicate_name("photo.jpg", false, &taken(&["photo copy.jpg", "photo copy 2.jpg"])),
            "photo copy 3.jpg"
        );
        assert_eq!(duplicate_name("dir.v2", true, &taken(&[])), "dir.v2 copy");
        assert_eq!(duplicate_name(".env", false, &taken(&[])), ".env copy");
    }

    #[test]
    fn spec_cf04_keep_both_names() {
        assert_eq!(keep_both_name("a.txt", false, &taken(&[])), "a 2.txt");
        assert_eq!(keep_both_name("a.txt", false, &taken(&["a 2.txt"])), "a 3.txt");
        assert_eq!(keep_both_name("folder", true, &taken(&["folder 2", "folder 3"])), "folder 4");
    }

    #[test]
    fn spec_fo13_fresh_names() {
        assert_eq!(fresh_name("untitled folder", true, &taken(&[])), "untitled folder");
        assert_eq!(fresh_name("untitled folder", true, &taken(&["untitled folder"])), "untitled folder 2");
        assert_eq!(
            fresh_name("untitled file.txt", false, &taken(&["untitled file.txt", "untitled file 2.txt"])),
            "untitled file 3.txt"
        );
        assert_eq!(suffixed_name("doc.pdf", "alias", &taken(&[])), "doc.pdf alias");
        assert_eq!(suffixed_name("doc.pdf", "alias", &taken(&["doc.pdf alias"])), "doc.pdf alias 2");
    }

    #[test]
    fn validation() {
        assert_eq!(validate("ok.txt", true).unwrap(), "ok.txt");
        assert_eq!(validate(" lead and trail. ", true).unwrap(), " lead and trail. ");
        assert_eq!(validate("12/24 notes", true).unwrap(), "12:24 notes");
        assert!(validate("a:b", true).is_err());
        assert_eq!(validate("a:b", false).unwrap(), "a:b");
        assert!(validate("a/b", false).is_err());
        assert!(validate("", true).is_err());
        assert!(validate("..", true).is_err());
        assert!(validate(&"x".repeat(256), true).is_err());
        assert!(validate(&"x".repeat(255), true).is_ok());
        // 255 is a byte limit: 128 two-byte characters are too long.
        assert!(validate(&"é".repeat(128), true).is_err());
    }
}
