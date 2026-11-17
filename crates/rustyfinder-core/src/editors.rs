//! Editors for Edit (⌘E): applications the user lists in Settings →
//! Editing, each with the extensions it edits, and at most one default
//! for every other extension (settings-defaults §3).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Editor {
    /// Bundle path of the application.
    pub app: String,
    /// Lower-case extensions without dots.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Used for extensions no editor lists.
    #[serde(default)]
    pub is_default: bool,
}

/// "txt, .MD md;rs" -> ["txt", "md", "rs"]: lower case, no dots, no
/// duplicates, in the order typed.
pub fn parse_extensions(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in text.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        let ext = part.trim().trim_start_matches('.').to_lowercase();
        if !ext.is_empty() && !out.contains(&ext) {
            out.push(ext);
        }
    }
    out
}

/// Tidy a list before it is saved: one entry per app, extensions parsed
/// again, and at most one default (the first one marked wins).
pub fn normalize(editors: Vec<Editor>) -> Vec<Editor> {
    let mut out: Vec<Editor> = Vec::new();
    let mut has_default = false;
    for mut editor in editors {
        if editor.app.trim().is_empty() || out.iter().any(|e| e.app == editor.app) {
            continue;
        }
        editor.extensions = parse_extensions(&editor.extensions.join(","));
        editor.is_default = editor.is_default && !has_default;
        has_default |= editor.is_default;
        out.push(editor);
    }
    out
}

/// The editor for a file name: the first one that lists its extension,
/// else the default one. `None` means the system's default app.
pub fn editor_for<'a>(editors: &'a [Editor], file_name: &str) -> Option<&'a Editor> {
    let ext = crate::entry::extension_of(file_name);
    if !ext.is_empty() {
        if let Some(editor) = editors.iter().find(|e| e.extensions.contains(&ext)) {
            return Some(editor);
        }
    }
    editors.iter().find(|e| e.is_default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(app: &str, exts: &[&str], default: bool) -> Editor {
        Editor { app: app.into(), extensions: exts.iter().map(|s| s.to_string()).collect(), is_default: default }
    }

    #[test]
    fn extensions_are_parsed_loosely() {
        assert_eq!(parse_extensions("txt, .MD ,md;rs  py"), vec!["txt", "md", "rs", "py"]);
        assert_eq!(parse_extensions(" , ."), Vec::<String>::new());
    }

    #[test]
    fn extension_match_beats_default() {
        let list = [ed("/A.app", &["md", "txt"], false), ed("/B.app", &[], true), ed("/C.app", &["txt"], false)];
        assert_eq!(editor_for(&list, "notes.TXT").unwrap().app, "/A.app");
        assert_eq!(editor_for(&list, "main.rs").unwrap().app, "/B.app");
        assert_eq!(editor_for(&list, "Makefile").unwrap().app, "/B.app");
        assert_eq!(editor_for(&list, "archive.tar.gz").unwrap().app, "/B.app");
    }

    #[test]
    fn without_default_unknown_types_use_the_system() {
        let list = [ed("/A.app", &["md"], false)];
        assert!(editor_for(&list, "x.rs").is_none());
        assert!(editor_for(&[], "x.md").is_none());
    }

    #[test]
    fn normalize_keeps_one_default_and_one_entry_per_app() {
        let list = normalize(vec![
            ed("/A.app", &["MD", ".txt"], true),
            ed("/B.app", &[], true),
            ed("/A.app", &["rs"], false),
            ed(" ", &["x"], false),
        ]);
        assert_eq!(list, vec![ed("/A.app", &["md", "txt"], true), ed("/B.app", &[], false)]);
    }
}
