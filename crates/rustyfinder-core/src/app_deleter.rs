//! App Deleter: the files an application leaves in the Library folders.
//!
//! When an `.app` goes to the Trash, the app shows these so the user can
//! trash them too (app-deleter spec). Safety comes first, so the rules are
//! strict:
//! - **Confident** matches are named after the bundle identifier (exactly,
//!   or the identifier followed by a dot): `com.example.foo`,
//!   `com.example.foo.plist`, `com.example.foo.savedState`. They start
//!   checked.
//! - **Possible** matches start unchecked: a folder named exactly like the
//!   app, a group container of the same team that names the app, a launch
//!   agent that mentions the app, a crash report of its executable.
//! - Never listed: anything matched only by loose similarity (`Foobar` for
//!   `Foo`), items of Apple's own apps, other applications, and anything
//!   outside the Library locations below (so never user documents).
//!
//! Only the given Library roots are read, so tests pass a fake home.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// What identifies an application. The shell reads it from the bundle's
/// Info.plist and code signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppIdentity {
    /// `CFBundleIdentifier`, e.g. `com.example.foo`.
    pub bundle_id: String,
    /// The app's name as shown ("Foo"), without `.app`.
    pub name: String,
    /// `CFBundleExecutable`, used by crash reports. Often equals `name`.
    pub executable: String,
    /// Team identifier from the code signature, if it is signed by one.
    pub team_id: Option<String>,
    /// Where the bundle is, so launch agents that start it can be found.
    pub bundle_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Confidence {
    /// Named after something that merely might be the app's.
    Possible,
    /// Named after the bundle identifier.
    Confident,
}

/// One item the app left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
    pub path: String,
    pub confidence: Confidence,
    /// Why it is listed, for the window: "Named after com.example.foo".
    pub reason: String,
    /// Bytes on disk, folders included recursively (links not followed).
    pub size: u64,
    pub is_dir: bool,
    /// The folder holding it can't be written by this user, so moving it
    /// needs an administrator's password.
    pub needs_admin: bool,
}

/// A Library folder to search: `~/Library`, or `/Library` (the local
/// domain, `system: true`), which has fewer of the per-user locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRoot {
    pub path: PathBuf,
    pub system: bool,
}

impl LibraryRoot {
    pub fn user(home: impl AsRef<Path>) -> Self {
        Self { path: home.as_ref().join("Library"), system: false }
    }

    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), system: true }
    }
}

/// App names that are ordinary words: a folder with such a name says
/// nothing about which app made it, so it is not listed at all.
const GENERIC_NAMES: &[&str] = &[
    "app",
    "apps",
    "application",
    "applications",
    "backup",
    "backups",
    "cache",
    "caches",
    "calendar",
    "common",
    "config",
    "data",
    "default",
    "documents",
    "downloads",
    "editor",
    "files",
    "helper",
    "library",
    "logs",
    "mail",
    "music",
    "notes",
    "photos",
    "plugins",
    "preferences",
    "preview",
    "settings",
    "setup",
    "shared",
    "support",
    "system",
    "temp",
    "tmp",
    "tools",
    "uninstaller",
    "updater",
    "utilities",
    "viewer",
];

/// How a location's entries may match.
#[derive(Clone, Copy)]
struct Rules {
    /// Exact app name (case-insensitive) counts as a possible match.
    by_name: bool,
}

/// The locations searched, relative to a Library root, and whether they
/// exist in the local domain too.
const LOCATIONS: &[(&str, Rules, bool)] = &[
    ("Application Support", Rules { by_name: true }, true),
    ("Caches", Rules { by_name: true }, true),
    ("Preferences", Rules { by_name: false }, true),
    ("Preferences/ByHost", Rules { by_name: false }, false),
    ("Containers", Rules { by_name: false }, false),
    ("Saved Application State", Rules { by_name: false }, false),
    ("Logs", Rules { by_name: true }, true),
    ("HTTPStorages", Rules { by_name: false }, false),
    ("WebKit", Rules { by_name: false }, false),
    ("Cookies", Rules { by_name: false }, false),
    ("Application Scripts", Rules { by_name: false }, false),
];

/// Most bytes read from a launch agent or crash report when looking for
/// the bundle identifier in it.
const SNIFF_LIMIT: u64 = 256 * 1024;

