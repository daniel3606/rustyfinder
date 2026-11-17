//! "Group by": which group an item falls in, and the order of the groups.
//!
//! Dates are bucketed in the user's local time zone: Today, Yesterday,
//! Previous 7 Days, Previous 30 Days, then each earlier month of this year,
//! then each earlier year. Newest groups come first; size groups go from
//! small to large.

use serde::{Deserialize, Serialize};

/// What a list is grouped by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum GroupKey {
    #[default]
    None,
    Kind,
    Added,
    Modified,
    Created,
    Size,
    Tags,
}

/// One group header and the first row under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub title: String,
    pub start: u32,
    /// For Kind groups of files: the extension, so the app can name the kind.
    pub ext: Option<String>,
}

/// Where an item sorts among the groups: by `rank`, then by `label`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Slot {
    pub rank: u32,
    pub label: String,
}

impl Slot {
    fn new(rank: u32, label: impl Into<String>) -> Self {
        Slot { rank, label: label.into() }
    }
}

pub const FOLDERS: &str = "Folders";
pub const NO_TAGS: &str = "No Tags";
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

/// Date bucket for `ms`, given "now" and the local UTC offsets (seconds)
/// at both instants.
pub fn date_slot(ms: Option<i64>, now_ms: i64, offset_then: i64, offset_now: i64) -> Slot {
    let Some(ms) = ms else { return Slot::new(u32::MAX, "Unknown") };
    let day = (ms.div_euclid(1000) + offset_then).div_euclid(86_400);
    let today = (now_ms.div_euclid(1000) + offset_now).div_euclid(86_400);
    match today - day {
        d if d <= 0 => Slot::new(0, "Today"),
        1 => Slot::new(1, "Yesterday"),
        2..=7 => Slot::new(2, "Previous 7 Days"),
        8..=30 => Slot::new(3, "Previous 30 Days"),
        _ => {
            let (year, month) = civil(day);
            let (this_year, _) = civil(today);
            if year == this_year {
                Slot::new(100 + (12 - month) as u32, MONTHS[(month - 1) as usize])
            } else {
                Slot::new(1000 + (this_year - year).clamp(0, 100_000) as u32, year.to_string())
            }
        }
    }
}

/// Size bucket of a file; folders (no size of their own) get their own group.
pub fn size_slot(size: u64, is_dir: bool) -> Slot {
    const KB: u64 = 1000;
    if is_dir {
        return Slot::new(0, FOLDERS);
    }
    match size {
        s if s < 10 * KB => Slot::new(1, "Less than 10 KB"),
        s if s < KB * KB => Slot::new(2, "10 KB to 1 MB"),
        s if s < 100 * KB * KB => Slot::new(3, "1 MB to 100 MB"),
        s if s < 10 * KB * KB * KB => Slot::new(4, "100 MB to 10 GB"),
        _ => Slot::new(5, "More than 10 GB"),
    }
}

/// Kind group: folders first when they are kept on top, then one group per
/// file extension in alphabetical order. The label is the extension; the
/// app turns it into a kind name.
pub fn kind_slot(ext: &str, is_dir: bool, folders_first: bool) -> Slot {
    if is_dir {
        return Slot::new(
            if folders_first { 0 } else { 1 },
            if folders_first { String::new() } else { "folder".into() },
        );
    }
    Slot::new(1, ext)
}

/// Tag group: the item's first tag; untagged items last.
pub fn tag_slot(first_tag: Option<&str>) -> Slot {
    match first_tag {
        Some(tag) => Slot::new(0, crate::sort::fold(tag)),
        None => Slot::new(1, NO_TAGS),
    }
}

/// Year and month (1-12) of a day count since 1970-01-01.
fn civil(days: i64) -> (i64, i64) {
    // Howard Hinnant's days-to-civil algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month)
}

/// The local time zone's offset from UTC, in seconds, at `ms`.
pub fn local_offset(ms: i64) -> i64 {
    let secs = ms.div_euclid(1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return 0;
    }
    tm.tm_gmtoff as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600_000;
    const DAY: i64 = 24 * HOUR;
    /// 2026-11-16 12:00 UTC.
    const NOW: i64 = 1_794_830_400_000;

    fn label(ms: i64, offset: i64) -> String {
        date_slot(Some(ms), NOW, offset, offset).label
    }

    #[test]
    fn recent_buckets() {
        assert_eq!(label(NOW - HOUR, 0), "Today");
        assert_eq!(label(NOW - DAY, 0), "Yesterday");
        assert_eq!(label(NOW - 3 * DAY, 0), "Previous 7 Days");
        assert_eq!(label(NOW - 7 * DAY, 0), "Previous 7 Days");
        assert_eq!(label(NOW - 20 * DAY, 0), "Previous 30 Days");
        assert_eq!(label(NOW - 60 * DAY, 0), "September");
        assert_eq!(label(NOW - 400 * DAY, 0), "2025");
        assert_eq!(label(NOW + HOUR, 0), "Today");
        assert_eq!(date_slot(None, NOW, 0, 0).label, "Unknown");
    }

    /// VW-04: the day boundary is the local one, not UTC's.
    #[test]
    fn days_follow_the_local_time_zone() {
        // 2026-11-16 00:30 in UTC+9 is 2026-11-15 15:30 UTC.
        let half_past_midnight_tokyo = NOW - 12 * HOUR + 30 * 60_000 - 9 * HOUR;
        assert_eq!(label(half_past_midnight_tokyo, 9 * 3600), "Today");
        assert_eq!(label(half_past_midnight_tokyo, 0), "Yesterday");
        // 23:30 the evening before in UTC-5 is still "Yesterday" there.
        let late_new_york = NOW - 12 * HOUR - 30 * 60_000 + 5 * HOUR;
        assert_eq!(label(late_new_york, -5 * 3600), "Yesterday");
    }

    #[test]
    fn newer_groups_rank_first() {
        let slots: Vec<Slot> = [0, DAY, 3 * DAY, 20 * DAY, 60 * DAY, 90 * DAY, 400 * DAY, 800 * DAY]
            .iter()
            .map(|d| date_slot(Some(NOW - d), NOW, 0, 0))
            .collect();
        assert!(slots.windows(2).all(|w| w[0] < w[1]), "{slots:?}");
    }

    /// VW-05.
    #[test]
    fn size_buckets() {
        assert_eq!(size_slot(5_000, false).label, "Less than 10 KB");
        assert_eq!(size_slot(2_000_000_000, false).label, "100 MB to 10 GB");
        assert_eq!(size_slot(500_000, false).label, "10 KB to 1 MB");
        assert_eq!(size_slot(50_000_000, false).label, "1 MB to 100 MB");
        assert_eq!(size_slot(20_000_000_000, false).label, "More than 10 GB");
        assert!(size_slot(1, false) < size_slot(20_000_000_000, false));
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1));
        assert_eq!(civil(NOW / DAY), (2026, 11));
        assert_eq!(civil(-1), (1969, 12));
    }
}
