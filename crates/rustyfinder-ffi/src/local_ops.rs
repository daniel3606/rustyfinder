//! FFI for local file features: App Deleter, editors for Edit (⌘E), and
//! the small rules the shell asks about local items.

use rustyfinder_core::app_deleter::{self, AppIdentity, Confidence, LibraryRoot};
use rustyfinder_core::editors::{self, Editor};
use rustyfinder_core::ops::{Effect, Privileged};

use crate::RustyFinderApp;

/// An application, as the shell read it from its bundle and signature.
#[derive(Debug, Clone, uniffi::Record)]
pub struct AppIdentityRecord {
    pub bundle_id: String,
    pub name: String,
    pub executable: String,
    pub team_id: Option<String>,
    pub bundle_path: String,
}

/// One item an app left behind.
#[derive(Debug, Clone, uniffi::Record)]
pub struct LeftoverRecord {
    pub path: String,
    /// Named after the bundle identifier: starts checked. Otherwise a
    /// possible match that starts unchecked.
    pub confident: bool,
    pub reason: String,
    pub size: u64,
    pub is_dir: bool,
    /// Moving it needs an administrator's password.
    pub needs_admin: bool,
}

/// What `app` left in `home`'s Library and, if given, in `local_library`
/// (normally `/Library`). Reads the disk: call it off the main thread.
#[uniffi::export]
pub fn find_app_leftovers(app: AppIdentityRecord, home: String, local_library: Option<String>) -> Vec<LeftoverRecord> {
    let identity = AppIdentity {
        bundle_id: app.bundle_id,
        name: app.name,
        executable: app.executable,
        team_id: app.team_id,
        bundle_path: app.bundle_path,
    };
    let mut roots = vec![LibraryRoot::user(&home)];
    roots.extend(local_library.map(LibraryRoot::local));
    app_deleter::find_leftovers(&identity, &roots)
        .into_iter()
        .map(|l| LeftoverRecord {
            path: l.path,
            confident: l.confidence == Confidence::Confident,
            reason: l.reason,
            size: l.size,
            is_dir: l.is_dir,
            needs_admin: l.needs_admin,
        })
        .collect()
}

/// One item's tags before and after an edit.
#[derive(Debug, Clone, uniffi::Record)]
pub struct TagChange {
    pub path: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

#[uniffi::export]
impl RustyFinderApp {
    /// The shell changed tags itself: make the change one undo step
    /// ("Undo Tags") and log it. Items whose tags did not change are left out.
    pub fn record_tag_change(&self, window: u64, changes: Vec<TagChange>) {
        let effects: Vec<Effect> = changes
            .into_iter()
            .filter(|c| c.before != c.after)
            .map(|c| Effect::Tags { path: c.path, before: c.before, after: c.after })
            .collect();
        let summary = match effects.as_slice() {
            [Effect::Tags { path, .. }] => {
                format!("Changed the tags of “{}”", rustyfinder_core::paths::file_name(path))
            }
            many => format!("Changed the tags of {} items", many.len()),
        };
        if let Some(m) = self.activities.get() {
            m.record(window, "Tags", &summary, effects);
        }
    }

    /// The shell changed a Finder comment: one undo step ("Undo Comment").
    pub fn record_comment_change(&self, window: u64, path: String, before: String, after: String) {
        if before == after {
            return;
        }
        let summary = format!("Changed the comment of “{}”", rustyfinder_core::paths::file_name(&path));
        if let Some(m) = self.activities.get() {
            m.record(window, "Comment", &summary, vec![Effect::Comment { path, before, after }]);
        }
    }
}

/// An editor application and the extensions it edits (Settings → Editing).
#[derive(Debug, Clone, uniffi::Record)]
pub struct EditorRecord {
    /// Bundle path of the application.
    pub app: String,
    /// Lower-case, without dots.
    pub extensions: Vec<String>,
    /// Used for extensions no editor lists.
    pub is_default: bool,
}

impl From<Editor> for EditorRecord {
    fn from(e: Editor) -> Self {
        EditorRecord { app: e.app, extensions: e.extensions, is_default: e.is_default }
    }
}

/// "txt, .MD md" -> ["txt", "md"], as the extensions field is read.
#[uniffi::export]
pub fn parse_editor_extensions(text: String) -> Vec<String> {
    editors::parse_extensions(&text)
}

#[uniffi::export]
impl RustyFinderApp {
    pub fn editors(&self) -> Vec<EditorRecord> {
        self.store.editors().into_iter().map(Into::into).collect()
    }

    /// Save the editor list; returns it as stored (one default at most).
    pub fn set_editors(&self, editors: Vec<EditorRecord>) -> Vec<EditorRecord> {
        let list = editors
            .into_iter()
            .map(|e| Editor { app: e.app, extensions: e.extensions, is_default: e.is_default })
            .collect();
        self.store.set_editors(list).into_iter().map(Into::into).collect()
    }

    /// The editor application for a file name, or `None` for the system's
    /// default app.
    pub fn editor_for(&self, file_name: String) -> Option<String> {
        editors::editor_for(&self.store.editors(), &file_name).map(|e| e.app.clone())
    }
}

/// One step to run with administrator rights. `argv` is the command line
/// (empty for a Trash step, whose place in the Trash the shell picks).
#[derive(Debug, Clone, uniffi::Record)]
pub struct PrivilegedStep {
    /// "remove", "move", "copy", "mkdir" or "trash".
    pub kind: String,
    /// The item the step is about.
    pub path: String,
    pub argv: Vec<String>,
}

impl From<&Privileged> for PrivilegedStep {
    fn from(p: &Privileged) -> Self {
        let kind = match p {
            Privileged::Remove { .. } => "remove",
            Privileged::Move { .. } => "move",
            Privileged::Copy { .. } => "copy",
            Privileged::MakeDir { .. } => "mkdir",
            Privileged::Trash { .. } => "trash",
        };
        let argv = if matches!(p, Privileged::Trash { .. }) { Vec::new() } else { p.argv("") };
        PrivilegedStep { kind: kind.to_string(), path: p.path().to_string(), argv }
    }
}

/// The command that moves `path` to `trash_to` (a Trash step).
#[uniffi::export]
pub fn privileged_trash_argv(path: String, trash_to: String) -> Vec<String> {
    Privileged::Trash { path }.argv(&trash_to)
}

/// AppleScript that runs `argv` through the system's authorization prompt,
/// every argument quoted so no path can change what runs.
#[uniffi::export]
pub fn privileged_apple_script(argv: Vec<String>, prompt: String) -> String {
    rustyfinder_core::ops::privileged::apple_script(&argv, &prompt)
}

/// The same command as one shell line (shown to the user, logged).
#[uniffi::export]
pub fn privileged_shell_line(argv: Vec<String>) -> String {
    rustyfinder_core::ops::privileged::shell_line(&argv)
}
