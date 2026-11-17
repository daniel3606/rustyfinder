//! Helpers for absolute, '/'-separated paths. Purely textual: nothing here
//! touches the disk or resolves symlinks.

/// Clean up a user-supplied path: expand a leading `~`, collapse repeated
/// slashes, resolve `.` and `..` textually, drop a trailing slash.
/// `home` is the user's home directory (used for `~`).
pub fn normalize(path: &str, home: &str) -> String {
    let expanded = if path == "~" {
        home.to_string()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("{}/{}", home.trim_end_matches('/'), rest)
    } else {
        path.to_string()
    };

    let mut parts: Vec<&str> = Vec::new();
    for part in expanded.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

/// Parent directory, or `None` for the root.
pub fn parent(path: &str) -> Option<String> {
    if path == "/" || path.is_empty() {
        return None;
    }
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(i) => Some(trimmed[..i].to_string()),
        None => None,
    }
}

/// Last path component ("" for the root).
pub fn file_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(i) => &trimmed[i + 1..],
        None => trimmed,
    }
}

/// Append a child name to a directory path.
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Every ancestor from the root down to `path` itself, as (label, path).
/// Used by the path bar.
pub fn components(path: &str) -> Vec<(String, String)> {
    let mut out = vec![("/".to_string(), "/".to_string())];
    let mut current = String::new();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        current.push('/');
        current.push_str(part);
        out.push((part.to_string(), current.clone()));
    }
    out
}

/// Escape a path so a shell reads it as exactly one argument, the way a
/// path dragged into Terminal looks: `/tmp/my\ file\'s.txt`.
pub fn shell_escape(path: &str) -> String {
    // Control characters cannot be backslash-escaped (a backslash before a
    // newline means "continue the line"), so such names use $'...' quoting.
    if path.chars().any(char::is_control) {
        let mut out = String::from("$'");
        for c in path.chars() {
            match c {
                '\'' => out.push_str("\\'"),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('\'');
        return out;
    }
    if path.is_empty() {
        return "''".to_string();
    }
    let mut out = String::with_capacity(path.len() + 8);
    for c in path.chars() {
        // Everything outside this safe set gets a backslash. That covers
        // spaces, quotes, globs, and Unicode spaces such as U+3000.
        let safe = c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c) || (!c.is_ascii() && !c.is_whitespace());
        if !safe {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_fo22_shell_escape_gives_one_argument() {
        assert_eq!(shell_escape("/tmp/plain-file_1.txt"), "/tmp/plain-file_1.txt");
        assert_eq!(shell_escape("/tmp/my file's.txt"), "/tmp/my\\ file\\'s.txt");
        assert_eq!(shell_escape("/a/full\u{3000}width"), "/a/full\\\u{3000}width");
        assert_eq!(shell_escape(""), "''");
        assert_eq!(shell_escape("a\nb"), "$'a\\nb'");

        // The real test: a shell must see exactly one existing path.
        let tmp = tempfile::tempdir().unwrap();
        let names = [
            "my file's.txt",
            "full\u{3000}width space",
            "quote\"and$dollar`tick",
            "glob*?[x] & ; | (paren) <r> #hash !bang ~tilde",
            "line\nbreak\ttab",
            "-leading-dash",
            "caf\u{e9} \u{1f4c1}",
        ];
        for name in names {
            let path = tmp.path().join(name);
            std::fs::write(&path, b"x").unwrap();
            let escaped = shell_escape(path.to_str().unwrap());
            let out = std::process::Command::new("/bin/zsh")
                .arg("-c")
                .arg(format!("for a in {escaped}; do [ -e \"$a\" ] && echo ok; done"))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok", "{name:?} -> {escaped}");
        }
    }

    #[test]
    fn normalize_cases() {
        let h = "/Users/me";
        assert_eq!(normalize("~", h), "/Users/me");
        assert_eq!(normalize("~/Docs/", h), "/Users/me/Docs");
        assert_eq!(normalize("/a//b/./c/..", h), "/a/b");
        assert_eq!(normalize("/..", h), "/");
        assert_eq!(normalize("", h), "/");
        assert_eq!(normalize("/a/b/", h), "/a/b");
        // A tilde that is not a prefix is an ordinary character.
        assert_eq!(normalize("/a/~b", h), "/a/~b");
    }

    #[test]
    fn parent_and_name() {
        assert_eq!(parent("/"), None);
        assert_eq!(parent("/a"), Some("/".into()));
        assert_eq!(parent("/a/b"), Some("/a".into()));
        assert_eq!(parent("/a/b/"), Some("/a".into()));
        assert_eq!(file_name("/a/b"), "b");
        assert_eq!(file_name("/a/b/"), "b");
        assert_eq!(file_name("/"), "");
    }

    #[test]
    fn join_and_components() {
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(join("/a", "b"), "/a/b");
        assert_eq!(
            components("/a/b"),
            vec![
                ("/".to_string(), "/".to_string()),
                ("a".to_string(), "/a".to_string()),
                ("b".to_string(), "/a/b".to_string())
            ]
        );
        assert_eq!(components("/").len(), 1);
    }
}
