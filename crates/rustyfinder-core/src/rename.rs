//! Multi-Rename: an ordered list of rules turns each selected name into a
//! new one. This module computes the preview (with every invalid or
//! clashing row flagged) and the order in which to rename so that swaps
//! and chains never collide.

use std::collections::{HashMap, HashSet};

use regex::RegexBuilder;
use serde::{Deserialize, Serialize};

use crate::ops::names::{split_ext, MAX_NAME_BYTES};
use crate::paths;
use crate::sort::fold;

/// Which part of the name a rule works on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Target {
    #[default]
    Name,
    Extension,
    NameWithExtension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Position {
    Beginning,
    #[default]
    End,
    /// 0-based character index from the start, or from the end. An index
    /// past the text clamps to the far end.
    Index {
        index: usize,
        from_end: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Case {
    Lower,
    Upper,
    /// First letter of each word upper, the rest lower.
    Title,
    /// First letter of the text upper, the rest lower.
    Sentence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Occurrences {
    #[default]
    All,
    First,
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DateSource {
    /// The moment the preview is computed.
    Now,
    Created,
    Modified,
    /// A fixed date, in milliseconds since the Unix epoch.
    Custom(i64),
    /// A date from the file's metadata (photo capture date...).
    Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleKind {
    ChangeCase {
        case: Case,
    },
    ReplaceText {
        find: String,
        replace: String,
        case_sensitive: bool,
        regex: bool,
        occurrences: Occurrences,
        /// Replace the whole target text, ignoring `find`.
        entire_text: bool,
    },
    AddText {
        text: String,
        position: Position,
    },
    AddDate {
        source: DateSource,
        /// Unicode date pattern, e.g. `yyyy-MM-dd` or `HHmmss`.
        format: String,
        position: Position,
        separator: String,
    },
    AddSequence {
        start: i64,
        step: i64,
        /// Minimum number of digits: 3 gives `001`.
        padding: usize,
        position: Position,
        prefix: String,
        suffix: String,
    },
    /// Insert a metadata value (dimensions, camera model, artist...).
    AddMetadata {
        key: String,
        position: Position,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub kind: RuleKind,
    #[serde(default)]
    pub target: Target,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "yes")]
    pub files: bool,
    #[serde(default = "yes")]
    pub folders: bool,
}

fn yes() -> bool {
    true
}

impl Rule {
    pub fn new(kind: RuleKind) -> Self {
        Self { kind, target: Target::Name, enabled: true, files: true, folders: true }
    }

    pub fn on(mut self, target: Target) -> Self {
        self.target = target;
        self
    }
}

/// A saved rule list. `version` lets later releases read older presets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    pub version: u32,
    pub name: String,
    pub rules: Vec<Rule>,
}

pub const PRESET_VERSION: u32 = 1;

/// One item to rename.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Item {
    pub path: String,
    pub is_dir: bool,
    pub created_ms: Option<i64>,
    pub modified_ms: Option<i64>,
    /// Date found in the file's metadata, for `DateSource::Metadata`.
    pub metadata_date_ms: Option<i64>,
    /// Metadata values by key, for `AddMetadata`.
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub original: String,
    pub new_name: String,
    pub changed: bool,
    /// The new name cannot be used at all (empty, has a slash, too long).
    pub invalid: bool,
    /// The new name collides with another row or an existing item.
    pub conflict: bool,
    /// Why the row is flagged, or a remark such as "no metadata date".
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub rows: Vec<Row>,
    /// Rows whose name changes.
    pub changed: usize,
    /// Rows that are invalid or conflicting.
    pub problems: usize,
    /// Indices of rules that cannot work (e.g. an invalid regex).
    pub broken_rules: Vec<usize>,
}

impl Preview {
    /// Rename is allowed only if something changes and nothing is flagged.
    pub fn can_rename(&self) -> bool {
        self.changed > 0 && self.problems == 0
    }

    /// Footer text: "2 of 5 items will be renamed".
    pub fn summary(&self) -> String {
        let mut text = match self.changed {
            0 => "No item will be renamed".to_string(),
            n => format!("{n} of {} items will be renamed", self.rows.len()),
        };
        if self.problems > 0 {
            text.push_str(&format!(
                ". {} {} conflicting or invalid names",
                self.problems,
                if self.problems == 1 { "item has" } else { "items have" }
            ));
        }
        text
    }
}

/// Facts about the place the items live in.
pub struct Context<'a> {
    /// Time of the preview, for `DateSource::Now`.
    pub now_ms: i64,
    /// Local volume: a colon is not allowed in a name.
    pub local: bool,
    /// Names that differ only by case are the same name.
    pub case_insensitive: bool,
    /// Does an item with this name exist in this folder?
    pub exists: &'a dyn Fn(&str, &str) -> bool,
}

/// Apply `rules` to `items` (in the given order, which drives sequences)
/// and flag every row that could not be renamed as shown.
pub fn preview(items: &[Item], rules: &[Rule], ctx: &Context<'_>) -> Preview {
    // Compile regexes once; a rule whose pattern is invalid does nothing.
    let mut broken_rules = Vec::new();
    let compiled: Vec<Option<regex::Regex>> = rules
        .iter()
        .enumerate()
        .map(|(i, rule)| match &rule.kind {
            RuleKind::ReplaceText { find, case_sensitive, regex, entire_text, .. } if !entire_text => {
                let pattern = if *regex { find.clone() } else { regex::escape(find) };
                match RegexBuilder::new(&pattern).case_insensitive(!case_sensitive).build() {
                    Ok(re) => Some(re),
                    Err(_) => {
                        broken_rules.push(i);
                        None
                    }
                }
            }
            _ => None,
        })
        .collect();

    // Sequence numbers count the rows each rule applies to.
    let mut counters = vec![0i64; rules.len()];
    let mut rows: Vec<Row> = items
        .iter()
        .map(|item| {
            let original = paths::file_name(&item.path).to_string();
            let mut name = original.clone();
            let mut note = String::new();
            for (i, rule) in rules.iter().enumerate() {
                let applies = rule.enabled && if item.is_dir { rule.folders } else { rule.files };
                if !applies || broken_rules.contains(&i) {
                    continue;
                }
                let ordinal = counters[i];
                counters[i] += 1;
                name = apply(rule, compiled[i].as_ref(), &name, item, ordinal, ctx.now_ms, &mut note);
            }
            Row { changed: name != original, original, new_name: name, invalid: false, conflict: false, note }
        })
        .collect();

    // Validity of each new name on its own.
    for row in rows.iter_mut().filter(|r| r.changed) {
        let reason = if row.new_name.is_empty() {
            Some("The name is empty")
        } else if row.new_name == "." || row.new_name == ".." {
            Some("That name is reserved")
        } else if row.new_name.contains('/') {
            Some("A name can’t contain a slash")
        } else if ctx.local && row.new_name.contains(':') {
            Some("A name can’t contain a colon")
        } else if row.new_name.len() > MAX_NAME_BYTES {
            Some("The name is too long")
        } else {
            None
        };
        if let Some(reason) = reason {
            row.invalid = true;
            row.note = reason.to_string();
        }
    }

    // Clashes: with each other, and with items that are not being renamed.
    let key = |name: &str| if ctx.case_insensitive { fold(name) } else { name.to_string() };
    let dir_of = |i: usize| paths::parent(&items[i].path).unwrap_or_else(|| "/".to_string());
    let mut targets: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, row) in rows.iter().enumerate() {
        targets.entry((dir_of(i), key(&row.new_name))).or_default().push(i);
    }
    // Names that go away, per folder: taking one of them is a swap or a
    // chain, not a clash.
    let vacated: HashSet<(String, String)> =
        rows.iter().enumerate().filter(|(_, r)| r.changed).map(|(i, r)| (dir_of(i), key(&r.original))).collect();
    for (i, row) in rows.iter().enumerate().map(|(i, r)| (i, r.clone())).collect::<Vec<_>>() {
        if !row.changed || row.invalid {
            continue;
        }
        let dir = dir_of(i);
        let k = key(&row.new_name);
        let reason = if targets[&(dir.clone(), k.clone())].len() > 1 {
            Some("Another item gets the same name")
        } else if k == key(&row.original) {
            None // only the spelling changes: the same item
        } else if (ctx.exists)(&dir, &row.new_name) && !vacated.contains(&(dir, k)) {
            Some("An item with this name already exists")
        } else {
            None
        };
        if let Some(reason) = reason {
            rows[i].conflict = true;
            rows[i].note = reason.to_string();
        }
    }

    Preview {
        changed: rows.iter().filter(|r| r.changed).count(),
        problems: rows.iter().filter(|r| r.invalid || r.conflict).count(),
        rows,
        broken_rules,
    }
}

