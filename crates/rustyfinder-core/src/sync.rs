//! Folder sync: compare two folder trees and plan what a sync would add,
//! update and delete. Planning reads only listings, never file contents.

use std::collections::BTreeMap;

use regex::RegexBuilder;
use serde::{Deserialize, Serialize};

use crate::entry::{Entry, EntryKind};
use crate::paths;
use crate::sort::{fold, natural_cmp};
use crate::vfs::Vfs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Direction {
    #[default]
    LeftToRight,
    RightToLeft,
    BothWays,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NameOp {
    Is,
    Contains,
    BeginsWith,
    EndsWith,
    Wildcard,
    Regex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KindClass {
    Folder,
    Image,
    Movie,
    Audio,
    Document,
    Archive,
    Application,
    Text,
}

/// One exclude rule. An item is left out if any rule matches it; a rule
/// that matches a folder leaves out everything inside it too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Exclude {
    Name(NameOp, String),
    /// Extensions without dots, e.g. ["log", "tmp"].
    Extension(Vec<String>),
    Kind(KindClass),
    /// Larger (or smaller) than this many bytes. Folders never match.
    Size {
        greater: bool,
        bytes: u64,
    },
    /// Modified before (or after) this moment, ms since the epoch.
    Modified {
        before: bool,
        ms: i64,
    },
    /// Carries (or, with `has` false, does not carry) this Finder tag.
    Tags {
        has: bool,
        tag: String,
    },
}

/// Modification times this close count as equal when one side is an FTP
/// server that lists times to the minute only (no MLSD).
pub const MINUTE_TOLERANCE_SECS: i64 = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncOptions {
    pub direction: Direction,
    /// Recurse into folders.
    pub subfolders: bool,
    pub hidden: bool,
    /// Copy items missing on the target side.
    pub add: bool,
    /// Do not create folders that would end up empty.
    pub skip_empty_folders: bool,
    /// Copy items that differ.
    pub update: bool,
    /// Delete target items that have no source (one-way only).
    pub delete_orphans: bool,
    /// Clock difference in minutes, and which side is ahead.
    pub offset_minutes: u32,
    pub right_is_ahead: bool,
    /// Modification times this close count as equal.
    pub tolerance_secs: i64,
    pub excludes: Vec<Exclude>,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            direction: Direction::LeftToRight,
            subfolders: true,
            hidden: false,
            add: true,
            skip_empty_folders: false,
            update: true,
            delete_orphans: false,
            offset_minutes: 0,
            right_is_ahead: false,
            // Covers file systems that store times in 2-second steps.
            tolerance_secs: 2,
            excludes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Add,
    Update,
    Delete,
}

/// What is known about one side of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemInfo {
    pub is_dir: bool,
    pub size: u64,
    pub modified_ms: Option<i64>,
}

/// One planned action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRow {
    /// Path relative to both roots, '/'-separated.
    pub path: String,
    pub action: Action,
    /// Side that changes: where the item is copied to, or deleted from.
    pub target: Side,
    pub is_dir: bool,
    /// Bytes this row moves (0 for folders and deletions of folders).
    pub size: u64,
    pub left: Option<ItemInfo>,
    pub right: Option<ItemInfo>,
    /// Both sides changed in a way the plan cannot settle by itself.
    pub conflict: bool,
    /// Rows the user (or a conflict) has switched off are skipped.
    pub checked: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub rows: Vec<PlanRow>,
    pub folders_analyzed: u64,
}

/// Count and total size of one category.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    pub count: u64,
    pub bytes: u64,
}

impl Plan {
    /// Footer figures: (all, added, updated, deleted).
    pub fn tallies(&self) -> (Tally, Tally, Tally, Tally) {
        let of = |action: Option<Action>| {
            self.rows
                .iter()
                .filter(|r| action.is_none_or(|a| r.action == a))
                .fold(Tally::default(), |t, r| Tally { count: t.count + 1, bytes: t.bytes + r.size })
        };
        (of(None), of(Some(Action::Add)), of(Some(Action::Update)), of(Some(Action::Delete)))
    }

    /// Synchronize is possible only with at least one row switched on.
    pub fn can_run(&self) -> bool {
        self.rows.iter().any(|r| r.checked)
    }
}

/// Two folder trees to compare.
pub struct Pair<'a> {
    pub left: &'a dyn Vfs,
    pub left_root: &'a str,
    pub right: &'a dyn Vfs,
    pub right_root: &'a str,
    /// Names differing only by case are the same item (true if either
    /// side ignores case).
    pub case_insensitive: bool,
}

