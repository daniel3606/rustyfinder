//! Tools, compare tools, archives, preview and rename additions to the
//! FFI surface. Kept out of lib.rs so that file stays easy to merge.

use crate::{RenameItem, RenamePreview, RenameRow, RustyFinderApp, ToolRecord};

/// The canonical form of a shortcut (`Shift+CMD+N` gives `shift+cmd+n`);
/// `Some("")` for none, `None` if the text is not a shortcut.
#[uniffi::export]
pub fn shortcut_normalize(text: String) -> Option<String> {
    rustyfinder_core::shortcuts::normalize(&text)
}

/// The tool (other than `except`) that already has this shortcut.
#[uniffi::export]
pub fn tool_shortcut_holder(tools: Vec<ToolRecord>, binding: String, except: u64) -> Option<u64> {
    let tools: Vec<rustyfinder_core::tools::Tool> = tools.into_iter().map(Into::into).collect();
    rustyfinder_core::tools::shortcut_holder(&tools, &binding, except)
}

/// One external compare tool, as Settings lists it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CompareToolInfo {
    pub key: String,
    pub name: String,
    /// The program to run; `None` when the tool is not installed.
    pub executable: Option<String>,
    /// What to install when it is missing.
    pub install_hint: String,
}

/// Every compare tool and whether it is installed. `filemerge_present`
/// says whether Xcode's FileMerge exists (the shell asks xcode-select).
#[uniffi::export]
pub fn compare_tools(home: String, filemerge_present: bool) -> Vec<CompareToolInfo> {
    use rustyfinder_core::compare_tools::ALL;
    let exists = |path: &str| {
        std::fs::metadata(path)
            .map(|m| m.is_file() && std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o111 != 0)
            .unwrap_or(false)
    };
    ALL.into_iter()
        .map(|tool| CompareToolInfo {
            key: tool.key().into(),
            name: tool.name().into(),
            executable: tool.locate(&home, &exists, filemerge_present),
            install_hint: tool.install_hint().into(),
        })
        .collect()
}

#[uniffi::export]
impl RustyFinderApp {
    /// The command line that opens a shell on an SFTP connection's server,
    /// in `path` there. `None` for any other kind of connection.
    pub fn ssh_shell_command(&self, connection: u64, path: String) -> Option<String> {
        let target = self.registry.ssh_target(connection)?;
        rustyfinder_core::tools::ssh_shell_command(&target, &path)
    }

    /// Whether permissions can be changed on this connection: the local
    /// disk, SFTP (setstat) and FTP (SITE CHMOD, where the server has it).
    /// Archives, WebDAV and S3 have no permissions to change.
    pub fn can_set_mode(&self, connection: u64) -> bool {
        if self.registry.origin(connection).is_some() {
            return false;
        }
        self.registry
            .get(connection)
            .is_some_and(|vfs| connection == rustyfinder_core::LOCAL || is_unix_like(&vfs.id()))
    }

    /// Copy `path` (a file or folder) on a connection to the local path
    /// `dest`, which must not exist, in the background. For previews,
    /// Quick Look, thumbnails and drags out of archives: no activity, no
    /// log, no undo. The listener hears progress and the outcome.
    pub fn fetch_copy(
        &self,
        connection: u64,
        path: String,
        dest: String,
        listener: std::sync::Arc<dyn FetchListener>,
    ) -> std::sync::Arc<FetchJob> {
        let job = std::sync::Arc::new(FetchJob { cancelled: std::sync::atomic::AtomicBool::new(false) });
        let worker = std::sync::Arc::clone(&job);
        let registry = std::sync::Arc::clone(&self.registry);
        std::thread::spawn(move || {
            let Some(vfs) = registry.get(connection) else {
                return listener.finished(Some("The connection is closed.".into()));
            };
            let total = rustyfinder_core::fetch::total_size(&*vfs, &path);
            let keep_going = || !worker.cancelled.load(std::sync::atomic::Ordering::SeqCst);
            let mut last = std::time::Instant::now();
            let result =
                rustyfinder_core::fetch::copy_out(&*vfs, &path, std::path::Path::new(&dest), &keep_going, &mut |n| {
                    // A few updates a second are plenty.
                    if last.elapsed() >= std::time::Duration::from_millis(100) {
                        last = std::time::Instant::now();
                        listener.progress(n, total);
                    }
                });
            listener.finished(result.err());
        });
        job
    }
}