/// Apply one rule to one name.
fn apply(
    rule: &Rule,
    compiled: Option<&regex::Regex>,
    name: &str,
    item: &Item,
    ordinal: i64,
    now_ms: i64,
    note: &mut String,
) -> String {
    // Split off the part the rule works on. Folders have no extension.
    let (stem, dotted_ext) = split_ext(name, item.is_dir);
    let ext = dotted_ext.strip_prefix('.').unwrap_or("");
    let text = match rule.target {
        Target::Name => stem,
        Target::Extension => ext,
        Target::NameWithExtension => name,
    };
    let changed = match &rule.kind {
        RuleKind::ChangeCase { case } => change_case(text, *case),
        RuleKind::ReplaceText { replace, occurrences, entire_text, .. } => {
            if *entire_text {
                replace.clone()
            } else {
                match compiled {
                    Some(re) => replace_occurrences(re, text, replace, *occurrences),
                    None => text.to_string(),
                }
            }
        }
        RuleKind::AddText { text: extra, position } => insert(text, extra, *position),
        RuleKind::AddDate { source, format, position, separator } => {
            let when = match source {
                DateSource::Now => Some(now_ms),
                DateSource::Created => item.created_ms,
                DateSource::Modified => item.modified_ms,
                DateSource::Custom(ms) => Some(*ms),
                DateSource::Metadata => item.metadata_date_ms,
            };
            match when {
                Some(ms) => {
                    let date = format_date(ms, format);
                    // The separator sits between the date and the name.
                    let piece = match position {
                        Position::Beginning => format!("{date}{separator}"),
                        _ => format!("{separator}{date}"),
                    };
                    insert(text, &piece, *position)
                }
                None => {
                    *note = "No date found for this item".to_string();
                    text.to_string()
                }
            }
        }
        RuleKind::AddSequence { start, step, padding, position, prefix, suffix } => {
            let number = start + step * ordinal;
            let digits = format!("{:0width$}", number.unsigned_abs(), width = *padding);
            let sign = if number < 0 { "-" } else { "" };
            insert(text, &format!("{prefix}{sign}{digits}{suffix}"), *position)
        }
        RuleKind::AddMetadata { key, position } => match item.metadata.get(key) {
            Some(value) if !value.is_empty() => insert(text, value, *position),
            _ => text.to_string(),
        },
    };
    match rule.target {
        Target::Name => format!("{changed}{dotted_ext}"),
        Target::Extension if changed.is_empty() => stem.to_string(),
        Target::Extension => format!("{stem}.{changed}"),
        Target::NameWithExtension => changed,
    }
}