/// Compare the two trees. `keep_going` is polled so a long analysis can be
/// abandoned (`None`); `progress` gets the number of folders analyzed.
pub fn analyze(
    pair: &Pair<'_>,
    options: &SyncOptions,
    keep_going: &dyn Fn() -> bool,
    progress: &dyn Fn(u64),
) -> Option<Plan> {
    let mut analysis = Analysis {
        pair,
        options,
        excludes: options.excludes.iter().map(Compiled::new).collect(),
        keep_going,
        progress,
        plan: Plan::default(),
        minute_times: false,
    };
    analysis.compare("")?;
    // Deletions run after every copy, so they are listed last.
    analysis.plan.rows.sort_by_key(|r| r.action == Action::Delete);
    Some(analysis.plan)
}

/// An exclude rule with its regex compiled once.
enum Compiled<'a> {
    Rule(&'a Exclude),
    Pattern(regex::Regex),
    /// An unusable rule (bad regex): matches nothing rather than crashing.
    Never,
}

impl<'a> Compiled<'a> {
    fn new(rule: &'a Exclude) -> Self {
        match rule {
            Exclude::Name(NameOp::Regex, pattern) => match RegexBuilder::new(pattern).case_insensitive(true).build() {
                Ok(re) => Compiled::Pattern(re),
                Err(_) => Compiled::Never,
            },
            other => Compiled::Rule(other),
        }
    }

    fn matches(&self, entry: &Entry) -> bool {
        let name = fold(&entry.name);
        match self {
            Compiled::Never => false,
            Compiled::Pattern(re) => re.is_match(&entry.name),
            Compiled::Rule(rule) => match rule {
                // An empty value means the rule is not filled in yet.
                Exclude::Name(_, value) if value.is_empty() => false,
                Exclude::Name(NameOp::Is, value) => name == fold(value),
                Exclude::Name(NameOp::Contains, value) => name.contains(&fold(value)),
                Exclude::Name(NameOp::BeginsWith, value) => name.starts_with(&fold(value)),
                Exclude::Name(NameOp::EndsWith, value) => name.ends_with(&fold(value)),
                Exclude::Name(NameOp::Wildcard, value) => {
                    crate::fuzzy::Pattern::Wildcard(fold(value)).matches(&entry.name)
                }
                Exclude::Name(NameOp::Regex, _) => false,
                Exclude::Extension(list) => {
                    let ext = entry.extension();
                    !ext.is_empty() && list.iter().any(|e| fold(e.trim().trim_start_matches('.')) == ext)
                }
                Exclude::Kind(class) => kind_matches(*class, entry),
                Exclude::Size { greater, bytes } => {
                    !entry.is_dir && if *greater { entry.size > *bytes } else { entry.size < *bytes }
                }
                Exclude::Modified { before, ms } => {
                    entry.modified_ms.is_some_and(|m| if *before { m < *ms } else { m > *ms })
                }
                Exclude::Tags { tag, .. } if tag.trim().is_empty() => false,
                Exclude::Tags { has, tag } => crate::tags::has(&entry.ext.tags, tag.trim()) == *has,
            },
        }
    }
}

pub(crate) fn kind_matches(class: KindClass, entry: &Entry) -> bool {
    if class == KindClass::Folder {
        return entry.is_dir;
    }
    if entry.is_dir {
        // A bundle is a folder on disk but an application to the user.
        return class == KindClass::Application && entry.name.to_lowercase().ends_with(".app");
    }
    let ext = entry.extension();
    let any = |list: &[&str]| list.contains(&ext.as_str());
    match class {
        KindClass::Folder => false,
        KindClass::Image => any(&[
            "jpg", "jpeg", "png", "gif", "heic", "tiff", "tif", "bmp", "webp", "svg", "raw", "cr2", "nef", "psd", "ico",
        ]),
        KindClass::Movie => any(&["mov", "mp4", "m4v", "avi", "mkv", "webm", "mpg", "mpeg", "wmv", "flv"]),
        KindClass::Audio => any(&["mp3", "m4a", "aac", "wav", "aiff", "aif", "flac", "ogg", "opus", "wma"]),
        KindClass::Document => any(&[
            "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "pages", "numbers", "key", "rtf", "odt", "ods", "odp",
            "epub",
        ]),
        KindClass::Archive => any(&["zip", "tar", "gz", "tgz", "bz2", "xz", "7z", "rar", "dmg", "iso", "zst"]),
        KindClass::Application => any(&["app", "exe", "pkg"]),
        KindClass::Text => any(&[
            "txt", "md", "markdown", "log", "csv", "json", "xml", "yaml", "yml", "toml", "ini", "html", "css", "js",
            "ts", "rs", "swift", "py", "sh", "c", "h",
        ]),
    }
}

