//! Finding items below a folder by name, extension, kind or content.
//!
//! Two engines produce the same thing, entries whose `name` is the path
//! relative to the folder searched:
//! - `walk` reads the folder tree itself through the `Vfs`, so it works on
//!   any backend (local disk, servers, archives);
//! - `spotlight` asks the system metadata index, which is near-instant but
//!   only knows local, indexed volumes.

use std::collections::VecDeque;
use std::io::Read;
use std::process::Command;

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::entry::{extension_of, Entry, EntryKind};
use crate::fuzzy::wildcard;
use crate::paths;
use crate::sort::fold;
use crate::sync::{kind_matches, KindClass};
use crate::text;
use crate::vfs::Vfs;

/// What the term is compared against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Criterion {
    #[default]
    Name,
    /// One extension or a comma-separated list, without dots.
    Extension,
    /// Folder, Image, Movie, Audio, Document, Archive, Application, Text, Other.
    Kind,
    /// Finder tag name. Only the Spotlight engine can answer this.
    Tags,
    /// Text inside files.
    Content,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Query {
    pub term: String,
    pub criterion: Criterion,
    pub case_sensitive: bool,
    /// The term is a regular expression (names only).
    pub regex: bool,
    pub subfolders: bool,
    /// Include hidden items, and look inside hidden folders.
    pub hidden: bool,
    /// Content search skips files larger than this many bytes (0 = no cap).
    pub content_limit: u64,
}

/// What a finished search has to say besides its results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Outcome {
    /// Files not read because they are over the content size cap.
    pub skipped_large: u64,
    /// Folders that could not be listed (no permission, vanished).
    pub unreadable: u64,
}

const KINDS: [(&str, KindClass); 8] = [
    ("folder", KindClass::Folder),
    ("image", KindClass::Image),
    ("movie", KindClass::Movie),
    ("audio", KindClass::Audio),
    ("document", KindClass::Document),
    ("archive", KindClass::Archive),
    ("application", KindClass::Application),
    ("text", KindClass::Text),
];

enum Test {
    /// Folded name contains the folded term.
    Contains(String),
    /// Name contains the term exactly as typed (case-sensitive).
    ContainsExact(String),
    /// Whole-name wildcard; the flag is "case-sensitive".
    Wildcard(String, bool),
    Regex(Regex),
    Extensions(Vec<String>),
    Kind(KindClass),
    /// None of the known kinds.
    OtherKind,
    /// Every file is a candidate; the content decides.
    Content(String),
    Nothing,
}

/// A query compiled once, then applied to every entry.
pub struct Matcher {
    test: Test,
    case_sensitive: bool,
}

/// Unicode forms differ between file systems; compare in one form.
fn nfc(text: &str) -> String {
    text.nfc().collect()
}

impl Matcher {
    /// Fails only for a regular expression that does not compile.
    pub fn new(query: &Query) -> Result<Matcher, String> {
        let term = query.term.trim();
        let test = match query.criterion {
            _ if term.is_empty() => Test::Nothing,
            Criterion::Name if query.regex => Test::Regex(
                RegexBuilder::new(term)
                    .case_insensitive(!query.case_sensitive)
                    .build()
                    .map_err(|e| format!("The regular expression isn’t valid: {e}"))?,
            ),
            Criterion::Name if term.contains(['*', '?']) => {
                if query.case_sensitive {
                    Test::Wildcard(nfc(term), true)
                } else {
                    Test::Wildcard(fold(term), false)
                }
            }
            Criterion::Name if query.case_sensitive => Test::ContainsExact(nfc(term)),
            Criterion::Name => Test::Contains(fold(term)),
            Criterion::Extension => Test::Extensions(
                term.split(',')
                    .map(|e| e.trim().trim_start_matches(['*', '.']).to_lowercase())
                    .filter(|e| !e.is_empty())
                    .collect(),
            ),
            Criterion::Kind => {
                let wanted = term.to_lowercase();
                match KINDS.iter().find(|(name, _)| *name == wanted) {
                    Some((_, class)) => Test::Kind(*class),
                    None if wanted == "other" => Test::OtherKind,
                    None => Test::Nothing,
                }
            }
            Criterion::Content => {
                Test::Content(if query.case_sensitive { term.to_string() } else { term.to_lowercase() })
            }
            Criterion::Tags => Test::Nothing,
        };
        Ok(Matcher { test, case_sensitive: query.case_sensitive })
    }