fn change_case(text: &str, case: Case) -> String {
    match case {
        Case::Lower => text.to_lowercase(),
        Case::Upper => text.to_uppercase(),
        Case::Sentence => {
            let mut chars = text.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect(),
                None => String::new(),
            }
        }
        Case::Title => {
            let mut out = String::with_capacity(text.len());
            let mut start_of_word = true;
            for c in text.chars() {
                if start_of_word {
                    out.extend(c.to_uppercase());
                } else {
                    out.extend(c.to_lowercase());
                }
                start_of_word = matches!(c, ' ' | '-' | '_' | '.');
            }
            out
        }
    }
}

fn replace_occurrences(re: &regex::Regex, text: &str, replacement: &str, which: Occurrences) -> String {
    match which {
        Occurrences::All => re.replace_all(text, replacement).into_owned(),
        Occurrences::First => re.replace(text, replacement).into_owned(),
        Occurrences::Last => match re.captures_iter(text).last() {
            Some(captures) => {
                let whole = captures.get(0).expect("group 0 always exists");
                let mut expanded = String::new();
                captures.expand(replacement, &mut expanded);
                format!("{}{}{}", &text[..whole.start()], expanded, &text[whole.end()..])
            }
            None => text.to_string(),
        },
    }
}

/// Insert `extra` into `text` at a position counted in characters.
fn insert(text: &str, extra: &str, position: Position) -> String {
    let length = text.chars().count();
    let at = match position {
        Position::Beginning => 0,
        Position::End => length,
        Position::Index { index, from_end: false } => index.min(length),
        Position::Index { index, from_end: true } => length - index.min(length),
    };
    let byte = text.char_indices().nth(at).map_or(text.len(), |(i, _)| i);
    format!("{}{}{}", &text[..byte], extra, &text[byte..])
}