/// SFTP and FTP keep Unix permissions.
fn is_unix_like(id: &str) -> bool {
    id.starts_with("sftp://") || id.starts_with("ftp://") || id.starts_with("ftps://")
}

/// Hears how a `fetch_copy` goes. Called on a worker thread.
#[uniffi::export(with_foreign)]
pub trait FetchListener: Send + Sync {
    fn progress(&self, bytes: u64, total: u64);
    /// `None` when the copy is complete.
    fn finished(&self, error: Option<String>);
}

/// A `fetch_copy` in progress.
#[derive(uniffi::Object)]
pub struct FetchJob {
    cancelled: std::sync::atomic::AtomicBool,
}

#[uniffi::export]
impl FetchJob {
    /// Stop; what was written is removed.
    pub fn cancel(&self) {
        self.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

// ------------------------------------------------------------ Multi-Rename

/// Metadata of one item for Multi-Rename's Add Date (Metadata) and Add
/// Metadata rules, read by the shell (Spotlight, ImageIO).
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct RenameMetadata {
    /// Capture or content-creation date, ms since the epoch.
    pub date_ms: Option<i64>,
    /// Values by key ("Dimensions", "Camera", "Artist"…).
    pub values: std::collections::HashMap<String, String>,
}

#[uniffi::export]
impl RustyFinderApp {
    /// Multi-Rename preview with metadata for each item (same order as
    /// `items`; missing entries mean "nothing known").
    pub fn rename_preview_with_metadata(
        &self,
        connection: u64,
        items: Vec<RenameItem>,
        metadata: Vec<RenameMetadata>,
        rules_json: String,
    ) -> RenamePreview {
        rename_preview(self, connection, items, metadata, rules_json)
    }
}

pub(crate) fn rename_preview(
    app: &RustyFinderApp,
    connection: u64,
    items: Vec<RenameItem>,
    metadata: Vec<RenameMetadata>,
    rules_json: String,
) -> RenamePreview {
    use rustyfinder_core::{paths, rename};
    let failed = |message: String| RenamePreview {
        rows: Vec::new(),
        changed: 0,
        problems: 0,
        can_rename: false,
        summary: String::new(),
        broken_rules: Vec::new(),
        error: Some(message),
    };
    let rules: Vec<rename::Rule> = match serde_json::from_str(&rules_json) {
        Ok(rules) => rules,
        Err(e) => return failed(format!("The rules can’t be read: {e}")),
    };
    let Some(vfs) = app.registry.get(connection) else {
        return failed("The connection is closed.".into());
    };
    let mut metadata = metadata.into_iter();
    let items: Vec<rename::Item> = items
        .into_iter()
        .map(|i| {
            let meta = metadata.next().unwrap_or_default();
            rename::Item {
                path: i.path,
                is_dir: i.is_dir,
                created_ms: i.created_ms,
                modified_ms: i.modified_ms,
                metadata_date_ms: meta.date_ms,
                metadata: meta.values,
            }
        })
        .collect();
    let local = connection == rustyfinder_core::LOCAL;
    let exists = |dir: &str, name: &str| vfs.stat(&paths::join(dir, name)).is_ok();
    let preview = rename::preview(
        &items,
        &rules,
        &rename::Context {
            now_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
            local,
            // The local disk ignores case; servers usually do not.
            case_insensitive: local,
            exists: &exists,
        },
    );
    RenamePreview {
        can_rename: preview.can_rename(),
        summary: preview.summary(),
        changed: preview.changed as u32,
        problems: preview.problems as u32,
        broken_rules: preview.broken_rules.iter().map(|i| *i as u32).collect(),
        rows: preview
            .rows
            .into_iter()
            .map(|r| RenameRow {
                original: r.original,
                new_name: r.new_name,
                changed: r.changed,
                invalid: r.invalid,
                conflict: r.conflict,
                note: r.note,
            })
            .collect(),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str) -> RenameItem {
        RenameItem { path: path.into(), is_dir: false, created_ms: None, modified_ms: Some(0) }
    }

    struct Done(std::sync::mpsc::Sender<Option<String>>);

    impl FetchListener for Done {
        fn progress(&self, _bytes: u64, _total: u64) {}
        fn finished(&self, error: Option<String>) {
            let _ = self.0.send(error);
        }
    }

    #[test]
    fn permissions_change_on_the_disk_and_servers_but_not_in_archives_and_fetch_copies_out() {
        let dir = tempfile::tempdir().unwrap();
        let app = RustyFinderApp::new(dir.path().to_str().unwrap().into(), "/".into());
        assert!(app.can_set_mode(0));
        assert!(!app.can_set_mode(99), "a closed connection");
        assert!(is_unix_like("sftp://me@host") && is_unix_like("ftp://host"));
        assert!(!is_unix_like("https://dav.example.org/") && !is_unix_like("s3://bucket"));

        // An archive: read-only, and fetch_copy brings an entry out.
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("a.txt"), "inside").unwrap();
        let zipped =
            std::process::Command::new("/usr/bin/zip").args(["-q", "../a.zip", "a.txt"]).current_dir(&src).status();
        assert!(zipped.unwrap().success());
        let cache = dir.path().join("cache");
        std::fs::create_dir(&cache).unwrap();
        let opened =
            rustyfinder_core::archive::open(dir.path().join("a.zip").to_str().unwrap(), None, cache.to_str().unwrap())
                .unwrap();
        let origin = rustyfinder_core::Location { conn: 0, path: dir.path().join("a.zip").to_str().unwrap().into() };
        let id = app.registry.add_archive(opened.vfs, "a.zip", origin);
        assert!(!app.can_set_mode(id));
        let (tx, rx) = std::sync::mpsc::channel();
        let dest = dir.path().join("out.txt");
        app.fetch_copy(id, "/a.txt".into(), dest.to_str().unwrap().into(), std::sync::Arc::new(Done(tx)));
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap(), None);
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "inside");
    }

    #[test]
    fn rules_in_the_shape_the_window_writes_them() {
        // The rule JSON the Multi-Rename window builds: a custom date, a
        // metadata date, an index position, a metadata value and a rule
        // switched off.
        let dir = tempfile::tempdir().unwrap();
        let app = RustyFinderApp::new(dir.path().to_str().unwrap().into(), "/".into());
        let d = dir.path().to_str().unwrap();
        let items = vec![item(&format!("{d}/abcd.jpg")), item(&format!("{d}/plain.jpg"))];
        let mut camera = std::collections::HashMap::new();
        camera.insert("Camera".to_string(), "X100".to_string());
        // 2024-12-30 12:00 UTC: the 30th in any time zone but the farthest east.
        let when: i64 = 1_735_560_000_000;
        let metadata = vec![RenameMetadata { date_ms: Some(when), values: camera }, RenameMetadata::default()];
        let rule = |kind: String, enabled: bool| {
            format!(r#"{{"kind":{kind},"target":"Name","enabled":{enabled},"files":true,"folders":true}}"#)
        };
        let rules = [
            rule(r#"{"AddMetadata":{"key":"Camera","position":{"Index":{"index":2,"from_end":false}}}}"#.into(), true),
            rule(
                r#"{"AddDate":{"source":"Metadata","format":"yyyy-MM-dd","position":"Beginning","separator":" "}}"#
                    .into(),
                true,
            ),
            rule(
                format!(
                    r#"{{"AddDate":{{"source":{{"Custom":{when}}},"format":"yyyy","position":"End","separator":"_"}}}}"#
                ),
                true,
            ),
            rule(r#"{"AddText":{"text":"OFF","position":"End"}}"#.into(), false),
        ];
        let preview = app.rename_preview_with_metadata(0, items, metadata, format!("[{}]", rules.join(",")));
        assert_eq!(preview.error, None);
        let names: Vec<&str> = preview.rows.iter().map(|r| r.new_name.as_str()).collect();
        assert_eq!(names, ["2024-12-30 abX100cd_2024.jpg", "plain_2024.jpg"]);
        assert_eq!(preview.rows[1].note, "No date found for this item");
    }
}
