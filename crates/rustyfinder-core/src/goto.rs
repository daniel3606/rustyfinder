//! Go to Folder: reading what the user typed, completing it, and deciding
//! where it leads.

use crate::paths;
use crate::sort::{fold, natural_cmp};
use crate::vfs::Vfs;

/// Turn typed text into an absolute path.
///
/// Accepts: a path wrapped in single quotes (`'/tmp/my dir'`), a
/// Terminal-escaped path (`/tmp/my\ dir`), `~` and `~/x`, and paths
/// relative to `current_dir`. `home` is the home folder locally, or the
/// login folder on a server.
pub fn parse_typed(text: &str, current_dir: &str, home: &str) -> String {
    let mut text = text.trim().to_string();
    if text.len() >= 2 && text.starts_with('\'') && text.ends_with('\'') {
        // Quoted: everything inside is literal.
        text = text[1..text.len() - 1].to_string();
    } else if text.contains('\\') {
        text = unescape(&text);
    }
    if text.is_empty() {
        return current_dir.to_string();
    }
    let absolute = if text.starts_with('/') || text == "~" || text.starts_with("~/") {
        text
    } else {
        paths::join(current_dir, &text)
    };
    paths::normalize(&absolute, home)
}

/// Remove shell escaping: a backslash makes the next character literal.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(next) => out.push(next),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// One entry of the autocompletion dropdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub name: String,
    pub is_dir: bool,
    /// What the field reads after accepting this entry.
    pub text: String,
}

/// Entries of the deepest typed folder whose names start with the last
/// typed component (ignoring case and accents). Folders first.
pub fn complete(vfs: &dyn Vfs, text: &str, current_dir: &str, home: &str) -> Vec<Completion> {
    let trimmed = text.trim_start();
    // Split into "folder part" (kept as typed) and the partial name.
    let (typed_dir, partial) = match trimmed.rfind('/') {
        Some(i) => (&trimmed[..=i], &trimmed[i + 1..]),
        None => ("", trimmed),
    };
    let dir = if typed_dir.is_empty() { current_dir.to_string() } else { parse_typed(typed_dir, current_dir, home) };
    let Ok(entries) = vfs.list(&dir) else {
        return Vec::new();
    };
    let prefix = fold(partial);
    let mut found: Vec<(String, Completion)> = entries
        .into_iter()
        // Dot-files only when a dot was typed.
        .filter(|e| !e.is_hidden || partial.starts_with('.'))
        .filter(|e| fold(&e.name).starts_with(&prefix))
        .map(|e| {
            let completion =
                Completion { text: accept(trimmed, &e.name, e.is_dir), name: e.name.clone(), is_dir: e.is_dir };
            (fold(&e.name), completion)
        })
        .collect();
    found.sort_by(|a, b| b.1.is_dir.cmp(&a.1.is_dir).then_with(|| natural_cmp(&a.0, &b.0)));
    found.into_iter().map(|(_, c)| c).take(50).collect()
}

/// The field's text after accepting `name` as the last component. A
/// folder gets exactly one trailing slash.
pub fn accept(text: &str, name: &str, is_dir: bool) -> String {
    let base = match text.rfind('/') {
        Some(i) => &text[..=i],
        None => "",
    };
    let mut out = format!("{base}{name}");
    if is_dir && !out.ends_with('/') {
        out.push('/');
    }
    out
}

/// Where a path leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// Navigate to this folder.
    Folder(String),
    /// Navigate to `parent` and select `name`. The file is never opened.
    File { parent: String, name: String },
    /// Nothing there: say so and keep the pop-up open.
    Missing(String),
}