    /// Does the entry itself match (everything except content)?
    pub fn matches(&self, entry: &Entry) -> bool {
        let name = paths::file_name(&entry.name);
        match &self.test {
            Test::Contains(term) => fold(name).contains(term.as_str()),
            Test::ContainsExact(term) => nfc(name).contains(term.as_str()),
            Test::Wildcard(pattern, true) => wildcard(pattern, &nfc(name)),
            Test::Wildcard(pattern, false) => wildcard(pattern, &fold(name)),
            Test::Regex(regex) => regex.is_match(&nfc(name)),
            Test::Extensions(list) => !entry.is_dir && list.contains(&extension_of(name)),
            Test::Kind(class) => kind_matches(*class, entry),
            Test::OtherKind => !KINDS.iter().any(|(_, class)| kind_matches(*class, entry)),
            Test::Content(_) => entry.kind == EntryKind::File,
            Test::Nothing => false,
        }
    }

    fn needle(&self) -> Option<&str> {
        match &self.test {
            Test::Content(needle) => Some(needle),
            _ => None,
        }
    }
}

/// How much of a file is looked at to tell text from binary data.
const SAMPLE: usize = 8192;

/// Does the file's text contain the needle? `None` if the file was not
/// read: binary data, unreadable, or over the size cap (counted).
fn content_matches(
    vfs: &dyn Vfs,
    path: &str,
    entry: &Entry,
    matcher: &Matcher,
    limit: u64,
    outcome: &mut Outcome,
) -> Option<bool> {
    let needle = matcher.needle()?;
    if limit > 0 && entry.size > limit {
        outcome.skipped_large += 1;
        return None;
    }
    let mut reader = vfs.open_read(path).ok()?;
    let mut bytes = Vec::with_capacity(entry.size.min(1 << 20) as usize);
    reader.by_ref().take(SAMPLE as u64).read_to_end(&mut bytes).ok()?;
    let guess = text::detect(&bytes);
    if !guess.is_text {
        return None;
    }
    reader.read_to_end(&mut bytes).ok()?;
    let encoding = encoding_rs::Encoding::for_label(guess.name.as_bytes())?;
    let (decoded, _, _) = encoding.decode(&bytes[guess.bom_length.min(bytes.len())..]);
    Some(if matcher.case_sensitive { decoded.contains(needle) } else { decoded.to_lowercase().contains(needle) })
}

/// Walk `root` and report every match through `found`, as an entry whose
/// name is its path relative to `root`. Folders that cannot be listed are
/// skipped. Symlinks are reported but never followed, so loops are harmless.
pub fn walk(
    vfs: &dyn Vfs,
    root: &str,
    query: &Query,
    keep_going: &dyn Fn() -> bool,
    found: &mut dyn FnMut(Entry),
) -> Result<Outcome, String> {
    let matcher = Matcher::new(query)?;
    let mut outcome = Outcome::default();
    // Relative folder paths still to visit; "" is the root itself.
    let mut queue = VecDeque::from([String::new()]);
    while let Some(rel) = queue.pop_front() {
        if !keep_going() {
            break;
        }
        let dir = if rel.is_empty() { root.to_string() } else { paths::join(root, &rel) };
        let Ok(entries) = vfs.list(&dir) else {
            outcome.unreadable += 1;
            continue;
        };
        for entry in entries {
            if entry.is_hidden && !query.hidden {
                continue;
            }
            let rel_name = if rel.is_empty() { entry.name.clone() } else { format!("{rel}/{}", entry.name) };
            let descend = query.subfolders && entry.kind == EntryKind::Directory;
            let mut hit = matcher.matches(&entry);
            if hit && matcher.needle().is_some() {
                if !keep_going() {
                    return Ok(outcome);
                }
                let path = paths::join(&dir, &entry.name);
                hit = content_matches(vfs, &path, &entry, &matcher, query.content_limit, &mut outcome) == Some(true);
            }
            if hit {
                found(Entry { name: rel_name.clone(), ..entry });
            }
            if descend {
                queue.push_back(rel_name);
            }
        }
    }
    Ok(outcome)
}