/// Everything `app` left in `roots`, confident matches first, then by
/// path. Unreadable locations are skipped.
pub fn find_leftovers(app: &AppIdentity, roots: &[LibraryRoot]) -> Vec<Leftover> {
    let bundle_id = app.bundle_id.trim();
    // Without an identifier nothing can be matched safely; Apple's own
    // apps and their data are never offered.
    if bundle_id.is_empty() || is_apple_id(bundle_id) {
        return Vec::new();
    }
    let mut found: HashMap<PathBuf, (Confidence, String)> = HashMap::new();
    let mut add = |path: PathBuf, confidence: Confidence, reason: String| {
        let entry = found.entry(path).or_insert((confidence, reason.clone()));
        if confidence > entry.0 {
            *entry = (confidence, reason);
        }
    };
    let app_path = Path::new(&app.bundle_path);

    for root in roots {
        for (location, rules, in_local) in LOCATIONS {
            if root.system && !in_local {
                continue;
            }
            for (name, path) in children(&root.path.join(location)) {
                if is_apple_id(&name) || is_app_bundle(&name) || path.starts_with(app_path) {
                    continue;
                }
                if named_after(&name, bundle_id) {
                    add(path, Confidence::Confident, format!("Named after {bundle_id}"));
                } else if rules.by_name && name_matches(&name, &app.name) {
                    add(path, Confidence::Possible, format!("Same name as the app ({})", app.name));
                }
            }
        }
        group_containers(app, root, &mut add);
        launch_items(app, root, &mut add);
        crash_reports(app, root, &mut add);
    }

    let mut out: Vec<Leftover> = found
        .into_iter()
        .filter_map(|(path, (confidence, reason))| {
            let meta = fs::symlink_metadata(&path).ok()?;
            Some(Leftover {
                path: path.to_string_lossy().into_owned(),
                confidence,
                reason,
                size: disk_size(&path),
                is_dir: meta.is_dir(),
                needs_admin: !parent_writable(&path),
            })
        })
        .collect();
    out.sort_by(|a, b| b.confidence.cmp(&a.confidence).then_with(|| a.path.cmp(&b.path)));
    out
}

/// True if `name` is named after `bundle_id`: equal to it, or it followed
/// by a dot (`com.example.foo.plist`, `com.example.foo.helper`). Case is
/// ignored, as bundle identifiers are case-insensitive. `com.example.foobar`
/// does not count.
pub fn named_after(name: &str, bundle_id: &str) -> bool {
    let (name, id) = (name.to_lowercase(), bundle_id.to_lowercase());
    name == id || name.strip_prefix(&id).is_some_and(|rest| rest.starts_with('.'))
}

/// An exact, case-insensitive match of the app's name, when the name says
/// something (not a generic word). Never a partial match.
pub fn name_matches(entry: &str, app_name: &str) -> bool {
    let app = app_name.trim();
    !app.is_empty() && !is_generic(app) && entry.eq_ignore_ascii_case(app)
}

/// True for names too common to point at one app ("Helper", "Notes").
pub fn is_generic(name: &str) -> bool {
    GENERIC_NAMES.contains(&name.trim().to_lowercase().as_str())
}

fn is_apple_id(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with("com.apple.") || lower == "com.apple"
}

fn is_app_bundle(name: &str) -> bool {
    name.to_lowercase().ends_with(".app")
}

/// Group containers are shared between a team's apps, so one is offered
/// only when its identifier is clearly the app's: `TEAM.<bundle id>…` or
/// `group.<bundle id>…` (confident). Another container of the same team
/// that names the app as one of its parts is a possible match; anything
/// else of that team may serve its other apps and is left alone.
fn group_containers(app: &AppIdentity, root: &LibraryRoot, add: &mut impl FnMut(PathBuf, Confidence, String)) {
    if root.system {
        return;
    }
    let team = app.team_id.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let name_part = app.name.to_lowercase().replace(' ', "");
    for (name, path) in children(&root.path.join("Group Containers")) {
        if is_apple_id(&name) {
            continue;
        }
        if let Some(rest) = name.strip_prefix("group.") {
            if named_after(rest, &app.bundle_id) {
                add(path, Confidence::Confident, format!("App group of {}", app.bundle_id));
            }
            continue;
        }
        let Some(team) = team else { continue };
        let Some(rest) = name.strip_prefix(team).and_then(|r| r.strip_prefix('.')) else {
            continue;
        };
        if named_after(rest, &app.bundle_id) {
            add(path, Confidence::Confident, format!("App group of {}", app.bundle_id));
        } else if !is_generic(&name_part)
            && name_part.len() >= 3
            && rest.to_lowercase().split('.').any(|part| part == name_part)
        {
            add(path, Confidence::Possible, format!("Shared by apps of team {team}; names {}", app.name));
        }
    }
}

