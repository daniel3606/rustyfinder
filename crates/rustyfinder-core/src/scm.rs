//! Source control status of the items in a folder, from the system `git`.

use std::collections::HashMap;
use std::process::{Command, Stdio};

/// Ordered from least to most urgent: a folder shows the most urgent
/// status found inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    Ignored,
    Untracked,
    Added,
    Modified,
    Conflicted,
}

const GIT: &str = "/usr/bin/git";

fn git(dir: &str, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new(GIT)
        .arg("-C")
        .arg(dir)
        .args(args)
        // Looking must never take a lock a real git command is waiting for.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// What the two status letters of one porcelain entry mean.
fn classify(x: u8, y: u8) -> Option<Status> {
    match (x, y) {
        (b'?', b'?') => Some(Status::Untracked),
        (b'!', b'!') => Some(Status::Ignored),
        (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => Some(Status::Conflicted),
        (b'A', b' ') => Some(Status::Added),
        (b' ', b' ') => None,
        _ => Some(Status::Modified),
    }
}

/// Status of each direct child of `dir` that is not clean, by name.
/// `None` if `dir` is not inside a git working tree (or git is missing):
/// nothing is shown then, and nothing is an error.
pub fn status(dir: &str) -> Option<HashMap<String, Status>> {
    // Paths in the status output are relative to the repository's top;
    // this is `dir` relative to it ("" at the top, else "sub/dir/").
    let prefix = String::from_utf8(git(dir, &["rev-parse", "--show-prefix"])?).ok()?;
    let prefix = prefix.trim_end_matches('\n');
    let raw =
        git(dir, &["status", "--porcelain=v1", "-z", "--ignored=matching", "--untracked-files=normal", "--", "."])?;

    let mut found: HashMap<String, Status> = HashMap::new();
    let mut entries = raw.split(|b| *b == 0);
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let (x, y) = (entry[0], entry[1]);
        // A rename or copy is followed by its old path as a separate entry.
        if x == b'R' || x == b'C' {
            entries.next();
        }
        let Some(status) = classify(x, y) else { continue };
        let Ok(path) = std::str::from_utf8(&entry[3..]) else { continue };
        let Some(inside) = path.strip_prefix(prefix) else { continue };
        let Some(name) = inside.split('/').find(|part| !part.is_empty()) else { continue };
        let slot = found.entry(name.to_string()).or_insert(status);
        *slot = (*slot).max(status);
    }
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn run(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new(GIT)
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false"])
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn tl08_modified_and_untracked_items_are_told_apart() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        run(repo, &["init", "-q"]);
        fs::create_dir_all(repo.join("src/deep")).unwrap();
        fs::write(repo.join("tracked.txt"), "one").unwrap();
        fs::write(repo.join("clean.txt"), "same").unwrap();
        fs::write(repo.join("src/deep/code.rs"), "fn main() {}").unwrap();
        fs::write(repo.join(".gitignore"), "*.log\n").unwrap();
        run(repo, &["add", "."]);
        run(repo, &["commit", "-q", "-m", "first"]);

        fs::write(repo.join("tracked.txt"), "two").unwrap();
        fs::write(repo.join("new file.txt"), "new").unwrap();
        fs::write(repo.join("debug.log"), "noise").unwrap();
        fs::write(repo.join("staged.txt"), "staged").unwrap();
        run(repo, &["add", "staged.txt"]);
        fs::write(repo.join("src/deep/code.rs"), "fn main() { }").unwrap();

        let top = status(repo.to_str().unwrap()).unwrap();
        assert_eq!(top.get("tracked.txt"), Some(&Status::Modified));
        assert_eq!(top.get("new file.txt"), Some(&Status::Untracked));
        assert_eq!(top.get("debug.log"), Some(&Status::Ignored));
        assert_eq!(top.get("staged.txt"), Some(&Status::Added));
        assert_eq!(top.get("clean.txt"), None, "clean items are simply absent");
        // A folder shows what is inside it.
        assert_eq!(top.get("src"), Some(&Status::Modified));

        // Inside a subfolder, names are relative to that folder.
        let sub = status(repo.join("src").to_str().unwrap()).unwrap();
        assert_eq!(sub.get("deep"), Some(&Status::Modified));
        assert_eq!(sub.len(), 1);
    }

    #[test]
    fn outside_a_repository_there_is_nothing_and_no_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(status(tmp.path().to_str().unwrap()), None);
        assert_eq!(status("/nonexistent/folder"), None);
    }

    #[test]
    fn status_letters() {
        assert_eq!(classify(b'?', b'?'), Some(Status::Untracked));
        assert_eq!(classify(b'U', b'U'), Some(Status::Conflicted));
        assert_eq!(classify(b'A', b'A'), Some(Status::Conflicted));
        assert_eq!(classify(b'A', b' '), Some(Status::Added));
        assert_eq!(classify(b'A', b'M'), Some(Status::Modified));
        assert_eq!(classify(b' ', b'M'), Some(Status::Modified));
        assert_eq!(classify(b'D', b' '), Some(Status::Modified));
        assert_eq!(classify(b'R', b' '), Some(Status::Modified));
    }
}
