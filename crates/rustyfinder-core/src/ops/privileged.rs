//! Retrying a step with administrator rights (file-ops §1).
//!
//! RustyFinder has no privileged helper. When a step on this Mac fails
//! for lack of permission, the user may retry it as an administrator; the
//! shell then runs one plain command through the system's own
//! authorization prompt. This module says which command, quoted safely.

/// One step to retry as an administrator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Privileged {
    /// Delete a file, a symlink (never its target) or an empty folder.
    Remove { path: String, is_dir: bool },
    /// Rename or move within one volume, never replacing anything.
    Move { from: String, to: String },
    /// Copy one file or symlink to a new path, keeping its metadata.
    Copy { from: String, to: String },
    /// Create a folder.
    MakeDir { path: String },
    /// Move to the Trash. The shell picks the place in the Trash.
    Trash { path: String },
}

impl Privileged {
    /// The item the step is about, for messages.
    pub fn path(&self) -> &str {
        match self {
            Privileged::Remove { path, .. } | Privileged::MakeDir { path } | Privileged::Trash { path } => path,
            Privileged::Move { from, .. } | Privileged::Copy { from, .. } => from,
        }
    }

    /// The command line, as separate arguments. `trash_to` is where a
    /// `Trash` step puts the item. Every command refuses to overwrite.
    pub fn argv(&self, trash_to: &str) -> Vec<String> {
        let v = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        match self {
            Privileged::Remove { path, is_dir: false } => v(&["/bin/rm", "-f", "--", path]),
            Privileged::Remove { path, is_dir: true } => v(&["/bin/rmdir", "--", path]),
            Privileged::Move { from, to } => v(&["/bin/mv", "-n", "--", from, to]),
            Privileged::Copy { from, to } => v(&["/bin/cp", "-a", "-n", "--", from, to]),
            Privileged::MakeDir { path } => v(&["/bin/mkdir", "--", path]),
            Privileged::Trash { path } => v(&["/bin/mv", "-n", "--", path, trash_to]),
        }
    }
}

/// One shell command line from separate arguments: each is single-quoted,
/// so no character in a path (spaces, quotes, `$`, newlines) can change
/// what runs.
pub fn shell_line(argv: &[String]) -> String {
    argv.iter().map(|a| format!("'{}'", a.replace('\'', "'\\''"))).collect::<Vec<_>>().join(" ")
}

/// The AppleScript that runs `argv` through the system's authorization
/// prompt: `do shell script "…" with administrator privileges`.
pub fn apple_script(argv: &[String], prompt: &str) -> String {
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "do shell script \"{}\" with prompt \"{}\" with administrator privileges",
        quote(&shell_line(argv)),
        quote(prompt)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn commands_never_overwrite() {
        let p = |s: &str| s.to_string();
        assert_eq!(Privileged::Remove { path: p("/a"), is_dir: false }.argv(""), ["/bin/rm", "-f", "--", "/a"]);
        assert_eq!(Privileged::Remove { path: p("/d"), is_dir: true }.argv(""), ["/bin/rmdir", "--", "/d"]);
        assert_eq!(Privileged::Move { from: p("/a"), to: p("/b") }.argv(""), ["/bin/mv", "-n", "--", "/a", "/b"]);
        assert_eq!(Privileged::Copy { from: p("/a"), to: p("/b") }.argv(""), ["/bin/cp", "-a", "-n", "--", "/a", "/b"]);
        assert_eq!(Privileged::MakeDir { path: p("/d") }.argv(""), ["/bin/mkdir", "--", "/d"]);
        assert_eq!(Privileged::Trash { path: p("/a") }.argv("/T/a"), ["/bin/mv", "-n", "--", "/a", "/T/a"]);
        assert_eq!(Privileged::Copy { from: p("/a"), to: p("/b") }.path(), "/a");
    }

    /// Whatever a path contains, the shell sees exactly the arguments
    /// given: checked by letting a real shell echo them back.
    #[test]
    fn shell_line_survives_hostile_names() {
        let names = [
            "plain",
            "with space",
            "it's",
            "\"quoted\"",
            "$(touch /tmp/rustyfinder-pwned)",
            "back\\slash",
            "new\nline",
            "-rf",
            "`tick`",
            "é ü 漢字",
        ];
        let argv: Vec<String> = std::iter::once("printf".to_string())
            .chain(std::iter::once("%s\\0".to_string()))
            .chain(names.iter().map(|s| s.to_string()))
            .collect();
        let out = Command::new("/bin/sh").arg("-c").arg(shell_line(&argv)).output().unwrap();
        let echoed: Vec<String> =
            String::from_utf8(out.stdout).unwrap().split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect();
        assert_eq!(echoed, names);
        assert!(!std::path::Path::new("/tmp/rustyfinder-pwned").exists());
    }

    #[test]
    fn apple_script_escapes_for_its_string_literal() {
        let argv = vec!["/bin/rm".to_string(), "-f".into(), "--".into(), "/a \"b\" \\c".into()];
        let script = apple_script(&argv, "RustyFinder wants to delete “b”.");
        assert_eq!(
            script,
            r#"do shell script "'/bin/rm' '-f' '--' '/a \"b\" \\c'" with prompt "RustyFinder wants to delete “b”." with administrator privileges"#
        );
    }
}