/// Launch agents (and, in the local domain, launch daemons). A property
/// list named after the bundle identifier is confident; one that only
/// mentions the identifier or the app's location is possible.
fn launch_items(app: &AppIdentity, root: &LibraryRoot, add: &mut impl FnMut(PathBuf, Confidence, String)) {
    let folders: &[&str] = if root.system { &["LaunchAgents", "LaunchDaemons"] } else { &["LaunchAgents"] };
    for folder in folders {
        for (name, path) in children(&root.path.join(folder)) {
            if is_apple_id(&name) || !name.to_lowercase().ends_with(".plist") {
                continue;
            }
            let stem = &name[..name.len() - ".plist".len()];
            if named_after(stem, &app.bundle_id) {
                add(path, Confidence::Confident, format!("Starts a part of {}", app.bundle_id));
                continue;
            }
            let text = sniff(&path);
            let mentions_path =
                !app.bundle_path.is_empty() && text.contains(&format!("{}/", app.bundle_path.trim_end_matches('/')));
            if contains_id(&text, &app.bundle_id) || mentions_path {
                add(path, Confidence::Possible, format!("Mentions {}", app.name));
            }
        }
    }
}

/// Crash reports of the app's executable (`Foo-2025-01-02-….ips`).
/// Confident when the report names the bundle identifier, else possible.
fn crash_reports(app: &AppIdentity, root: &LibraryRoot, add: &mut impl FnMut(PathBuf, Confidence, String)) {
    let exe = app.executable.trim();
    if exe.is_empty() || is_generic(exe) {
        return;
    }
    for (name, path) in children(&root.path.join("Logs/DiagnosticReports")) {
        let Some(rest) = name.strip_prefix(exe) else { continue };
        let mut chars = rest.chars();
        let dated = matches!(chars.next(), Some('-' | '_')) && chars.next().is_some_and(|c| c.is_ascii_digit());
        if !dated {
            continue;
        }
        if contains_id(&sniff(&path), &app.bundle_id) {
            add(path, Confidence::Confident, format!("Crash report of {}", app.bundle_id));
        } else {
            add(path, Confidence::Possible, format!("Crash report of “{exe}”"));
        }
    }
}

/// True if `text` names `id` as a whole identifier: not as the start of a
/// longer one (`com.example.foobar`).
fn contains_id(text: &str, id: &str) -> bool {
    let (text, id) = (text.to_lowercase(), id.to_lowercase());
    let mut from = 0;
    while let Some(at) = text[from..].find(&id) {
        let end = from + at + id.len();
        let before = text[..from + at].chars().next_back();
        let after = text[end..].chars().next();
        let part = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !part(before) && !part(after) {
            return true;
        }
        from = end;
    }
    false
}