struct Analysis<'a> {
    pair: &'a Pair<'a>,
    options: &'a SyncOptions,
    excludes: Vec<Compiled<'a>>,
    keep_going: &'a dyn Fn() -> bool,
    progress: &'a dyn Fn(u64),
    plan: Plan,
    /// The folder being compared has times to the minute only on one side.
    minute_times: bool,
}

/// True for an FTP server's listing whose times are whole minutes: what
/// a server without MLSD gives (its LIST shows no seconds).
fn minute_times(vfs: &dyn Vfs, listing: &BTreeMap<String, Entry>) -> bool {
    let mut times = listing.values().filter_map(|e| e.modified_ms).peekable();
    vfs.id().starts_with("ftp") && times.peek().is_some() && times.all(|ms| ms.rem_euclid(60_000) == 0)
}

fn info(entry: &Entry) -> ItemInfo {
    ItemInfo {
        is_dir: entry.kind == EntryKind::Directory,
        size: if entry.kind == EntryKind::File { entry.size } else { 0 },
        modified_ms: entry.modified_ms,
    }
}

impl Analysis<'_> {
    fn skipped(&self, entry: &Entry) -> bool {
        (entry.is_hidden && !self.options.hidden) || self.excludes.iter().any(|rule| rule.matches(entry))
    }

    /// Entries of `rel` on one side, by matching key, minus what the
    /// filters leave out. A folder that cannot be listed counts as empty.
    fn listing(&self, vfs: &dyn Vfs, root: &str, rel: &str) -> BTreeMap<String, Entry> {
        let dir = if rel.is_empty() { root.to_string() } else { paths::join(root, rel) };
        let mut entries = vfs.list(&dir).unwrap_or_default();
        // Tags cost a system call per item, so they are read only when a
        // rule needs them.
        if self.options.excludes.iter().any(|e| matches!(e, Exclude::Tags { .. })) {
            let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
            if let Some(found) = vfs.added_and_tags(&dir, &names) {
                let tags: Vec<Vec<String>> = found.into_iter().map(|(_, tags)| tags).collect();
                for (entry, tags) in entries.iter_mut().zip(tags) {
                    entry.ext.tags = tags;
                }
            }
        }
        entries
            .into_iter()
            .filter(|e| !self.skipped(e))
            .map(|e| {
                let key = if self.pair.case_insensitive { fold(&e.name) } else { e.name.clone() };
                (key, e)
            })
            .collect()
    }

    /// Modification time with the clock offset removed from the side that
    /// runs ahead, so both sides are on the same clock.
    fn adjusted(&self, side: Side, ms: Option<i64>) -> Option<i64> {
        let ahead = if self.options.right_is_ahead { Side::Right } else { Side::Left };
        let offset = i64::from(self.options.offset_minutes) * 60_000;
        ms.map(|m| if side == ahead { m - offset } else { m })
    }

    fn push(
        &mut self,
        path: String,
        action: Action,
        target: Side,
        left: Option<&Entry>,
        right: Option<&Entry>,
        conflict: bool,
    ) {
        let source = match target {
            Side::Right => left,
            Side::Left => right,
        };
        // A copy moves the source's bytes; a deletion frees the target's.
        let moved = if action == Action::Delete {
            match target {
                Side::Left => left,
                Side::Right => right,
            }
        } else {
            source
        };
        self.plan.rows.push(PlanRow {
            is_dir: moved.is_some_and(|e| e.kind == EntryKind::Directory),
            size: moved.map_or(0, |e| info(e).size),
            path,
            action,
            target,
            left: left.map(info),
            right: right.map(info),
            conflict,
            checked: !conflict,
        });
    }

    fn compare(&mut self, rel: &str) -> Option<()> {
        if !(self.keep_going)() {
            return None;
        }
        let left = self.listing(self.pair.left, self.pair.left_root, rel);
        let right = self.listing(self.pair.right, self.pair.right_root, rel);
        // Each folder decides for itself (a subfolder compared in between
        // must not change it for the rest of this one).
        let minute = minute_times(self.pair.left, &left) || minute_times(self.pair.right, &right);
        self.plan.folders_analyzed += 1;
        (self.progress)(self.plan.folders_analyzed);

        let one_way_target = match self.options.direction {
            Direction::LeftToRight => Some(Side::Right),
            Direction::RightToLeft => Some(Side::Left),
            Direction::BothWays => None,
        };
        let keys: Vec<&String> = {
            let mut all: Vec<&String> = left.keys().chain(right.keys()).collect();
            all.sort();
            all.dedup();
            // Listed the way the panes list names: "file2" before "file10".
            all.sort_by(|a, b| natural_cmp(&fold(a), &fold(b)).then_with(|| a.cmp(b)));
            all
        };
        for key in keys {
            let (l, r) = (left.get(key), right.get(key));
            let name = l.or(r).map(|e| e.name.clone()).unwrap_or_default();
            let path = if rel.is_empty() { name } else { paths::join(rel, &name) };
            match (l, r) {
                (Some(l), Some(r)) => {
                    self.minute_times = minute;
                    self.both(&path, l, r, one_way_target)?
                }
                (Some(only), None) => self.only(&path, only, Side::Left, one_way_target)?,
                (None, Some(only)) => self.only(&path, only, Side::Right, one_way_target)?,
                (None, None) => {}
            }
        }
        Some(())
    }

    /// The item exists on both sides.
    fn both(&mut self, path: &str, l: &Entry, r: &Entry, one_way_target: Option<Side>) -> Option<()> {
        let (l_dir, r_dir) = (l.kind == EntryKind::Directory, r.kind == EntryKind::Directory);
        if l_dir && r_dir {
            // Folders are never "updated" themselves, only looked into.
            if self.options.subfolders {
                self.compare(path)?;
            }
            return Some(());
        }
        if !self.options.update {
            return Some(());
        }
        if l_dir != r_dir {
            // A file on one side, a folder on the other.
            match one_way_target {
                Some(target) => self.push(path.to_string(), Action::Update, target, Some(l), Some(r), false),
                None => self.push(path.to_string(), Action::Update, Side::Right, Some(l), Some(r), true),
            }
            return Some(());
        }
        // Links are compared as links: by where they point, never followed.
        let links = l.kind == EntryKind::Symlink || r.kind == EntryKind::Symlink;
        let (lt, rt) = (self.adjusted(Side::Left, l.modified_ms), self.adjusted(Side::Right, r.modified_ms));
        let tolerance = if self.minute_times {
            self.options.tolerance_secs.max(MINUTE_TOLERANCE_SECS)
        } else {
            self.options.tolerance_secs
        } * 1000;
        // A missing date (some servers) cannot tell two files apart.
        let same_time = match (lt, rt) {
            (Some(a), Some(b)) => (a - b).abs() <= tolerance,
            _ => true,
        };
        let same =
            if links { l.kind == r.kind && l.link_target == r.link_target } else { l.size == r.size && same_time };
        if same {
            return Some(());
        }
        match one_way_target {
            // One-way: the source wins, even when it is the older one.
            Some(target) => self.push(path.to_string(), Action::Update, target, Some(l), Some(r), false),
            None if same_time => {
                // Same moment, different content: nobody can say which is right.
                self.push(path.to_string(), Action::Update, Side::Right, Some(l), Some(r), true);
            }
            None => {
                // Both ways: the newer file replaces the older.
                let target = if lt > rt { Side::Right } else { Side::Left };
                self.push(path.to_string(), Action::Update, target, Some(l), Some(r), false);
            }
        }
        Some(())
    }

    /// The item exists on one side only.
    fn only(&mut self, path: &str, entry: &Entry, side: Side, one_way_target: Option<Side>) -> Option<()> {
        let (left, right) = match side {
            Side::Left => (Some(entry), None),
            Side::Right => (None, Some(entry)),
        };
        match one_way_target {
            // It sits on the target side with no source: an orphan.
            Some(target) if target == side => {
                if self.options.delete_orphans {
                    self.push(path.to_string(), Action::Delete, side, left, right, false);
                }
                Some(())
            }
            // Missing on the target (or, both ways, on the other side).
            _ => {
                if self.options.add {
                    self.add_tree(path, entry, side)?;
                }
                Some(())
            }
        }
    }

    /// Plan adding `entry` (found on `side`) to the other side. Folders
    /// are walked so filters apply inside them. Returns whether anything
    /// was planned.
    fn add_tree(&mut self, path: &str, entry: &Entry, side: Side) -> Option<bool> {
        let (left, right) = match side {
            Side::Left => (Some(entry), None),
            Side::Right => (None, Some(entry)),
        };
        if entry.kind != EntryKind::Directory {
            self.push(path.to_string(), Action::Add, side.other(), left, right, false);
            return Some(true);
        }
        if !self.options.subfolders {
            return Some(false);
        }
        if !(self.keep_going)() {
            return None;
        }
        // The folder's own row goes first so it exists before its children.
        let at = self.plan.rows.len();
        self.push(path.to_string(), Action::Add, side.other(), left, right, false);
        let (vfs, root) = match side {
            Side::Left => (self.pair.left, self.pair.left_root),
            Side::Right => (self.pair.right, self.pair.right_root),
        };
        self.plan.folders_analyzed += 1;
        (self.progress)(self.plan.folders_analyzed);
        let mut any = false;
        for (_, child) in self.listing(vfs, root, path) {
            any |= self.add_tree(&paths::join(path, &child.name), &child, side)?;
        }
        if !any && self.options.skip_empty_folders {
            // Nothing would go inside: do not create it at all.
            self.plan.rows.truncate(at);
            return Some(false);
        }
        Some(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::LocalFs;
    use std::fs;

    const T: i64 = 1_700_000_000_000;

    struct Fx {
        tmp: tempfile::TempDir,
    }

    impl Fx {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            fs::create_dir(tmp.path().join("left")).unwrap();
            fs::create_dir(tmp.path().join("right")).unwrap();
            Self { tmp }
        }

        fn p(&self, rel: &str) -> String {
            self.tmp.path().join(rel).to_str().unwrap().to_string()
        }

        /// Write a file of `size` bytes with modification time `ms`.
        fn file(&self, rel: &str, size: usize, ms: i64) {
            let path = self.tmp.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, vec![b'x'; size]).unwrap();
            LocalFs.set_times(path.to_str().unwrap(), Some(ms), None).unwrap();
        }

        fn plan(&self, options: &SyncOptions) -> Plan {
            let (l, r) = (self.p("left"), self.p("right"));
            analyze(
                &Pair { left: &LocalFs, left_root: &l, right: &LocalFs, right_root: &r, case_insensitive: true },
                options,
                &|| true,
                &|_| {},
            )
            .unwrap()
        }
    }

    /// "action path -> side" for each row, sorted.
    fn describe(plan: &Plan) -> Vec<String> {
        let mut rows: Vec<String> = plan
            .rows
            .iter()
            .map(|r| {
                let side = if r.target == Side::Right { "right" } else { "left" };
                let flag = if r.conflict { " (conflict)" } else { "" };
                format!("{:?} {} -> {side}{flag}", r.action, r.path)
            })
            .collect();
        rows.sort();
        rows
    }

    fn both_ways() -> SyncOptions {
        SyncOptions { direction: Direction::BothWays, ..Default::default() }
    }

    /// Local files seen through another file system: an FTP server (by its
    /// id) and Finder tags given per name.
    struct Seen {
        id: &'static str,
        tags: Vec<(&'static str, &'static str)>,
    }

    impl Vfs for Seen {
        fn id(&self) -> String {
            self.id.into()
        }
        fn list(&self, path: &str) -> crate::error::VfsResult<Vec<Entry>> {
            LocalFs.list(path)
        }
        fn stat(&self, path: &str) -> crate::error::VfsResult<Entry> {
            LocalFs.stat(path)
        }
        fn added_and_tags(&self, _dir: &str, names: &[&str]) -> Option<Vec<(Option<i64>, Vec<String>)>> {
            Some(
                names
                    .iter()
                    .map(|n| {
                        let tags = self.tags.iter().filter(|(name, _)| name == n).map(|(_, t)| t.to_string()).collect();
                        (None, tags)
                    })
                    .collect(),
            )
        }
    }

    fn plan_with(f: &Fx, left: &dyn Vfs, right: &dyn Vfs, options: &SyncOptions) -> Plan {
        let (l, r) = (f.p("left"), f.p("right"));
        let pair = Pair { left, left_root: &l, right, right_root: &r, case_insensitive: true };
        analyze(&pair, options, &|| true, &|_| {}).unwrap()
    }

    #[test]
    fn an_ftp_listing_without_seconds_gets_a_minute_of_tolerance() {
        // §3: LIST gives minutes only, so 40 s apart is "the same time".
        let f = Fx::new();
        let minute = 1_700_000_040_000; // a whole minute
        f.file("left/a.txt", 10, minute + 40_000);
        f.file("right/a.txt", 10, minute);
        let ftp = Seen { id: "ftp://me@host", tags: Vec::new() };
        assert!(plan_with(&f, &LocalFs, &ftp, &SyncOptions::default()).rows.is_empty());
        // Beyond a minute they differ even there.
        f.file("left/a.txt", 10, minute + 61_000);
        assert_eq!(plan_with(&f, &LocalFs, &ftp, &SyncOptions::default()).rows.len(), 1);
        // A server with exact times (MLSD), or a local disk, keeps 2 s.
        f.file("left/a.txt", 10, minute + 40_000);
        f.file("right/a.txt", 10, minute + 1_000);
        assert_eq!(plan_with(&f, &LocalFs, &ftp, &SyncOptions::default()).rows.len(), 1);
        f.file("right/a.txt", 10, minute);
        assert_eq!(f.plan(&SyncOptions::default()).rows.len(), 1, "whole minutes on a local disk are exact");
    }

    #[test]
    fn a_tag_rule_leaves_out_tagged_or_untagged_items() {
        // §4: Tags has / doesn't have.
        let f = Fx::new();
        f.file("left/red.txt", 1, T);
        f.file("left/plain.txt", 1, T);
        let tagged = Seen { id: "local", tags: vec![("red.txt", "Red"), ("red.txt", "Work")] };
        let has = SyncOptions { excludes: vec![Exclude::Tags { has: true, tag: "red".into() }], ..Default::default() };
        assert_eq!(describe(&plan_with(&f, &tagged, &LocalFs, &has)), ["Add plain.txt -> right"]);
        let lacks =
            SyncOptions { excludes: vec![Exclude::Tags { has: false, tag: "Red".into() }], ..Default::default() };
        assert_eq!(describe(&plan_with(&f, &tagged, &LocalFs, &lacks)), ["Add red.txt -> right"]);
        // A rule with no tag typed yet does nothing.
        let empty = SyncOptions { excludes: vec![Exclude::Tags { has: true, tag: " ".into() }], ..Default::default() };
        assert_eq!(plan_with(&f, &tagged, &LocalFs, &empty).rows.len(), 2);
    }

    #[test]
    fn sy01_same_size_and_times_within_tolerance_are_equal() {
        let f = Fx::new();
        f.file("left/a.txt", 10, T);
        f.file("right/a.txt", 10, T + 1000);
        assert!(f.plan(&SyncOptions::default()).rows.is_empty());
        // Beyond the tolerance it differs.
        f.file("right/a.txt", 10, T + 5000);
        assert_eq!(describe(&f.plan(&SyncOptions::default())), vec!["Update a.txt -> right"]);
    }

    #[test]
    fn sy02_a_size_difference_is_an_update_towards_the_target() {
        let f = Fx::new();
        f.file("left/a.txt", 10, T);
        f.file("right/a.txt", 12, T);
        let plan = f.plan(&SyncOptions::default());
        assert_eq!(describe(&plan), vec!["Update a.txt -> right"]);
        assert_eq!(plan.rows[0].size, 10);
        assert_eq!(plan.rows[0].left.unwrap().size, 10);
        assert_eq!(plan.rows[0].right.unwrap().size, 12);
        // The other direction updates the left side instead.
        let reversed = SyncOptions { direction: Direction::RightToLeft, ..Default::default() };
        assert_eq!(describe(&f.plan(&reversed)), vec!["Update a.txt -> left"]);
        // One-way: the source wins even when it is the older file.
        f.file("left/a.txt", 10, T - 86_400_000);
        assert_eq!(describe(&f.plan(&SyncOptions::default())), vec!["Update a.txt -> right"]);
    }

    #[test]
    fn sy03_orphans_are_deleted_only_when_asked() {
        let f = Fx::new();
        f.file("left/a.txt", 1, T);
        f.file("right/a.txt", 1, T);
        f.file("right/z.txt", 7, T);
        assert!(f.plan(&SyncOptions::default()).rows.is_empty());
        let deleting = SyncOptions { delete_orphans: true, ..Default::default() };
        let plan = f.plan(&deleting);
        assert_eq!(describe(&plan), vec!["Delete z.txt -> right"]);
        assert_eq!(plan.tallies().3, Tally { count: 1, bytes: 7 });
    }

    #[test]
    fn sy04_both_ways_newer_wins_missing_items_are_added_nothing_is_deleted() {
        let f = Fx::new();
        f.file("left/n.txt", 5, T + 60_000);
        f.file("right/n.txt", 4, T);
        f.file("left/l.txt", 1, T);
        f.file("right/r.txt", 1, T);
        let options = SyncOptions {
            delete_orphans: true, // must be ignored both ways
            ..both_ways()
        };
        assert_eq!(
            describe(&f.plan(&options)),
            vec!["Add l.txt -> right", "Add r.txt -> left", "Update n.txt -> right"]
        );
        // The older side receives: flip the times and the arrow flips.
        f.file("left/n.txt", 5, T - 60_000);
        assert!(describe(&f.plan(&options)).contains(&"Update n.txt -> left".to_string()));
    }

    #[test]
    fn both_ways_same_time_different_size_is_a_conflict_left_unchecked() {
        let f = Fx::new();
        f.file("left/c.txt", 5, T);
        f.file("right/c.txt", 9, T);
        let plan = f.plan(&both_ways());
        assert_eq!(describe(&plan), vec!["Update c.txt -> right (conflict)"]);
        assert!(!plan.rows[0].checked);
        assert!(!plan.can_run(), "a plan with only unchecked rows cannot run");
    }

    #[test]
    fn sy05_a_clock_offset_is_removed_before_comparing() {
        let f = Fx::new();
        f.file("left/a.txt", 10, T);
        f.file("right/a.txt", 10, T + 3_600_000);
        // Without the correction the files look different.
        assert_eq!(f.plan(&SyncOptions::default()).rows.len(), 1);
        let corrected = SyncOptions { offset_minutes: 60, right_is_ahead: true, ..Default::default() };
        assert!(f.plan(&corrected).rows.is_empty());
        // Marking the wrong side makes it worse, not better.
        let wrong = SyncOptions { offset_minutes: 60, right_is_ahead: false, ..Default::default() };
        assert_eq!(f.plan(&wrong).rows.len(), 1);
    }

    #[test]
    fn sy06_excluded_items_never_appear_in_any_category() {
        let f = Fx::new();
        f.file("left/app.log", 3, T);
        f.file("left/keep.txt", 3, T);
        f.file("right/old.log", 3, T);
        f.file("left/sub/deep.LOG", 3, T);
        let options = SyncOptions {
            delete_orphans: true,
            excludes: vec![Exclude::Extension(vec!["log".into()])],
            ..Default::default()
        };
        assert_eq!(describe(&f.plan(&options)), vec!["Add keep.txt -> right", "Add sub -> right"]);
    }

    #[test]
    fn exclude_rules_of_every_kind() {
        let f = Fx::new();
        for (name, size) in
            [("notes.txt", 10), ("photo.JPG", 2000), ("Thumbs.db", 5), ("build/out.o", 1), ("big.iso", 5000)]
        {
            f.file(&format!("left/{name}"), size, T);
        }
        let plan_with = |rule: Exclude| describe(&f.plan(&SyncOptions { excludes: vec![rule], ..Default::default() }));
        let added = |rows: &[String], name: &str| rows.contains(&format!("Add {name} -> right"));

        let rows = plan_with(Exclude::Name(NameOp::Is, "thumbs.db".into()));
        assert!(!added(&rows, "Thumbs.db") && added(&rows, "notes.txt"));
        // A rule on a folder leaves out its whole subtree.
        let rows = plan_with(Exclude::Name(NameOp::Is, "build".into()));
        assert!(!rows.iter().any(|r| r.contains("build")));
        assert!(!added(&plan_with(Exclude::Name(NameOp::Wildcard, "*.j?g".into())), "photo.JPG"));
        assert!(!added(&plan_with(Exclude::Name(NameOp::Regex, r"^big\.".into())), "big.iso"));
        assert!(!added(&plan_with(Exclude::Name(NameOp::BeginsWith, "NOTE".into())), "notes.txt"));
        assert!(!added(&plan_with(Exclude::Kind(KindClass::Image)), "photo.JPG"));
        assert!(!plan_with(Exclude::Kind(KindClass::Folder)).iter().any(|r| r.contains("build")));
        let rows = plan_with(Exclude::Size { greater: true, bytes: 1000 });
        assert!(!added(&rows, "photo.JPG") && !added(&rows, "big.iso") && added(&rows, "notes.txt"));
        // Folders have no size, so a size rule never drops them.
        assert!(added(&rows, "build"));
        // A half-typed or broken rule matches nothing instead of crashing.
        assert_eq!(plan_with(Exclude::Name(NameOp::Contains, String::new())).len(), 6);
        assert_eq!(plan_with(Exclude::Name(NameOp::Regex, "(".into())).len(), 6);
    }

    #[test]
    fn sy10_folders_that_would_end_up_empty_are_not_created() {
        let f = Fx::new();
        f.file("left/only-logs/a.log", 1, T);
        f.file("left/mixed/a.log", 1, T);
        f.file("left/mixed/b.txt", 1, T);
        fs::create_dir(f.p("left/empty")).unwrap();
        let options = SyncOptions {
            skip_empty_folders: true,
            excludes: vec![Exclude::Extension(vec!["log".into()])],
            ..Default::default()
        };
        assert_eq!(describe(&f.plan(&options)), vec!["Add mixed -> right", "Add mixed/b.txt -> right"]);
        // With the option off, the empty folders are created too.
        let keeping = SyncOptions { skip_empty_folders: false, ..options };
        assert_eq!(f.plan(&keeping).rows.len(), 4);
    }

    #[test]
    fn subfolders_hidden_items_and_the_add_and_update_switches() {
        let f = Fx::new();
        f.file("left/top.txt", 1, T);
        f.file("left/sub/inner.txt", 1, T);
        f.file("left/.secret", 1, T);
        f.file("left/changed.txt", 2, T);
        f.file("right/changed.txt", 3, T);

        let top_only = SyncOptions { subfolders: false, ..Default::default() };
        assert_eq!(describe(&f.plan(&top_only)), vec!["Add top.txt -> right", "Update changed.txt -> right"]);

        let with_hidden = SyncOptions { hidden: true, ..Default::default() };
        assert!(describe(&f.plan(&with_hidden)).contains(&"Add .secret -> right".to_string()));
        assert!(!describe(&f.plan(&SyncOptions::default())).iter().any(|r| r.contains(".secret")));

        let no_add = SyncOptions { add: false, ..Default::default() };
        assert_eq!(describe(&f.plan(&no_add)), vec!["Update changed.txt -> right"]);
        let no_update = SyncOptions { update: false, ..Default::default() };
        assert!(!describe(&f.plan(&no_update)).iter().any(|r| r.starts_with("Update")));
    }

    #[test]
    fn a_file_against_a_folder_is_an_update_or_a_conflict_both_ways() {
        let f = Fx::new();
        f.file("left/thing", 3, T);
        f.file("right/thing/inside", 1, T);
        assert_eq!(describe(&f.plan(&SyncOptions::default())), vec!["Update thing -> right"]);
        let plan = f.plan(&both_ways());
        assert_eq!(describe(&plan), vec!["Update thing -> right (conflict)"]);
    }

    #[test]
    fn links_are_compared_by_where_they_point() {
        let f = Fx::new();
        f.file("left/target", 1, T);
        f.file("right/target", 1, T);
        std::os::unix::fs::symlink("target", f.p("left/link")).unwrap();
        std::os::unix::fs::symlink("target", f.p("right/link")).unwrap();
        assert!(f.plan(&SyncOptions::default()).rows.is_empty());
        fs::remove_file(f.p("right/link")).unwrap();
        std::os::unix::fs::symlink("elsewhere", f.p("right/link")).unwrap();
        assert_eq!(describe(&f.plan(&SyncOptions::default())), vec!["Update link -> right"]);
    }

    #[test]
    fn names_differing_only_by_case_are_one_item_and_deletions_come_last() {
        let f = Fx::new();
        f.file("left/Readme.md", 4, T);
        f.file("right/zz-orphan", 1, T);
        f.file("left/new.txt", 1, T);
        let options = SyncOptions { delete_orphans: true, ..Default::default() };
        let plan = f.plan(&options);
        assert_eq!(plan.rows.last().unwrap().action, Action::Delete, "deletes run after every copy");
        assert_eq!(plan.folders_analyzed, 1);
        let (all, added, updated, deleted) = plan.tallies();
        assert_eq!((all.count, added.count, updated.count, deleted.count), (3, 2, 0, 1));
        assert_eq!(added.bytes, 5);
    }

    #[test]
    fn analysis_can_be_abandoned_and_reports_progress() {
        let f = Fx::new();
        f.file("left/a/b/c.txt", 1, T);
        let (l, r) = (f.p("left"), f.p("right"));
        let pair = Pair { left: &LocalFs, left_root: &l, right: &LocalFs, right_root: &r, case_insensitive: true };
        assert!(analyze(&pair, &SyncOptions::default(), &|| false, &|_| {}).is_none());
        let seen = std::cell::Cell::new(0);
        let plan = analyze(&pair, &SyncOptions::default(), &|| true, &|n| seen.set(n)).unwrap();
        assert_eq!(seen.get(), plan.folders_analyzed);
        assert_eq!(plan.folders_analyzed, 3); // root, a, a/b
    }

    #[test]
    fn options_round_trip_for_synclets_and_tolerate_missing_fields() {
        let options = SyncOptions {
            direction: Direction::BothWays,
            offset_minutes: 90,
            excludes: vec![Exclude::Extension(vec!["log".into()]), Exclude::Size { greater: true, bytes: 1 << 20 }],
            ..Default::default()
        };
        let json = serde_json::to_string(&options).unwrap();
        assert_eq!(serde_json::from_str::<SyncOptions>(&json).unwrap(), options);
        let old: SyncOptions = serde_json::from_str(r#"{"direction":"RightToLeft"}"#).unwrap();
        assert_eq!(old.direction, Direction::RightToLeft);
        assert!(old.subfolders && old.add && old.update && !old.delete_orphans);
        assert_eq!(old.tolerance_secs, 2);
    }
}
