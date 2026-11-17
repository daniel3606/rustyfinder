//! An immutable, sorted, filtered view of one directory.
//!
//! The UI never holds the rows itself. It keeps a `Snapshot` and asks for
//! rows by index, so a 100k-entry folder costs the UI nothing until rows
//! scroll into view.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

use crate::entry::Entry;
use crate::group::{self, Group, GroupKey, Slot};
use crate::sort::{fold, natural_cmp, SortKey, SortSpec};
use crate::vfs::Vfs;

/// How a tab lays its items out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Layout {
    Icons,
    #[default]
    List,
    Columns,
}

/// How a directory should be presented.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewSpec {
    pub sort: SortSpec,
    /// Group by (None for a plain list).
    pub group: GroupKey,
    /// With folders on top, keep the folder block in name order whatever
    /// the files are sorted by.
    pub folders_by_name: bool,
    pub layout: Layout,
    pub show_hidden: bool,
    /// Case-insensitive substring filter on the name; empty means no filter.
    /// Not persisted: a restored tab starts unfiltered.
    #[serde(skip)]
    pub filter: String,
}

/// Raw listing plus folded names, shared between re-sorted views.
struct Listing {
    dir: String,
    entries: Vec<Entry>,
    /// `fold(name)` for each entry, same indices as `entries`.
    keys: Vec<String>,
    /// Lower-cased extension for each entry.
    exts: Vec<String>,
    /// Where the entries came from, to read the costly details on demand.
    source: Option<Arc<dyn Vfs>>,
    /// Date added and tags, each read the first time a view needs it.
    added: OnceLock<Vec<Option<i64>>>,
    tags: OnceLock<Vec<Vec<String>>>,
}

impl std::fmt::Debug for Listing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Listing").field("dir", &self.dir).field("entries", &self.entries.len()).finish()
    }
}

impl Listing {
    fn build(
        dir: String,
        entries: Vec<Entry>,
        key_name: impl Fn(&Entry) -> &str,
        source: Option<Arc<dyn Vfs>>,
    ) -> Self {
        let keys = entries.iter().map(|e| fold(key_name(e))).collect();
        let exts = entries.iter().map(Entry::extension).collect();
        Listing { dir, entries, keys, exts, source, added: OnceLock::new(), tags: OnceLock::new() }
    }

    fn names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.name.as_str()).collect()
    }

    /// Date added per entry: what the entry carries, else what the source
    /// reports (read once, on first use).
    fn added(&self) -> &[Option<i64>] {
        self.added.get_or_init(|| {
            let read = self.source.as_ref().and_then(|vfs| vfs.added_dates(&self.dir, &self.names()));
            self.entries
                .iter()
                .enumerate()
                .map(|(i, e)| e.ext.added_ms.or_else(|| read.as_ref().and_then(|r| r[i])))
                .collect()
        })
    }

    /// Tags per entry, the same way.
    fn tags(&self) -> &[Vec<String>] {
        self.tags.get_or_init(|| {
            let read = self.source.as_ref().and_then(|vfs| vfs.tag_lists(&self.dir, &self.names()));
            self.entries
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    if e.ext.tags.is_empty() {
                        read.as_ref().map(|r| r[i].clone()).unwrap_or_default()
                    } else {
                        e.ext.tags.clone()
                    }
                })
                .collect()
        })
    }
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    path: String,
    listing: Arc<Listing>,
    /// Indices into the listing, in display order.
    order: Vec<u32>,
    /// Group headers, in row order; empty when not grouped.
    groups: Vec<Group>,
    view: ViewSpec,
}

impl Snapshot {
    pub fn new(path: impl Into<String>, entries: Vec<Entry>, view: ViewSpec) -> Self {
        Self::listed(path, entries, view, None)
    }

    /// A folder listed from `source`, which is asked for date added and
    /// tags if a view sorts by them.
    pub fn listed(path: impl Into<String>, entries: Vec<Entry>, view: ViewSpec, source: Option<Arc<dyn Vfs>>) -> Self {
        let path = path.into();
        let listing = Arc::new(Listing::build(path.clone(), entries, |e| &e.name, source));
        let (order, groups) = build_order(&listing, &view);
        Self { path, listing, order, groups, view }
    }