/// The start of a file as text (binary property lists keep ASCII strings
/// as plain bytes, so identifiers are found in them too).
fn sniff(path: &Path) -> String {
    let mut bytes = Vec::new();
    if let Ok(file) = fs::File::open(path) {
        let _ = file.take(SNIFF_LIMIT).read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Names and paths of a folder's children; nothing if it can't be read.
fn children(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(reader) = fs::read_dir(dir) else { return Vec::new() };
    reader.flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).collect()
}

/// Allocated bytes of an item and everything in it, links not followed.
fn disk_size(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = fs::symlink_metadata(path) else { return 0 };
    let own = meta.blocks() * 512;
    if !meta.is_dir() {
        return own;
    }
    own + children(path).iter().map(|(_, child)| disk_size(child)).sum::<u64>()
}

/// True if this user may remove items from the folder holding `path`.
fn parent_writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Some(parent) = path.parent() else { return false };
    let Ok(c) = std::ffi::CString::new(parent.as_os_str().as_bytes()) else { return false };
    // SAFETY: a valid NUL-terminated path; access() only reads it.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn foo() -> AppIdentity {
        AppIdentity {
            bundle_id: "com.example.foo".into(),
            name: "Foo Writer".into(),
            executable: "FooWriter".into(),
            team_id: Some("ABCDE12345".into()),
            bundle_path: "/Applications/Foo Writer.app".into(),
        }
    }

    /// A fake home with a Library, never the real one.
    fn home() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn touch(root: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, bytes).unwrap();
        p
    }

    fn mkdir(root: &Path, rel: &str) -> PathBuf {
        let p = root.join(rel);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn found(app: &AppIdentity, home: &Path) -> Vec<(String, Confidence)> {
        let lib = home.join("Library");
        find_leftovers(app, &[LibraryRoot::user(home)])
            .into_iter()
            .map(|l| (Path::new(&l.path).strip_prefix(&lib).unwrap().to_string_lossy().into_owned(), l.confidence))
            .collect()
    }

    #[test]
    fn named_after_needs_the_whole_identifier() {
        assert!(named_after("com.example.foo", "com.example.foo"));
        assert!(named_after("com.example.foo.plist", "com.example.foo"));
        assert!(named_after("COM.Example.Foo.helper", "com.example.foo"));
        assert!(!named_after("com.example.foobar", "com.example.foo"));
        assert!(!named_after("com.example.fo", "com.example.foo"));
        assert!(!named_after("xcom.example.foo", "com.example.foo"));
    }

    #[test]
    fn name_matches_only_exactly_and_never_generic_words() {
        assert!(name_matches("Foo Writer", "Foo Writer"));
        assert!(name_matches("foo writer", "Foo Writer"));
        assert!(!name_matches("Foo Writer 2", "Foo Writer"));
        assert!(!name_matches("Foo", "Foo Writer"));
        assert!(!name_matches("Notes", "Notes"));
        assert!(!name_matches("Helper", "helper"));
        assert!(!name_matches("x", ""));
    }

    #[test]
    fn contains_id_wants_whole_identifiers() {
        assert!(contains_id("<string>com.example.foo</string>", "com.example.foo"));
        assert!(contains_id("label com.example.foo.agent", "com.example.foo"));
        assert!(!contains_id("<string>com.example.foobar</string>", "com.example.foo"));
        assert!(!contains_id("xcom.example.foo", "com.example.foo"));
    }

    /// AD-01: preferences and caches named after the bundle id are found,
    /// confident, with sizes.
    #[test]
    fn ad01_bundle_id_items_are_confident_with_sizes() {
        let h = home();
        let lib = h.path().join("Library");
        touch(&lib, "Preferences/com.example.foo.plist", b"prefs");
        touch(&lib, "Caches/com.example.foo/blob", &[0u8; 10_000]);
        let all = find_leftovers(&foo(), &[LibraryRoot::user(h.path())]);
        assert_eq!(all.len(), 2, "{all:?}");
        for item in &all {
            assert_eq!(item.confidence, Confidence::Confident);
            assert!(item.size > 0, "{item:?}");
            assert!(!item.needs_admin);
        }
        let cache = all.iter().find(|l| l.path.ends_with("Caches/com.example.foo")).unwrap();
        assert!(cache.is_dir);
        assert!(cache.size >= 10_000, "{cache:?}");
    }

    #[test]
    fn every_bundle_id_location_is_searched() {
        let h = home();
        let lib = h.path().join("Library");
        for dir in [
            "Application Support/com.example.foo",
            "Containers/com.example.foo",
            "Saved Application State/com.example.foo.savedState",
            "Logs/com.example.foo",
            "HTTPStorages/com.example.foo",
            "WebKit/com.example.foo",
            "Application Scripts/com.example.foo",
            "Caches/com.example.foo.ShipIt",
        ] {
            mkdir(&lib, dir);
        }
        touch(&lib, "Preferences/ByHost/com.example.foo.0F1E2D3C.plist", b"x");
        touch(&lib, "HTTPStorages/com.example.foo.binarycookies", b"x");
        touch(&lib, "Cookies/com.example.foo.binarycookies", b"x");
        touch(&lib, "LaunchAgents/com.example.foo.updater.plist", b"x");
        let names: Vec<_> = found(&foo(), h.path());
        assert_eq!(names.len(), 12, "{names:?}");
        assert!(names.iter().all(|(_, c)| *c == Confidence::Confident), "{names:?}");
    }

    /// Items of other apps, look-alikes and documents are never listed.
    #[test]
    fn look_alikes_and_other_apps_are_left_out() {
        let h = home();
        let lib = h.path().join("Library");
        mkdir(&lib, "Application Support/com.example.foobar");
        mkdir(&lib, "Application Support/Foo Writer Pro");
        mkdir(&lib, "Application Support/Foo");
        mkdir(&lib, "Application Support/com.apple.foo");
        mkdir(&lib, "Application Support/Foo Writer.app");
        touch(&lib, "Preferences/com.example.foobar.plist", b"x");
        touch(&lib, "Preferences/Foo Writer.plist", b"x");
        mkdir(&lib, "Containers/Foo Writer");
        touch(h.path(), "Documents/com.example.foo.txt", b"mine");
        mkdir(h.path(), "Library/Mobile Documents/com.example.foo");
        assert!(found(&foo(), h.path()).is_empty(), "{:?}", found(&foo(), h.path()));
    }

    /// AD-02 (and the owner's rule): an exact app-name match is only a
    /// possible match, unchecked, short names included.
    #[test]
    fn ad02_name_matches_are_possible_only() {
        let h = home();
        let lib = h.path().join("Library");
        mkdir(&lib, "Application Support/Go");
        let go = AppIdentity { bundle_id: "org.golang.go".into(), name: "Go".into(), ..foo() };
        assert_eq!(found(&go, h.path()), vec![("Application Support/Go".to_string(), Confidence::Possible)]);

        mkdir(&lib, "Caches/Foo Writer");
        mkdir(&lib, "Logs/foo writer");
        let names = found(&foo(), h.path());
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.iter().all(|(_, c)| *c == Confidence::Possible));
    }

    #[test]
    fn generic_app_names_match_nothing_by_name() {
        let h = home();
        let lib = h.path().join("Library");
        mkdir(&lib, "Application Support/Notes");
        let notes = AppIdentity { bundle_id: "com.example.notes".into(), name: "Notes".into(), ..foo() };
        assert!(found(&notes, h.path()).is_empty());
    }

    #[test]
    fn apple_apps_get_no_leftovers() {
        let h = home();
        let lib = h.path().join("Library");
        mkdir(&lib, "Containers/com.apple.Safari");
        mkdir(&lib, "Application Support/Safari");
        let safari = AppIdentity { bundle_id: "com.apple.Safari".into(), name: "Safari".into(), ..foo() };
        assert!(find_leftovers(&safari, &[LibraryRoot::user(h.path())]).is_empty());
    }

    #[test]
    fn empty_bundle_id_matches_nothing() {
        let h = home();
        mkdir(&h.path().join("Library"), "Application Support/Foo Writer");
        let anon = AppIdentity { bundle_id: " ".into(), ..foo() };
        assert!(find_leftovers(&anon, &[LibraryRoot::user(h.path())]).is_empty());
    }

    #[test]
    fn group_containers_only_when_clearly_the_apps() {
        let h = home();
        let lib = h.path().join("Library");
        mkdir(&lib, "Group Containers/ABCDE12345.com.example.foo");
        mkdir(&lib, "Group Containers/group.com.example.foo.shared");
        mkdir(&lib, "Group Containers/ABCDE12345.foowriter");
        // Same team, another product: shared, never offered.
        mkdir(&lib, "Group Containers/ABCDE12345.com.example.office");
        // Another team naming the same id: not this app's.
        mkdir(&lib, "Group Containers/ZZZZZ99999.com.example.foo");
        mkdir(&lib, "Group Containers/group.com.example.foobar");
        let mut names = found(&foo(), h.path());
        names.sort();
        assert_eq!(
            names,
            vec![
                ("Group Containers/ABCDE12345.com.example.foo".to_string(), Confidence::Confident),
                ("Group Containers/ABCDE12345.foowriter".to_string(), Confidence::Possible),
                ("Group Containers/group.com.example.foo.shared".to_string(), Confidence::Confident),
            ]
        );
        // Unsigned: team containers can't be attributed at all.
        let unsigned = AppIdentity { team_id: None, ..foo() };
        assert_eq!(found(&unsigned, h.path()).len(), 1);
    }

    #[test]
    fn launch_agents_that_mention_the_app_are_possible() {
        let h = home();
        let lib = h.path().join("Library");
        touch(&lib, "LaunchAgents/com.example.foo.plist", b"x");
        touch(&lib, "LaunchAgents/org.vendor.sync.plist", b"<string>com.example.foo</string>");
        touch(
            &lib,
            "LaunchAgents/org.vendor.start.plist",
            b"<string>/Applications/Foo Writer.app/Contents/MacOS/FooWriter</string>",
        );
        touch(&lib, "LaunchAgents/org.vendor.other.plist", b"<string>com.example.foobar</string>");
        touch(&lib, "LaunchAgents/com.apple.foo.plist", b"com.example.foo");
        let mut names = found(&foo(), h.path());
        names.sort();
        assert_eq!(
            names,
            vec![
                ("LaunchAgents/com.example.foo.plist".to_string(), Confidence::Confident),
                ("LaunchAgents/org.vendor.start.plist".to_string(), Confidence::Possible),
                ("LaunchAgents/org.vendor.sync.plist".to_string(), Confidence::Possible),
            ]
        );
    }

    #[test]
    fn crash_reports_of_the_executable() {
        let h = home();
        let lib = h.path().join("Library");
        touch(&lib, "Logs/DiagnosticReports/FooWriter-2025-01-02-101010.ips", br#"{"bundleID":"com.example.foo"}"#);
        touch(&lib, "Logs/DiagnosticReports/FooWriter_2025-01-03-101010_mac.crash", b"no id here");
        touch(&lib, "Logs/DiagnosticReports/FooWriterPro-2025-01-02-101010.ips", b"com.example.foo");
        touch(&lib, "Logs/DiagnosticReports/FooWriter-notes.txt", b"x");
        let mut names = found(&foo(), h.path());
        names.sort();
        assert_eq!(
            names,
            vec![
                ("Logs/DiagnosticReports/FooWriter-2025-01-02-101010.ips".to_string(), Confidence::Confident),
                ("Logs/DiagnosticReports/FooWriter_2025-01-03-101010_mac.crash".to_string(), Confidence::Possible),
            ]
        );
    }

    #[test]
    fn confident_wins_when_both_rules_match_and_order_is_confident_first() {
        let h = home();
        let lib = h.path().join("Library");
        mkdir(&lib, "Application Support/Foo Writer");
        mkdir(&lib, "Caches/com.example.foo");
        let app = AppIdentity { name: "com.example.foo".into(), ..foo() };
        let all = find_leftovers(&app, &[LibraryRoot::user(h.path())]);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].confidence, Confidence::Confident);

        let all = find_leftovers(&foo(), &[LibraryRoot::user(h.path())]);
        assert_eq!(all[0].confidence, Confidence::Confident);
        assert_eq!(all[1].confidence, Confidence::Possible);
    }

    #[test]
    fn local_domain_has_its_own_locations_and_flags_admin_items() {
        let h = home();
        let local = h.path().join("LocalLibrary");
        mkdir(&local, "Application Support/com.example.foo");
        touch(&local, "LaunchDaemons/com.example.foo.helper.plist", b"x");
        // Per-user locations are not looked at in the local domain.
        mkdir(&local, "Containers/com.example.foo");
        let locked = mkdir(&local, "Caches");
        mkdir(&local, "Caches/com.example.foo");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let all = find_leftovers(&foo(), &[LibraryRoot::local(&local)]);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        let mut rel: Vec<_> = all
            .iter()
            .map(|l| (Path::new(&l.path).strip_prefix(&local).unwrap().to_string_lossy().into_owned(), l.needs_admin))
            .collect();
        rel.sort();
        assert_eq!(
            rel,
            vec![
                ("Application Support/com.example.foo".to_string(), false),
                ("Caches/com.example.foo".to_string(), true),
                ("LaunchDaemons/com.example.foo.helper.plist".to_string(), false),
            ]
        );
    }

    #[test]
    fn symlinks_are_listed_but_never_followed_for_size() {
        let h = home();
        let lib = h.path().join("Library");
        let big = touch(h.path(), "elsewhere/big", &[1u8; 100_000]);
        mkdir(&lib, "Caches");
        std::os::unix::fs::symlink(big.parent().unwrap(), lib.join("Caches/com.example.foo")).unwrap();
        let all = find_leftovers(&foo(), &[LibraryRoot::user(h.path())]);
        assert_eq!(all.len(), 1);
        assert!(!all[0].is_dir);
        assert!(all[0].size < 100_000, "{:?}", all[0]);
    }

    #[test]
    fn missing_library_is_fine() {
        let h = home();
        assert!(find_leftovers(&foo(), &[LibraryRoot::user(h.path().join("nobody"))]).is_empty());
    }
}