pub fn resolve(vfs: &dyn Vfs, path: &str) -> Destination {
    match vfs.stat(path) {
        Ok(entry) if entry.is_dir => Destination::Folder(path.to_string()),
        Ok(_) => match paths::parent(path) {
            Some(parent) => Destination::File { parent, name: paths::file_name(path).to_string() },
            None => Destination::Folder(path.to_string()),
        },
        Err(_) => Destination::Missing(path.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::LocalFs;
    use std::fs;

    #[test]
    fn spec_qo04_quotes_escapes_tilde_and_relative_paths() {
        let (cur, home) = ("/work/here", "/Users/me");
        assert_eq!(parse_typed("'/Users/me/My Docs'", cur, home), "/Users/me/My Docs");
        assert_eq!(parse_typed("/tmp/my\\ dir", cur, home), "/tmp/my dir");
        assert_eq!(parse_typed("/tmp/it\\'s", cur, home), "/tmp/it's");
        assert_eq!(parse_typed("~", cur, home), "/Users/me");
        assert_eq!(parse_typed("~/Docs/", cur, home), "/Users/me/Docs");
        assert_eq!(parse_typed("sub/dir", cur, home), "/work/here/sub/dir");
        assert_eq!(parse_typed("../other", cur, home), "/work/other");
        assert_eq!(parse_typed("  /a//b/./c  ", cur, home), "/a/b/c");
        assert_eq!(parse_typed("", cur, home), "/work/here");
        // Quotes that do not wrap the whole text are ordinary characters.
        assert_eq!(parse_typed("/tmp/it's", cur, home), "/tmp/it's");
        // A backslash inside quotes stays.
        assert_eq!(parse_typed("'/tmp/a\\b'", cur, home), "/tmp/a\\b");
        // On a server "~" is the login folder.
        assert_eq!(parse_typed("~/www", "/", "/home/bob"), "/home/bob/www");
    }

    fn tree() -> (tempfile::TempDir, String) {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap().to_str().unwrap().to_string();
        for d in ["Users", "usr", "Utilities/sub", ".hidden"] {
            fs::create_dir_all(format!("{root}/{d}")).unwrap();
        }
        fs::write(format!("{root}/Unsorted.txt"), "x").unwrap();
        fs::write(format!("{root}/other"), "x").unwrap();
        std::os::unix::fs::symlink(format!("{root}/Users"), format!("{root}/Ulink")).unwrap();
        (tmp, root)
    }

    #[test]
    fn spec_qo05_completion_adds_exactly_one_trailing_slash() {
        let (_tmp, root) = tree();
        let typed = format!("{root}/Us");
        let found = complete(&LocalFs, &typed, "/", "/");
        // "Us" matches Users and usr (case-insensitive), folders only here.
        let names: Vec<_> = found.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["Users", "usr"]);
        assert_eq!(found[0].text, format!("{root}/Users/"));
        // Accepting again when the field already ends with "/" adds none.
        assert_eq!(accept(&format!("{root}/Users/"), "Users", true), format!("{root}/Users/Users/"));
        assert_eq!(accept("Us", "Users", true), "Users/");
        assert_eq!(accept("/a/no", "notes.txt", false), "/a/notes.txt");
    }

    #[test]
    fn completion_lists_folders_first_includes_links_and_hides_dot_files() {
        let (_tmp, root) = tree();
        let found = complete(&LocalFs, &format!("{root}/U"), "/", "/");
        let names: Vec<_> = found.iter().map(|c| (c.name.as_str(), c.is_dir)).collect();
        // Folders (a symlink to a folder counts) before the file.
        assert_eq!(
            names,
            vec![("Ulink", true), ("Users", true), ("usr", true), ("Utilities", true), ("Unsorted.txt", false)]
        );
        // Everything in the folder, minus dot-files, for a trailing slash.
        let all = complete(&LocalFs, &format!("{root}/"), "/", "/");
        assert!(all.iter().all(|c| c.name != ".hidden"));
        assert_eq!(all.len(), 6);
        // Typing a dot shows dot-files.
        let dots = complete(&LocalFs, &format!("{root}/.h"), "/", "/");
        assert_eq!(dots[0].name, ".hidden");
        // Relative to the current folder, with no slash typed at all.
        let relative = complete(&LocalFs, "Uti", &root, "/");
        assert_eq!(relative[0].text, "Utilities/");
        // A folder that does not exist completes to nothing.
        assert!(complete(&LocalFs, &format!("{root}/nope/x"), "/", "/").is_empty());
    }

    #[test]
    fn spec_qo06_a_file_path_selects_the_file_in_its_folder() {
        let (_tmp, root) = tree();
        assert_eq!(resolve(&LocalFs, &format!("{root}/Users")), Destination::Folder(format!("{root}/Users")));
        assert_eq!(
            resolve(&LocalFs, &format!("{root}/Unsorted.txt")),
            Destination::File { parent: root.clone(), name: "Unsorted.txt".into() }
        );
        // A link to a folder is a folder to go into.
        assert_eq!(resolve(&LocalFs, &format!("{root}/Ulink")), Destination::Folder(format!("{root}/Ulink")));
        assert_eq!(resolve(&LocalFs, &format!("{root}/gone")), Destination::Missing(format!("{root}/gone")));
    }
}
