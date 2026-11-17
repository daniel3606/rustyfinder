//! Finder tags: the rules for changing an item's tag list.
//!
//! Reading and writing the tags themselves is the system's job (the app
//! shell does it through Foundation, so Finder and Spotlight agree). What
//! lives here is the list logic: no duplicates, toggling over a selection,
//! renaming, completion. Tag names compare without regard to case, as in
//! Finder, and keep the spelling they were first given.

/// The seven colour labels in Finder's numbering (index 0 is "no colour").
pub const COLORS: [&str; 8] = ["None", "Gray", "Green", "Purple", "Blue", "Yellow", "Red", "Orange"];

fn same(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

pub fn has(tags: &[String], tag: &str) -> bool {
    tags.iter().any(|t| same(t, tag))
}

/// `tags` with `tag` added at the end, unless it is already there.
pub fn add(tags: &[String], tag: &str) -> Vec<String> {
    let tag = tag.trim();
    let mut out = tags.to_vec();
    if !tag.is_empty() && !has(tags, tag) {
        out.push(tag.to_string());
    }
    out
}

pub fn remove(tags: &[String], tag: &str) -> Vec<String> {
    tags.iter().filter(|t| !same(t, tag)).cloned().collect()
}

/// Toggle `tag` over a selection: if every item has it, it comes off all
/// of them; otherwise it goes on those that lack it. Returns the new tag
/// list of each item, in the same order.
pub fn toggle(items: &[Vec<String>], tag: &str) -> Vec<Vec<String>> {
    let all_have = !items.is_empty() && items.iter().all(|tags| has(tags, tag));
    items.iter().map(|tags| if all_have { remove(tags, tag) } else { add(tags, tag) }).collect()
}

/// The tags every item of a selection has, in the first item's order.
pub fn common(items: &[Vec<String>]) -> Vec<String> {
    match items.split_first() {
        None => Vec::new(),
        Some((first, rest)) => first.iter().filter(|tag| rest.iter().all(|tags| has(tags, tag))).cloned().collect(),
    }
}

/// Apply an edited list of common tags to a selection: tags that were
/// common and are gone come off every item, new ones go on every item.
/// Tags only some items had are left alone.
pub fn apply_common(items: &[Vec<String>], edited: &[String]) -> Vec<Vec<String>> {
    let before = common(items);
    let removed: Vec<&String> = before.iter().filter(|t| !has(edited, t)).collect();
    items
        .iter()
        .map(|tags| {
            let mut out: Vec<String> = tags.iter().filter(|t| !removed.iter().any(|r| same(r, t))).cloned().collect();
            for tag in edited {
                out = add(&out, tag);
            }
            out
        })
        .collect()
}

/// `tags` with `old` renamed to `new`, or `None` if `old` is not there.
/// If `new` is already present the two merge into one.
pub fn rename(tags: &[String], old: &str, new: &str) -> Option<Vec<String>> {
    if !has(tags, old) {
        return None;
    }
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let next = if same(tag, old) { new } else { tag };
        if !has(&out, next) {
            out.push(next.to_string());
        }
    }
    Some(out)
}

/// Known tags that start with what was typed, for the token field.
pub fn complete(known: &[String], typed: &str) -> Vec<String> {
    let typed = typed.trim().to_lowercase();
    if typed.is_empty() {
        return Vec::new();
    }
    known.iter().filter(|tag| tag.to_lowercase().starts_with(&typed)).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(tags: &[&str]) -> Vec<String> {
        tags.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn tg01_toggling_twice_ends_without_the_tag() {
        let once = toggle(&[list(&[])], "Red");
        assert_eq!(once, [list(&["Red"])]);
        assert_eq!(toggle(&once, "Red"), [list(&[])]);
    }

    #[test]
    fn tg03_adding_a_tag_twice_keeps_one() {
        assert_eq!(add(&list(&["Blue"]), "Blue"), list(&["Blue"]));
        assert_eq!(add(&list(&["Blue"]), "blue"), list(&["Blue"]), "case does not make a second tag");
        assert_eq!(add(&list(&["Blue"]), "  "), list(&["Blue"]));
        assert_eq!(add(&list(&["Blue"]), " Work "), list(&["Blue", "Work"]));
    }

    #[test]
    fn toggling_a_mixed_selection_adds_to_those_without() {
        let items = [list(&["Red", "Work"]), list(&["Work"]), list(&[])];
        let on = toggle(&items, "Red");
        assert_eq!(on, [list(&["Red", "Work"]), list(&["Work", "Red"]), list(&["Red"])]);
        let off = toggle(&on, "red");
        assert_eq!(off, [list(&["Work"]), list(&["Work"]), list(&[])]);
        assert!(toggle(&[], "Red").is_empty());
    }

    #[test]
    fn tg02_the_tags_field_shows_common_tags_and_edits_apply_to_all() {
        let items = [list(&["Red", "Work"]), list(&["Work", "Old"]), list(&["work"])];
        assert_eq!(common(&items), list(&["Work"]));
        // Add "Urgent", remove "Work": "Red" and "Old" stay where they were.
        let edited = apply_common(&items, &list(&["Urgent"]));
        assert_eq!(edited, [list(&["Red", "Urgent"]), list(&["Old", "Urgent"]), list(&["Urgent"])]);
        // Nothing edited: nothing changes.
        assert_eq!(apply_common(&items, &list(&["Work"])), items);
        assert!(common(&[]).is_empty());
    }

    #[test]
    fn tg05_renaming_a_tag_keeps_its_place_and_merges_duplicates() {
        assert_eq!(rename(&list(&["A", "Old", "B"]), "old", "Archive"), Some(list(&["A", "Archive", "B"])));
        assert_eq!(rename(&list(&["Old", "Archive"]), "Old", "Archive"), Some(list(&["Archive"])));
        assert_eq!(rename(&list(&["A"]), "Old", "Archive"), None);
    }

    #[test]
    fn tg02_completion_offers_tags_by_prefix() {
        let known = list(&["Work", "Workshop", "Home", "Red"]);
        assert_eq!(complete(&known, "Wo"), list(&["Work", "Workshop"]));
        assert_eq!(complete(&known, "wo"), list(&["Work", "Workshop"]));
        assert!(complete(&known, "").is_empty() && complete(&known, "x").is_empty());
    }

    #[test]
    fn colors_follow_finders_numbering() {
        assert_eq!(COLORS[6], "Red");
        assert_eq!(COLORS[2], "Green");
        assert_eq!(COLORS.len(), 8);
    }
}