    /// Search results below `path`: each entry's name is a path relative
    /// to it. Sorting, filtering and type-to-select go by the last part of
    /// that path, the name the user sees.
    pub fn of_results(path: impl Into<String>, entries: Vec<Entry>, view: ViewSpec) -> Self {
        let path = path.into();
        let listing = Arc::new(Listing::build(path.clone(), entries, |e| crate::paths::file_name(&e.name), None));
        let (order, groups) = build_order(&listing, &view);
        Self { path, listing, order, groups, view }
    }

    /// Same directory contents under a different sort/filter. Cheap: the
    /// entries are shared, only the index order is rebuilt.
    pub fn with_view(&self, view: ViewSpec) -> Self {
        let (order, groups) = build_order(&self.listing, &view);
        Self { path: self.path.clone(), listing: Arc::clone(&self.listing), order, groups, view }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn view(&self) -> &ViewSpec {
        &self.view
    }

    /// Group headers with the row each starts at; empty when not grouped.
    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    /// Number of visible rows.
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Number of entries in the directory, including filtered-out ones.
    pub fn total_len(&self) -> usize {
        self.listing.entries.len()
    }

    pub fn get(&self, row: usize) -> Option<&Entry> {
        let idx = *self.order.get(row)? as usize;
        self.listing.entries.get(idx)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Entry> + '_ {
        self.order.iter().map(|i| &self.listing.entries[*i as usize])
    }

    /// Row of the entry with exactly this name.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.order.iter().position(|i| self.listing.entries[*i as usize].name == name)
    }

    /// Rows of all entries whose names are in `names`, ascending. One pass,
    /// so restoring a large selection after a re-sort stays fast.
    pub fn indices_of(&self, names: &[String]) -> Vec<usize> {
        let wanted: HashSet<&str> = names.iter().map(String::as_str).collect();
        self.order
            .iter()
            .enumerate()
            .filter(|(_, i)| wanted.contains(self.listing.entries[**i as usize].name.as_str()))
            .map(|(row, _)| row)
            .collect()
    }

    /// Combined size in bytes of the given rows (folders count as 0).
    pub fn total_size(&self, rows: &[u32]) -> u64 {
        rows.iter().filter_map(|r| self.get(*r as usize)).map(|e| e.size).sum()
    }

    /// Type-to-select: first row at or after `start` (wrapping) whose name
    /// starts with `prefix`, ignoring case and Unicode form.
    pub fn find_prefix(&self, prefix: &str, start: usize) -> Option<usize> {
        let needle = fold(prefix);
        if needle.is_empty() || self.order.is_empty() {
            return None;
        }
        let n = self.order.len();
        (0..n)
            .map(|off| (start + off) % n)
            .find(|row| self.listing.keys[self.order[*row] as usize].starts_with(&needle))
    }
}

/// Which group each entry belongs to, or nothing when not grouped.
fn group_slots(listing: &Listing, view: &ViewSpec) -> Vec<Slot> {
    let now =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let offset_now = group::local_offset(now);
    let date =
        |ms: Option<i64>| group::date_slot(ms, now, ms.map(group::local_offset).unwrap_or(offset_now), offset_now);
    let entries = &listing.entries;
    match view.group {
        GroupKey::None => Vec::new(),
        GroupKey::Kind => entries
            .iter()
            .enumerate()
            .map(|(i, e)| group::kind_slot(&listing.exts[i], e.is_dir, view.sort.folders_first))
            .collect(),
        GroupKey::Size => entries.iter().map(|e| group::size_slot(e.size, e.is_dir)).collect(),
        GroupKey::Modified => entries.iter().map(|e| date(e.modified_ms)).collect(),
        GroupKey::Created => entries.iter().map(|e| date(e.created_ms)).collect(),
        GroupKey::Added => listing.added().iter().map(|added| date(*added)).collect(),
        GroupKey::Tags => listing.tags().iter().map(|tags| group::tag_slot(tags.first().map(String::as_str))).collect(),
    }
}