/// Escape a term for use inside a quoted Spotlight string, where `\`, `"`
/// and `*` have meaning.
fn spotlight_quote(term: &str) -> String {
    let mut out = String::with_capacity(term.len() + 8);
    for c in term.chars() {
        if matches!(c, '\\' | '"' | '\'' | '*') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The Spotlight query for `query`, or `None` if the index cannot answer
/// it (kinds, regular expressions, case-sensitive matching).
pub fn spotlight_expression(query: &Query) -> Option<String> {
    let term = query.term.trim();
    if term.is_empty() || query.regex || query.case_sensitive {
        return None;
    }
    let quoted = spotlight_quote(term);
    Some(match query.criterion {
        Criterion::Name => format!("kMDItemFSName == \"*{quoted}*\"cd"),
        Criterion::Extension => {
            let parts: Vec<String> = term
                .split(',')
                .map(|e| e.trim().trim_start_matches(['*', '.']))
                .filter(|e| !e.is_empty())
                .map(|e| format!("kMDItemFSName == \"*.{}\"c", spotlight_quote(e)))
                .collect();
            if parts.is_empty() {
                return None;
            }
            format!("({})", parts.join(" || "))
        }
        Criterion::Tags => format!("kMDItemUserTags == \"{quoted}\"c"),
        Criterion::Content => format!("kMDItemTextContent == \"{quoted}\"cd"),
        Criterion::Kind => return None,
    })
}

/// Ask the system metadata index. Paths come back absolute; each one below
/// `root` is described through `vfs` and reported like `walk` does.
pub fn spotlight(
    vfs: &dyn Vfs,
    root: &str,
    query: &Query,
    keep_going: &dyn Fn() -> bool,
    found: &mut dyn FnMut(Entry),
) -> Result<Outcome, String> {
    let Some(expression) = spotlight_expression(query) else {
        return walk(vfs, root, query, keep_going, found);
    };
    // The term travels as one argument, never through a shell.
    let output = Command::new("/usr/bin/mdfind")
        .args(["-0", "-onlyin", root, &expression])
        .output()
        .map_err(|e| format!("Spotlight could not be asked: {e}"))?;
    let prefix = if root.ends_with('/') { root.to_string() } else { format!("{root}/") };
    for raw in output.stdout.split(|b| *b == 0) {
        if !keep_going() {
            break;
        }
        let Ok(path) = std::str::from_utf8(raw) else { continue };
        let Some(rel) = path.strip_prefix(&prefix) else { continue };
        if rel.is_empty() || (!query.subfolders && rel.contains('/')) {
            continue;
        }
        if !query.hidden && rel.split('/').any(|part| part.starts_with('.')) {
            continue;
        }
        if let Ok(entry) = vfs.stat(path) {
            if entry.is_hidden && !query.hidden {
                continue;
            }
            found(Entry { name: rel.to_string(), ..entry });
        }
    }
    Ok(Outcome::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::LocalFs;
    use std::fs;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        fs::create_dir_all(p.join("docs/old")).unwrap();
        fs::create_dir_all(p.join(".git")).unwrap();
        fs::write(p.join("alpha.txt"), "plain needle here").unwrap();
        fs::write(p.join("beta.txt"), "nothing").unwrap();
        fs::write(p.join("Report 2024.PDF"), b"%PDF\0\0binary needle").unwrap();
        fs::write(p.join("docs/annual report.md"), "The NEEDLE is upper case").unwrap();
        fs::write(p.join("docs/old/report.jpg"), b"\xff\xd8\xff\0").unwrap();
        fs::write(p.join("docs/photo.PNG"), b"\x89PNG\0").unwrap();
        fs::write(p.join(".git/report"), "hidden").unwrap();
        fs::write(p.join("résumé.txt"), "accented").unwrap();
        dir
    }

    fn run(dir: &tempfile::TempDir, query: Query) -> Vec<String> {
        let mut names = Vec::new();
        walk(&LocalFs::new(), dir.path().to_str().unwrap(), &query, &|| true, &mut |e| names.push(e.name)).unwrap();
        names.sort();
        names
    }

    fn name(term: &str) -> Query {
        Query { term: term.into(), subfolders: true, ..Query::default() }
    }

    #[test]
    fn name_search_finds_matches_at_any_depth_with_relative_paths() {
        // SE-02
        let dir = tree();
        assert_eq!(run(&dir, name("report")), ["Report 2024.PDF", "docs/annual report.md", "docs/old/report.jpg"]);
    }

    #[test]
    fn without_subfolders_only_the_folder_itself_is_searched() {
        let dir = tree();
        let query = Query { subfolders: false, ..name("report") };
        assert_eq!(run(&dir, query), ["Report 2024.PDF"]);
    }

    #[test]
    fn hidden_items_and_folders_are_left_out_unless_asked_for() {
        let dir = tree();
        assert!(!run(&dir, name("report")).contains(&".git/report".to_string()));
        let query = Query { hidden: true, ..name("report") };
        assert!(run(&dir, query).contains(&".git/report".to_string()));
    }

    #[test]
    fn names_match_without_accents_and_case() {
        let dir = tree();
        let found = run(&dir, name("RESUME"));
        assert_eq!(found.len(), 1);
        assert_eq!(fold(&found[0]), "resume.txt");
    }

    #[test]
    fn case_sensitive_and_wildcard_and_regex_names() {
        let dir = tree();
        let sensitive = Query { case_sensitive: true, ..name("report") };
        assert_eq!(run(&dir, sensitive), ["docs/annual report.md", "docs/old/report.jpg"]);
        assert_eq!(run(&dir, name("*.txt")).len(), 3);
        assert_eq!(run(&dir, name("?eta.txt")), ["beta.txt"]);
        let regex = Query { regex: true, ..name(r"^report \d{4}") };
        assert_eq!(run(&dir, regex), ["Report 2024.PDF"]);
        let broken = Query { regex: true, ..name("(") };
        assert!(Matcher::new(&broken).is_err());
    }

    #[test]
    fn extension_lists_match_exactly_ignoring_case() {
        // SE-05
        let dir = tree();
        let query = Query { criterion: Criterion::Extension, ..name("jpg, .PNG") };
        assert_eq!(run(&dir, query), ["docs/old/report.jpg", "docs/photo.PNG"]);
    }

    #[test]
    fn kinds_including_folders_and_other() {
        let dir = tree();
        let kind = |term: &str| Query { criterion: Criterion::Kind, ..name(term) };
        assert_eq!(run(&dir, kind("Folder")), ["docs", "docs/old"]);
        assert_eq!(run(&dir, kind("image")), ["docs/old/report.jpg", "docs/photo.PNG"]);
        assert_eq!(run(&dir, kind("document")), ["Report 2024.PDF"]);
        assert!(run(&dir, kind("other")).is_empty());
        assert!(run(&dir, kind("nonsense")).is_empty());
    }

    #[test]
    fn content_search_reads_text_and_skips_binary() {
        // SE-04: found in the current folder with subfolders off.
        let dir = tree();
        let content = |subfolders: bool, case_sensitive: bool| Query {
            criterion: Criterion::Content,
            subfolders,
            case_sensitive,
            ..name("needle")
        };
        assert_eq!(run(&dir, content(false, false)), ["alpha.txt"]);
        // The PDF has a NUL byte early on: binary, not read.
        assert_eq!(run(&dir, content(true, false)), ["alpha.txt", "docs/annual report.md"]);
        assert_eq!(run(&dir, content(true, true)), ["alpha.txt"]);
    }

    #[test]
    fn content_search_decodes_legacy_encodings_and_honours_the_size_cap() {
        let dir = tempfile::tempdir().unwrap();
        // "Привет, мир" in Windows-1251, long enough for the detector.
        let (bytes, _, _) =
            encoding_rs::WINDOWS_1251.encode("Привет, мир. Это обычный русский текст для проверки поиска.");
        fs::write(dir.path().join("cyrillic.txt"), &bytes).unwrap();
        fs::write(dir.path().join("big.txt"), "x".repeat(5000) + "поиска").unwrap();
        let query = Query { criterion: Criterion::Content, content_limit: 1000, ..name("ПОИСКА") };
        let mut names = Vec::new();
        let root = dir.path().to_str().unwrap();
        let outcome = walk(&LocalFs::new(), root, &query, &|| true, &mut |e| names.push(e.name)).unwrap();
        assert_eq!(names, ["cyrillic.txt"]);
        assert_eq!(outcome.skipped_large, 1);
    }

    #[test]
    fn a_stopped_search_ends_early() {
        let dir = tree();
        let mut count = 0;
        walk(&LocalFs::new(), dir.path().to_str().unwrap(), &name("report"), &|| false, &mut |_| count += 1).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn symlink_loops_do_not_trap_the_walk() {
        let dir = tree();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("docs/loop")).unwrap();
        assert_eq!(run(&dir, name("loop")), ["docs/loop"]);
    }

    #[test]
    fn spotlight_terms_are_quoted() {
        // SE-06
        let query = name("he said \"hi\" * \\");
        assert_eq!(spotlight_expression(&query).unwrap(), "kMDItemFSName == \"*he said \\\"hi\\\" \\* \\\\*\"cd");
        let tags = Query { criterion: Criterion::Tags, ..name("Work") };
        assert_eq!(spotlight_expression(&tags).unwrap(), "kMDItemUserTags == \"Work\"c");
        let kind = Query { criterion: Criterion::Kind, ..name("image") };
        assert_eq!(spotlight_expression(&kind), None);
    }

    #[test]
    fn a_spotlight_search_with_quotes_runs_without_error() {
        // SE-06. A temp folder is not indexed, so no results are expected.
        let dir = tree();
        let mut count = 0;
        let query = name("he said \"hi\"");
        let result = spotlight(&LocalFs::new(), dir.path().to_str().unwrap(), &query, &|| true, &mut |_| count += 1);
        assert!(result.is_ok());
        assert_eq!(count, 0);
    }
}