/// Format a moment with a Unicode date pattern, in local time. Supported
/// letters: y (calendar year), M, d, H, h, m, s, a; text in single quotes
/// is literal; everything else is copied as is. `Y` is treated as `y`:
/// the week-based year would turn 2024-12-30 into 2025.
pub fn format_date(ms: i64, pattern: &str) -> String {
    let secs = ms.div_euclid(1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the duration of the call.
    unsafe { libc::localtime_r(&secs, &mut tm) };
    let (year, month, day) = (tm.tm_year as i64 + 1900, tm.tm_mon + 1, tm.tm_mday);
    let (hour, minute, second) = (tm.tm_hour, tm.tm_min, tm.tm_sec);

    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let mut out = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            // Quoted literal; '' is a quote character.
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                out.push(chars[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        let mut run = 1;
        while i + run < chars.len() && chars[i + run] == c {
            run += 1;
        }
        let padded = |value: i64| format!("{value:0run$}");
        match c {
            'y' | 'Y' if run == 2 => out.push_str(&format!("{:02}", year.rem_euclid(100))),
            'y' | 'Y' => out.push_str(&padded(year)),
            'M' if run == 3 => out.push_str(&MONTHS[(month - 1) as usize][..3]),
            'M' if run >= 4 => out.push_str(MONTHS[(month - 1) as usize]),
            'M' => out.push_str(&padded(month as i64)),
            'd' => out.push_str(&padded(day as i64)),
            'H' => out.push_str(&padded(hour as i64)),
            'h' => out.push_str(&padded(if hour % 12 == 0 { 12 } else { (hour % 12) as i64 })),
            'm' => out.push_str(&padded(minute as i64)),
            's' => out.push_str(&padded(second as i64)),
            'a' => out.push_str(if hour < 12 { "AM" } else { "PM" }),
            other => out.extend(std::iter::repeat_n(other, run)),
        }
        i += run;
    }
    out
}

/// One rename to perform. `pair` is the index of the requested pair it
/// belongs to, so a failure can be traced back to the item it concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub from: String,
    pub to: String,
    pub pair: usize,
}

/// Order `pairs` (current path, new path) so that no rename ever lands on
/// a name still in use by another item of the set. Swaps and cycles are
/// broken with a temporary name. `case_insensitive` says whether names
/// differing only by case are the same name on this file system.
pub fn plan(pairs: &[(String, String)], case_insensitive: bool) -> Vec<(String, String)> {
    plan_steps(pairs, case_insensitive).into_iter().map(|s| (s.from, s.to)).collect()
}

/// Like `plan`, keeping track of which requested pair each step serves.
pub fn plan_steps(pairs: &[(String, String)], case_insensitive: bool) -> Vec<Step> {
    let key = |path: &str| if case_insensitive { fold(path) } else { path.to_string() };
    let mut pending: Vec<Step> = pairs
        .iter()
        .enumerate()
        .filter(|(_, (from, to))| from != to)
        .map(|(pair, (from, to))| Step { from: from.clone(), to: to.clone(), pair })
        .collect();
    let mut steps = Vec::with_capacity(pending.len());
    let mut temp = 0usize;
    while !pending.is_empty() {
        // A rename is safe now if nothing else still sits on its target.
        let safe = pending
            .iter()
            .position(|step| !pending.iter().any(|other| other.from != step.from && key(&other.from) == key(&step.to)));
        match safe {
            Some(i) => steps.push(pending.remove(i)),
            None => {
                // Everything left is in a cycle. Move one item out of the
                // way under a temporary name; it goes to its real name
                // once that is free.
                let step = pending.remove(0);
                let dir = paths::parent(&step.from).unwrap_or_else(|| "/".to_string());
                let parked = paths::join(&dir, &format!(".rustyfinder-rename-{}-{temp}", std::process::id()));
                temp += 1;
                steps.push(Step { from: step.from, to: parked.clone(), pair: step.pair });
                pending.push(Step { from: parked, to: step.to, pair: step.pair });
            }
        }
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(names: &[&str]) -> Vec<Item> {
        names.iter().map(|n| Item { path: format!("/d/{n}"), ..Default::default() }).collect()
    }

    fn run(names: &[&str], rules: &[Rule]) -> Preview {
        run_with(&items(names), rules, &|_, _| false)
    }

    fn run_with(items: &[Item], rules: &[Rule], exists: &dyn Fn(&str, &str) -> bool) -> Preview {
        preview(
            items,
            rules,
            &Context {
                now_ms: 1_735_560_000_000, // 2024-12-30 12:00 UTC
                local: true,
                case_insensitive: true,
                exists,
            },
        )
    }

    fn new_names(p: &Preview) -> Vec<&str> {
        p.rows.iter().map(|r| r.new_name.as_str()).collect()
    }

    fn sequence(start: i64, padding: usize, position: Position, suffix: &str) -> Rule {
        Rule::new(RuleKind::AddSequence {
            start,
            step: 1,
            padding,
            position,
            prefix: String::new(),
            suffix: suffix.into(),
        })
    }

    fn replace(find: &str, with: &str, regex: bool) -> Rule {
        Rule::new(RuleKind::ReplaceText {
            find: find.into(),
            replace: with.into(),
            case_sensitive: false,
            regex,
            occurrences: Occurrences::All,
            entire_text: false,
        })
    }

    #[test]
    fn mr01_sequence_at_the_beginning_with_padding_and_suffix() {
        let p = run(&["a.txt", "b.txt"], &[sequence(1, 3, Position::Beginning, "_")]);
        assert_eq!(new_names(&p), vec!["001_a.txt", "002_b.txt"]);
        assert!(p.can_rename());
        assert_eq!(p.summary(), "2 of 2 items will be renamed");
    }

    #[test]
    fn mr02_regex_groups_on_the_name_and_lower_case_on_the_extension() {
        let rules = [
            replace(r"IMG_(\d+)", "photo-$1", true),
            Rule::new(RuleKind::ChangeCase { case: Case::Lower }).on(Target::Extension),
        ];
        assert_eq!(new_names(&run(&["IMG_0001.JPG"], &rules)), vec!["photo-0001.jpg"]);
    }

    #[test]
    fn mr03_two_rows_with_the_same_new_name_are_flagged_and_block_rename() {
        let p = run(&["x1.txt", "x2.txt"], &[replace(r"x\d", "y", true)]);
        assert_eq!(new_names(&p), vec!["y.txt", "y.txt"]);
        assert!(p.rows.iter().all(|r| r.conflict));
        assert_eq!(p.problems, 2);
        assert!(!p.can_rename());
        assert_eq!(p.summary(), "2 of 2 items will be renamed. 2 items have conflicting or invalid names");
    }

    #[test]
    fn mr04_a_swap_is_not_a_conflict_and_is_ordered_through_a_temporary_name() {
        // a -> b and b -> a.
        let rules = [replace("^a$", "TMP", true), replace("^b$", "a", true), replace("^TMP$", "b", true)];
        let p = run_with(&items(&["a", "b"]), &rules, &|_, name| name == "a" || name == "b");
        assert_eq!(new_names(&p), vec!["b", "a"]);
        assert!(p.can_rename(), "{:?}", p.rows);

        let steps = plan(&[("/d/a".into(), "/d/b".into()), ("/d/b".into(), "/d/a".into())], true);
        assert_eq!(steps.len(), 3);
        // Replay on a model of the folder: no step lands on a used name.
        let mut names: HashSet<String> = ["/d/a".to_string(), "/d/b".to_string()].into();
        let mut content: HashMap<String, &str> = [("/d/a".to_string(), "A"), ("/d/b".to_string(), "B")].into();
        for (from, to) in &steps {
            assert!(names.contains(from), "{from} must exist");
            assert!(!names.contains(to), "{to} must be free");
            names.remove(from);
            names.insert(to.clone());
            let moved = content.remove(from).unwrap();
            content.insert(to.clone(), moved);
        }
        assert_eq!(content["/d/a"], "B");
        assert_eq!(content["/d/b"], "A");
    }

    #[test]
    fn mr05_a_case_only_change_is_not_a_conflict_with_itself() {
        let rule = Rule::new(RuleKind::ChangeCase { case: Case::Upper });
        // The volume ignores case, so "REPORT.txt" already "exists".
        let p = run_with(&items(&["report.txt"]), &[rule], &|_, name| name.eq_ignore_ascii_case("report.txt"));
        assert_eq!(new_names(&p), vec!["REPORT.txt"]);
        assert!(p.can_rename());
        assert_eq!(plan(&[("/d/report.txt".into(), "/d/REPORT.txt".into())], true).len(), 1);
    }

    #[test]
    fn mr06_date_uses_the_calendar_year_and_the_separator() {
        // Local noon keeps the calendar day the same in every time zone.
        let noon = {
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            tm.tm_year = 124;
            tm.tm_mon = 11;
            tm.tm_mday = 30;
            tm.tm_hour = 12;
            tm.tm_isdst = -1;
            (unsafe { libc::mktime(&mut tm) }) as i64 * 1000
        };
        let item = Item { path: "/d/notes.txt".into(), modified_ms: Some(noon), ..Default::default() };
        let date = |format: &str, position| {
            Rule::new(RuleKind::AddDate {
                source: DateSource::Modified,
                format: format.into(),
                position,
                separator: " ".into(),
            })
        };
        let p = run_with(std::slice::from_ref(&item), &[date("yyyy-MM-dd", Position::Beginning)], &|_, _| false);
        assert_eq!(new_names(&p), vec!["2024-12-30 notes.txt"]);
        // "YYYY" (week-based year) must not turn this into 2025.
        let p = run_with(std::slice::from_ref(&item), &[date("YYYY-MM-dd", Position::End)], &|_, _| false);
        assert_eq!(new_names(&p), vec!["notes 2024-12-30.txt"]);
        assert_eq!(format_date(noon, "yy.M.d 'at' HH:mm:ss"), "24.12.30 at 12:00:00");
        assert_eq!(format_date(noon, "MMM d, yyyy h a"), "Dec 30, 2024 12 PM");
    }

    #[test]
    fn a_missing_metadata_date_leaves_the_name_alone_with_a_note() {
        let rule = Rule::new(RuleKind::AddDate {
            source: DateSource::Metadata,
            format: "yyyy".into(),
            position: Position::Beginning,
            separator: "-".into(),
        });
        let p = run(&["photo.jpg"], &[rule]);
        assert_eq!(new_names(&p), vec!["photo.jpg"]);
        assert!(!p.rows[0].changed);
        assert!(p.rows[0].note.contains("No date"));
    }

    #[test]
    fn mr07_sequence_numbers_follow_the_row_order() {
        let rule = sequence(10, 1, Position::End, "");
        assert_eq!(
            new_names(&run(&["a.txt", "b.txt", "c.txt"], std::slice::from_ref(&rule))),
            vec!["a10.txt", "b11.txt", "c12.txt"]
        );
        assert_eq!(new_names(&run(&["c.txt", "a.txt", "b.txt"], &[rule])), vec!["c10.txt", "a11.txt", "b12.txt"]);
        // Step and a files-only filter: folders are skipped and not counted.
        let mut stepped = Rule::new(RuleKind::AddSequence {
            start: 5,
            step: 5,
            padding: 2,
            position: Position::Beginning,
            prefix: "(".into(),
            suffix: ") ".into(),
        });
        stepped.folders = false;
        let mut mixed = items(&["a", "dir", "b"]);
        mixed[1].is_dir = true;
        assert_eq!(new_names(&run_with(&mixed, &[stepped], &|_, _| false)), vec!["(05) a", "dir", "(10) b"]);
    }

    #[test]
    fn mr09_invalid_names_are_flagged_and_block_rename() {
        let cases = [("a/b", "slash"), ("a:b", "colon"), ("", "empty"), ("..", "reserved")];
        for (bad, why) in cases {
            let rule = Rule::new(RuleKind::ReplaceText {
                find: String::new(),
                replace: bad.into(),
                case_sensitive: false,
                regex: false,
                occurrences: Occurrences::All,
                entire_text: true,
            })
            .on(Target::NameWithExtension);
            let p = run(&["x.txt"], &[rule]);
            assert!(p.rows[0].invalid, "{bad:?}");
            assert!(p.rows[0].note.to_lowercase().contains(why), "{bad:?}: {}", p.rows[0].note);
            assert!(!p.can_rename());
        }
        let long = Rule::new(RuleKind::AddText { text: "x".repeat(300), position: Position::End });
        assert!(run(&["a.txt"], &[long]).rows[0].invalid);
    }

    #[test]
    fn mr10_add_text_at_an_index_from_either_end() {
        let at = |index, from_end| {
            Rule::new(RuleKind::AddText { text: "X".into(), position: Position::Index { index, from_end } })
        };
        assert_eq!(new_names(&run(&["abcd.txt"], &[at(2, false)])), vec!["abXcd.txt"]);
        assert_eq!(new_names(&run(&["abcd.txt"], &[at(1, true)])), vec!["abcXd.txt"]);
        // Past the end clamps.
        assert_eq!(new_names(&run(&["abcd.txt"], &[at(99, false)])), vec!["abcdX.txt"]);
        assert_eq!(new_names(&run(&["abcd.txt"], &[at(99, true)])), vec!["Xabcd.txt"]);
        // Counted in characters, not bytes.
        assert_eq!(new_names(&run(&["日本語.txt"], &[at(1, false)])), vec!["日X本語.txt"]);
    }

    #[test]
    fn change_case_modes_and_targets() {
        let case = |c| Rule::new(RuleKind::ChangeCase { case: c });
        assert_eq!(new_names(&run(&["my_big-file name.TXT"], &[case(Case::Title)])), vec!["My_Big-File Name.TXT"]);
        assert_eq!(new_names(&run(&["hELLO wORLD.txt"], &[case(Case::Sentence)])), vec!["Hello world.txt"]);
        assert_eq!(
            new_names(&run(&["Mixed.Case.TXT"], &[case(Case::Lower).on(Target::NameWithExtension)])),
            vec!["mixed.case.txt"]
        );
        // A folder has no extension: the Extension target leaves it alone.
        let mut dir = items(&["Folder.V2"]);
        dir[0].is_dir = true;
        assert_eq!(
            new_names(&run_with(&dir, &[case(Case::Upper).on(Target::Extension)], &|_, _| false)),
            vec!["Folder.V2"]
        );
        assert_eq!(new_names(&run_with(&dir, &[case(Case::Upper)], &|_, _| false)), vec!["FOLDER.V2"]);
    }

    #[test]
    fn replace_text_options() {
        let rule = |find: &str, with: &str, occurrences, case_sensitive| {
            Rule::new(RuleKind::ReplaceText {
                find: find.into(),
                replace: with.into(),
                case_sensitive,
                regex: false,
                occurrences,
                entire_text: false,
            })
        };
        assert_eq!(new_names(&run(&["a-a-A.txt"], &[rule("a", "b", Occurrences::All, false)])), vec!["b-b-b.txt"]);
        assert_eq!(new_names(&run(&["a-a-A.txt"], &[rule("a", "b", Occurrences::All, true)])), vec!["b-b-A.txt"]);
        assert_eq!(new_names(&run(&["a-a-a.txt"], &[rule("a", "b", Occurrences::First, false)])), vec!["b-a-a.txt"]);
        assert_eq!(new_names(&run(&["a-a-a.txt"], &[rule("a", "b", Occurrences::Last, false)])), vec!["a-a-b.txt"]);
        // Plain text is literal: regex characters mean nothing special.
        assert_eq!(new_names(&run(&["a.b(1).txt"], &[rule("(1)", "", Occurrences::All, false)])), vec!["a.b.txt"]);
        // Removing the extension drops the dot too.
        let strip = rule("txt", "", Occurrences::All, false).on(Target::Extension);
        assert_eq!(new_names(&run(&["notes.txt"], &[strip])), vec!["notes"]);
    }

    #[test]
    fn an_invalid_regex_marks_the_rule_and_changes_nothing() {
        let p = run(&["a.txt"], &[replace("(unclosed", "x", true), sequence(1, 1, Position::End, "")]);
        assert_eq!(p.broken_rules, vec![0]);
        // The other rule still works.
        assert_eq!(new_names(&p), vec!["a1.txt"]);
    }

    #[test]
    fn existing_items_conflict_unless_they_are_being_renamed_away() {
        let rule = replace("^a$", "b", true);
        // "b" exists and stays: conflict.
        let p = run_with(&items(&["a"]), std::slice::from_ref(&rule), &|_, name| name == "b");
        assert!(p.rows[0].conflict);
        assert!(p.rows[0].note.contains("already exists"));
        // "b" exists but is itself renamed to "c": a chain, not a conflict.
        let chain = [rule, replace("^b$", "c", true)];
        let p = run_with(&items(&["b", "a"]), &[chain[1].clone(), replace("^a$", "b", true)], &|_, name| name == "b");
        assert_eq!(new_names(&p), vec!["c", "b"]);
        assert!(p.can_rename(), "{:?}", p.rows);
        // A chain is ordered so the target is free first.
        let steps = plan(&[("/d/a".into(), "/d/b".into()), ("/d/b".into(), "/d/c".into())], false);
        assert_eq!(steps, vec![("/d/b".to_string(), "/d/c".to_string()), ("/d/a".to_string(), "/d/b".to_string())]);
    }

    #[test]
    fn nothing_to_rename_and_metadata_insertion() {
        let p = run(&["a.txt"], &[]);
        assert_eq!(p.summary(), "No item will be renamed");
        assert!(!p.can_rename());

        let mut item = items(&["song.mp3"]);
        item[0].metadata.insert("artist".into(), "Miles".into());
        let rule = Rule::new(RuleKind::AddMetadata { key: "artist".into(), position: Position::Beginning });
        assert_eq!(new_names(&run_with(&item, std::slice::from_ref(&rule), &|_, _| false)), vec!["Milessong.mp3"]);
        // No value: the name stays.
        assert_eq!(new_names(&run(&["other.mp3"], &[rule])), vec!["other.mp3"]);
    }

    #[test]
    fn presets_survive_serialisation_and_tolerate_missing_fields() {
        let preset = Preset {
            version: PRESET_VERSION,
            name: "Photos".into(),
            rules: vec![sequence(1, 3, Position::Beginning, "_"), replace("a", "b", false).on(Target::Extension)],
        };
        let json = serde_json::to_string(&preset).unwrap();
        assert_eq!(serde_json::from_str::<Preset>(&json).unwrap(), preset);
        // An older preset without the newer flags still loads, enabled.
        let old = r#"{"version":1,"name":"Old","rules":[{"kind":{"ChangeCase":{"case":"Lower"}}}]}"#;
        let loaded: Preset = serde_json::from_str(old).unwrap();
        assert!(loaded.rules[0].enabled && loaded.rules[0].files && loaded.rules[0].folders);
        assert_eq!(loaded.rules[0].target, Target::Name);
    }
}