/// The header title of a group, from its first entry.
fn group_header(listing: &Listing, view: &ViewSpec, slot: &Slot, first: usize) -> Group {
    let (title, ext) = match view.group {
        GroupKey::Kind if listing.entries[first].is_dir => ("Folder".to_string(), None),
        GroupKey::Kind => (String::new(), Some(slot.label.clone())),
        // Tag slots are folded for ordering; show the tag as spelled.
        GroupKey::Tags if slot.rank == 0 => (listing.tags()[first][0].clone(), None),
        _ => (slot.label.clone(), None),
    };
    Group { title, start: 0, ext }
}

fn build_order(listing: &Listing, view: &ViewSpec) -> (Vec<u32>, Vec<Group>) {
    let needle = fold(&view.filter);
    let mut order: Vec<u32> = (0..listing.entries.len() as u32)
        .filter(|i| {
            let e = &listing.entries[*i as usize];
            (view.show_hidden || !e.is_hidden) && (needle.is_empty() || listing.keys[*i as usize].contains(&needle))
        })
        .collect();

    let spec = view.sort;
    // Keys that need a lookup per item are worked out once, not per compare.
    let text_key: Vec<String> = match spec.key {
        SortKey::Owner => listing.entries.iter().map(|e| e.owner_name().unwrap_or_default()).collect(),
        SortKey::Group => listing.entries.iter().map(|e| e.group_name().unwrap_or_default()).collect(),
        SortKey::Tags => listing.tags().iter().map(|tags| tags.first().map(|t| fold(t)).unwrap_or_default()).collect(),
        _ => Vec::new(),
    };
    let added: &[Option<i64>] = if spec.key == SortKey::Added { listing.added() } else { &[] };
    let slots = group_slots(listing, view);
    order.sort_unstable_by(|a, b| {
        let (ia, ib) = (*a as usize, *b as usize);
        let (ea, eb) = (&listing.entries[ia], &listing.entries[ib]);
        // Groups first, in their own order whichever way the list runs.
        if !slots.is_empty() {
            let by_group = slots[ia].cmp(&slots[ib]);
            if by_group != Ordering::Equal {
                return by_group;
            }
        }
        if spec.folders_first && ea.is_dir != eb.is_dir {
            return if ea.is_dir { Ordering::Less } else { Ordering::Greater };
        }
        let by_name = || {
            natural_cmp(&listing.keys[ia], &listing.keys[ib])
                // Names that fold equal (e.g. "a" vs "A", NFC vs NFD) still
                // need a stable order.
                .then_with(|| ea.name.cmp(&eb.name))
        };
        if spec.folders_first && view.folders_by_name && ea.is_dir && eb.is_dir {
            return by_name();
        }
        let primary = match spec.key {
            SortKey::Name => Ordering::Equal,
            SortKey::Size => ea.size.cmp(&eb.size),
            SortKey::Modified => ea.modified_ms.cmp(&eb.modified_ms),
            SortKey::Created => ea.created_ms.cmp(&eb.created_ms),
            SortKey::Kind => listing.exts[ia].cmp(&listing.exts[ib]),
            SortKey::Added => added[ia].cmp(&added[ib]),
            SortKey::Permissions => ea.mode.cmp(&eb.mode),
            // Untagged (empty key) after tagged, whichever way the list runs.
            SortKey::Tags => match (text_key[ia].is_empty(), text_key[ib].is_empty()) {
                (true, false) => return Ordering::Greater,
                (false, true) => return Ordering::Less,
                _ => natural_cmp(&text_key[ia], &text_key[ib]),
            },
            SortKey::Owner | SortKey::Group => natural_cmp(&fold(&text_key[ia]), &fold(&text_key[ib])),
        };
        let ord = primary.then_with(by_name);
        if spec.ascending {
            ord
        } else {
            ord.reverse()
        }
    });
    let mut groups: Vec<Group> = Vec::new();
    if !slots.is_empty() {
        let mut last: Option<&Slot> = None;
        for (row, idx) in order.iter().enumerate() {
            let slot = &slots[*idx as usize];
            if last != Some(slot) {
                let mut header = group_header(listing, view, slot, *idx as usize);
                header.start = row as u32;
                groups.push(header);
                last = Some(slot);
            }
        }
    }
    (order, groups)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::EntryKind;

    fn file(name: &str, size: u64, modified: i64) -> Entry {
        Entry {
            name: name.into(),
            kind: EntryKind::File,
            is_dir: false,
            size,
            modified_ms: Some(modified),
            created_ms: Some(modified),
            is_hidden: name.starts_with('.'),
            mode: Some(0o644),
            link_target: None,
            is_broken_link: false,
            ext: Default::default(),
        }
    }

    fn dir(name: &str) -> Entry {
        Entry { kind: EntryKind::Directory, is_dir: true, size: 0, ..file(name, 0, 0) }
    }

    fn names(s: &Snapshot) -> Vec<&str> {
        s.iter().map(|e| e.name.as_str()).collect()
    }

    fn sample() -> Vec<Entry> {
        vec![
            file("b.txt", 30, 3),
            file("a10.png", 10, 1),
            file("a2.png", 20, 2),
            dir("zeta"),
            dir("Alpha"),
            file(".hidden", 5, 9),
        ]
    }

    #[test]
    fn default_view_folders_first_natural_no_hidden() {
        let s = Snapshot::new("/x", sample(), ViewSpec::default());
        assert_eq!(names(&s), vec!["Alpha", "zeta", "a2.png", "a10.png", "b.txt"]);
        assert_eq!(s.len(), 5);
        assert_eq!(s.total_len(), 6);
    }

    #[test]
    fn show_hidden() {
        let view = ViewSpec { show_hidden: true, ..Default::default() };
        let s = Snapshot::new("/x", sample(), view);
        assert_eq!(s.len(), 6);
        assert_eq!(s.get(2).unwrap().name, ".hidden");
    }

    #[test]
    fn sort_by_size_descending_keeps_folders_first() {
        let view = ViewSpec {
            sort: SortSpec { key: SortKey::Size, ascending: false, folders_first: true },
            ..Default::default()
        };
        let s = Snapshot::new("/x", sample(), view);
        assert_eq!(names(&s), vec!["zeta", "Alpha", "b.txt", "a2.png", "a10.png"]);
    }

    #[test]
    fn sort_by_kind_and_modified() {
        let base = Snapshot::new("/x", sample(), ViewSpec::default());
        let by_kind = base.with_view(ViewSpec {
            sort: SortSpec { key: SortKey::Kind, ascending: true, folders_first: false },
            ..Default::default()
        });
        // Folders have no extension so they sort first; then png, then txt.
        assert_eq!(names(&by_kind), vec!["Alpha", "zeta", "a2.png", "a10.png", "b.txt"]);

        let by_date = base.with_view(ViewSpec {
            sort: SortSpec { key: SortKey::Modified, ascending: false, folders_first: false },
            ..Default::default()
        });
        assert_eq!(names(&by_date), vec!["b.txt", "a2.png", "a10.png", "zeta", "Alpha"]);
    }

    fn sorted_by(entries: Vec<Entry>, key: SortKey, ascending: bool) -> Vec<String> {
        let view = ViewSpec { sort: SortSpec { key, ascending, folders_first: false }, ..Default::default() };
        Snapshot::new("/x", entries, view).iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn sort_by_date_added() {
        let mut entries = vec![file("old", 1, 1), file("new", 1, 1), file("mid", 1, 1), file("unknown", 1, 1)];
        for (e, added) in entries.iter_mut().zip([Some(100), Some(300), Some(200), None]) {
            e.ext.added_ms = added;
        }
        assert_eq!(sorted_by(entries.clone(), SortKey::Added, false), ["new", "mid", "old", "unknown"]);
        assert_eq!(sorted_by(entries, SortKey::Added, true), ["unknown", "old", "mid", "new"]);
    }

    /// Listing leaves date added and tags out (they are slow to read); a
    /// view that sorts by them reads them from the source then.
    #[cfg(target_os = "macos")]
    #[test]
    fn sorting_by_added_or_tags_reads_them_from_disk() {
        use crate::{LocalFs, Vfs};
        let dir = tempfile::tempdir().unwrap();
        for name in ["first", "second", "third"] {
            std::fs::write(dir.path().join(name), "x").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // Finder stores tags as a binary plist; make one the same way.
        let plist = dir.path().join("tags.plist");
        std::fs::write(&plist, "<plist version=\"1.0\"><array><string>Red\n6</string></array></plist>").unwrap();
        let run = |cmd: &mut std::process::Command| assert!(cmd.status().unwrap().success());
        run(std::process::Command::new("plutil").args(["-convert", "binary1"]).arg(&plist));
        let hex: String = std::fs::read(&plist).unwrap().iter().map(|b| format!("{b:02x}")).collect();
        std::fs::remove_file(&plist).unwrap();
        run(std::process::Command::new("xattr")
            .args(["-wx", "com.apple.metadata:_kMDItemUserTags", &hex])
            .arg(dir.path().join("third")));
        let path = dir.path().to_str().unwrap();
        let entries = LocalFs.list(path).unwrap();
        assert!(entries.iter().all(|e| e.ext.added_ms.is_none() && e.ext.tags.is_empty()));
        let source: Arc<dyn Vfs> = Arc::new(LocalFs);
        let snap = Snapshot::listed(path, entries, ViewSpec::default(), Some(source));
        let view =
            |key, ascending| ViewSpec { sort: SortSpec { key, ascending, folders_first: true }, ..Default::default() };
        let names = |s: Snapshot| s.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(snap.with_view(view(SortKey::Added, false))), ["third", "second", "first"]);
        assert_eq!(names(snap.with_view(view(SortKey::Tags, true))), ["third", "first", "second"]);
    }

    fn grouped(entries: Vec<Entry>, group: GroupKey, key: SortKey, ascending: bool) -> Vec<String> {
        let view = ViewSpec { sort: SortSpec { key, ascending, folders_first: true }, group, ..Default::default() };
        let s = Snapshot::new("/x", entries, view);
        // "[Header] a b [Header] c" so order and headers read as one line.
        let mut out = Vec::new();
        for (row, e) in s.iter().enumerate() {
            if let Some(g) = s.groups().iter().find(|g| g.start as usize == row) {
                out.push(format!("[{}]", g.ext.clone().unwrap_or_else(|| g.title.clone())));
            }
            out.push(e.name.clone());
        }
        out
    }

    /// VW-05, and size groups run small to large with sorting inside them.
    #[test]
    fn group_by_size() {
        let entries = vec![file("big", 2_000_000_000, 1), file("small", 5_000, 1), file("tiny", 10, 1), dir("d")];
        assert_eq!(
            grouped(entries, GroupKey::Size, SortKey::Name, true),
            ["[Folders]", "d", "[Less than 10 KB]", "small", "tiny", "[100 MB to 10 GB]", "big"]
        );
    }

    #[test]
    fn group_by_kind_folders_first_then_extensions() {
        let entries = vec![file("b.txt", 1, 1), file("a.png", 1, 1), dir("zz"), file("c.png", 1, 1)];
        assert_eq!(
            grouped(entries, GroupKey::Kind, SortKey::Name, false),
            ["[Folder]", "zz", "[png]", "c.png", "a.png", "[txt]", "b.txt"]
        );
    }

    #[test]
    fn group_by_tags_untagged_last() {
        let mut entries = vec![file("plain", 1, 1), file("red", 1, 1), file("blue", 1, 1)];
        entries[1].ext.tags = vec!["Red".into()];
        entries[2].ext.tags = vec!["Blue".into(), "Red".into()];
        assert_eq!(
            grouped(entries, GroupKey::Tags, SortKey::Name, true),
            ["[Blue]", "blue", "[Red]", "red", "[No Tags]", "plain"]
        );
    }

    #[test]
    fn group_by_date_newest_group_first_even_ascending() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
        let entries = vec![file("old", 1, now - 400 * 86_400_000), file("new", 1, now - 60_000), file("newer", 1, now)];
        assert_eq!(
            grouped(entries, GroupKey::Modified, SortKey::Name, true),
            [
                "[Today]",
                "new",
                "newer",
                &format!("[{}]", crate::group::date_slot(Some(now - 400 * 86_400_000), now, 0, 0).label),
                "old"
            ]
        );
    }

    #[test]
    fn folders_can_stay_alphabetical_while_files_sort_by_size() {
        let mut entries = vec![file("small", 1, 1), file("big", 9, 1), dir("b"), dir("a")];
        entries[2].size = 5; // a folder with a size must still go by name
        let view = ViewSpec {
            sort: SortSpec { key: SortKey::Size, ascending: false, folders_first: true },
            folders_by_name: true,
            ..Default::default()
        };
        let s = Snapshot::new("/x", entries, view);
        assert_eq!(s.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["a", "b", "big", "small"]);
    }

    #[test]
    fn ungrouped_has_no_headers() {
        assert!(Snapshot::new("/x", sample(), ViewSpec::default()).groups().is_empty());
    }

    #[test]
    fn sort_by_tags_puts_untagged_last_both_ways() {
        let mut entries = vec![file("none", 1, 1), file("work", 1, 1), file("red", 1, 1)];
        entries[1].ext.tags = vec!["Work".into()];
        entries[2].ext.tags = vec!["Red".into(), "Work".into()];
        assert_eq!(sorted_by(entries.clone(), SortKey::Tags, true), ["red", "work", "none"]);
        assert_eq!(sorted_by(entries, SortKey::Tags, false), ["work", "red", "none"]);
    }

    #[test]
    fn sort_by_permissions_and_owner() {
        let mut entries = vec![file("open", 1, 1), file("private", 1, 1)];
        entries[0].mode = Some(0o755);
        entries[1].mode = Some(0o600);
        assert_eq!(sorted_by(entries.clone(), SortKey::Permissions, true), ["private", "open"]);
        entries[0].ext.uid = Some(0); // root
        entries[1].ext.uid = Some(4_000_000_000); // no such user: shown as the number
        assert_eq!(sorted_by(entries, SortKey::Owner, true), ["private", "open"]);
    }

    #[test]
    fn filter_is_case_and_form_insensitive() {
        let mut entries = sample();
        entries.push(file("Cafe\u{301}.txt", 1, 1)); // NFD on disk
        let view = ViewSpec {
            filter: "CAF\u{c9}".into(), // NFC, upper-case
            ..Default::default()
        };
        let s = Snapshot::new("/x", entries, view);
        assert_eq!(names(&s), vec!["Cafe\u{301}.txt"]);
    }

    #[test]
    fn index_and_prefix_lookup() {
        let s = Snapshot::new("/x", sample(), ViewSpec::default());
        assert_eq!(s.index_of("a10.png"), Some(3));
        assert_eq!(s.index_of(".hidden"), None);
        assert_eq!(s.indices_of(&["b.txt".into(), "Alpha".into(), ".hidden".into(), "gone".into()]), vec![0, 4]);
        assert_eq!(s.total_size(&[0, 2, 3, 4, 99]), 60);
        assert_eq!(s.find_prefix("A", 0), Some(0));
        // Wraps past the end and finds the next match after the start row.
        assert_eq!(s.find_prefix("a", 1), Some(2));
        assert_eq!(s.find_prefix("a", 4), Some(0));
        assert_eq!(s.find_prefix("nope", 0), None);
        assert_eq!(s.find_prefix("", 0), None);
    }

    #[test]
    fn empty_directory() {
        let s = Snapshot::new("/x", vec![], ViewSpec::default());
        assert!(s.is_empty());
        assert!(s.get(0).is_none());
        assert_eq!(s.find_prefix("a", 0), None);
    }

    #[test]
    fn names_differing_only_by_case_are_both_kept_in_stable_order() {
        let s = Snapshot::new("/x", vec![file("readme", 1, 1), file("README", 1, 1)], ViewSpec::default());
        assert_eq!(names(&s), vec!["README", "readme"]);
    }

    #[test]
    fn sorts_100k_entries_quickly() {
        let entries: Vec<Entry> =
            (0..100_000).map(|i| file(&format!("file{}.dat", (i * 7919) % 100_000), i as u64, i as i64)).collect();
        let start = std::time::Instant::now();
        let s = Snapshot::new("/big", entries, ViewSpec::default());
        assert_eq!(s.len(), 100_000);
        assert_eq!(s.get(0).unwrap().name, "file0.dat");
        assert_eq!(s.get(99_999).unwrap().name, "file99999.dat");
        // Generous bound: debug builds on a busy machine.
        assert!(start.elapsed().as_secs() < 5, "took {:?}", start.elapsed());
    }
}
