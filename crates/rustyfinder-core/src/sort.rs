//! Finder-like ordering: case-insensitive, digit runs compared as numbers.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// Column a listing can be sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Modified,
    Created,
    /// By extension, then name.
    Kind,
    /// Date the item was added to its folder.
    Added,
    /// First tag name; untagged items last.
    Tags,
    Owner,
    Group,
    /// Unix permission bits.
    Permissions,
}

impl SortKey {
    /// Dates and sizes read best newest / largest first.
    pub fn descending_first(self) -> bool {
        matches!(self, SortKey::Size | SortKey::Modified | SortKey::Created | SortKey::Added)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct SortSpec {
    pub key: SortKey,
    pub ascending: bool,
    /// Keep folders above files regardless of key and direction.
    pub folders_first: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        Self { key: SortKey::Name, ascending: true, folders_first: true }
    }
}

/// Fold a name for comparing and matching: decompose, drop accents, lower-case.
/// "É" (NFC), "É" (NFD) and "e" all fold to "e", so typing `e` finds `é`
/// and both Unicode forms of a name sort and match alike.
pub fn fold(name: &str) -> String {
    name.nfd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase).collect()
}

/// Compare two *folded* names so that "file2" sorts before "file10".
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < ab.len() && j < bb.len() {
        if ab[i].is_ascii_digit() && bb[j].is_ascii_digit() {
            let (si, sj) = (i, j);
            while i < ab.len() && ab[i].is_ascii_digit() {
                i += 1;
            }
            while j < bb.len() && bb[j].is_ascii_digit() {
                j += 1;
            }
            let da = trim_zeros(&ab[si..i]);
            let db = trim_zeros(&bb[sj..j]);
            // Longer digit run (sans leading zeros) is the bigger number.
            let ord = da.len().cmp(&db.len()).then_with(|| da.cmp(db));
            if ord != Ordering::Equal {
                return ord;
            }
        } else {
            // Compare whole characters, not bytes, so multi-byte text orders
            // by code point.
            let ca = a[i..].chars().next().unwrap();
            let cb = b[j..].chars().next().unwrap();
            if ca != cb {
                return ca.cmp(&cb);
            }
            i += ca.len_utf8();
            j += cb.len_utf8();
        }
    }
    (ab.len() - i).cmp(&(bb.len() - j))
}

fn trim_zeros(digits: &[u8]) -> &[u8] {
    let first = digits.iter().position(|d| *d != b'0').unwrap_or(digits.len());
    &digits[first..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut names: Vec<&str>) -> Vec<&str> {
        names.sort_by(|a, b| natural_cmp(&fold(a), &fold(b)));
        names
    }

    #[test]
    fn numbers_sort_numerically() {
        assert_eq!(sorted(vec!["file10", "file2", "file1", "file02x"]), vec!["file1", "file2", "file02x", "file10"]);
    }

    #[test]
    fn case_is_ignored() {
        assert_eq!(sorted(vec!["b", "A", "C", "a1"]), vec!["A", "a1", "b", "C"]);
    }

    #[test]
    fn nfc_and_nfd_fold_equal() {
        let nfc = "caf\u{e9}";
        let nfd = "cafe\u{301}";
        assert_ne!(nfc, nfd);
        assert_eq!(fold(nfc), fold(nfd));
        assert_eq!(natural_cmp(&fold(nfc), &fold(nfd)), Ordering::Equal);
    }

    #[test]
    fn accents_are_ignored_for_matching() {
        assert_eq!(fold("\u{c9}t\u{e9}"), "ete");
        assert_eq!(fold("na\u{ef}ve"), "naive");
        assert!(fold("r\u{e9}sum\u{e9}.pdf").starts_with(&fold("RESU")));
    }

    #[test]
    fn spec_vw01_natural_case_insensitive() {
        assert_eq!(sorted(vec!["a2.txt", "a10.txt", "A1.txt"]), vec!["A1.txt", "a2.txt", "a10.txt"]);
    }

    #[test]
    fn prefix_sorts_first() {
        assert_eq!(natural_cmp("abc", "abcd"), Ordering::Less);
        assert_eq!(natural_cmp("", "a"), Ordering::Less);
        assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
    }

    #[test]
    fn huge_digit_runs_do_not_overflow() {
        let a = format!("f{}", "9".repeat(60));
        let b = format!("f1{}", "0".repeat(60));
        assert_eq!(natural_cmp(&a, &b), Ordering::Less);
    }

    #[test]
    fn multibyte_names() {
        assert_eq!(sorted(vec!["日本", "abc", "Ünï"]), vec!["abc", "Ünï", "日本"]);
    }
}
