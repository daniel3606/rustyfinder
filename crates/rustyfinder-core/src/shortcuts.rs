//! Keyboard shortcuts: their text form, and the user's changes laid over
//! the defaults, with conflict detection.
//!
//! A binding is written as modifiers then a key, joined by `+`:
//! `cmd+shift+n`, `alt+cmd+delete`, `f5`, `ctrl+alt+t`. The empty string
//! means "no shortcut". Commands are named by their menu path
//! (`Commands/Copy to`).

use std::collections::BTreeMap;

/// Modifiers in their canonical order.
const MODIFIERS: [(&str, &str); 5] = [("fn", "🌐"), ("ctrl", "⌃"), ("alt", "⌥"), ("shift", "⇧"), ("cmd", "⌘")];

/// Named keys and how they are shown.
const KEYS: [(&str, &str); 16] = [
    ("delete", "⌫"),
    ("forwarddelete", "⌦"),
    ("return", "↩"),
    ("enter", "⌤"),
    ("tab", "⇥"),
    ("space", "Space"),
    ("escape", "⎋"),
    ("up", "↑"),
    ("down", "↓"),
    ("left", "←"),
    ("right", "→"),
    ("home", "↖"),
    ("end", "↘"),
    ("pageup", "⇞"),
    ("pagedown", "⇟"),
    ("plus", "+"),
];

fn modifier_alias(word: &str) -> Option<&'static str> {
    Some(match word {
        "ctrl" | "control" => "ctrl",
        "alt" | "opt" | "option" => "alt",
        "shift" => "shift",
        "cmd" | "command" => "cmd",
        "fn" | "globe" => "fn",
        _ => return None,
    })
}

fn is_key(word: &str) -> bool {
    if KEYS.iter().any(|(name, _)| *name == word) {
        return true;
    }
    if let Some(number) = word.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=20).contains(&number);
    }
    // One printable character that is not the separator.
    let mut chars = word.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_control() && !c.is_whitespace() && c != '+')
}

/// The canonical form of a binding typed or recorded in any order and
/// case (`Shift+CMD+N` gives `shift+cmd+n`). `Some("")` for no shortcut;
/// `None` if it is not a binding at all.
pub fn normalize(text: &str) -> Option<String> {
    let text = text.trim().to_lowercase();
    if text.is_empty() {
        return Some(String::new());
    }
    // A literal "+" key is written "plus" ("cmd++" is accepted too).
    let text = match text.strip_suffix("++") {
        Some(rest) => format!("{rest}+plus"),
        None if text == "+" => "plus".to_string(),
        None => text,
    };
    let mut words: Vec<&str> = text.split('+').collect();
    let key = words.pop()?;
    if !is_key(key) {
        return None;
    }
    let mut held = [false; MODIFIERS.len()];
    for word in words {
        let name = modifier_alias(word)?;
        let index = MODIFIERS.iter().position(|(m, _)| *m == name)?;
        if held[index] {
            return None;
        }
        held[index] = true;
    }
    let mut out = String::new();
    for (index, (name, _)) in MODIFIERS.iter().enumerate() {
        if held[index] {
            out.push_str(name);
            out.push('+');
        }
    }
    out.push_str(key);
    Some(out)
}

/// A binding as the menus show it: `⇧⌘N`, `⌥⌘⌫`, `F5`.
pub fn display(binding: &str) -> String {
    let Some(binding) = normalize(binding) else { return String::new() };
    if binding.is_empty() {
        return String::new();
    }
    let mut words: Vec<&str> = binding.split('+').collect();
    let key = words.pop().unwrap_or("");
    let mut out: String =
        words.iter().filter_map(|w| MODIFIERS.iter().find(|(m, _)| m == w).map(|(_, s)| *s)).collect();
    match KEYS.iter().find(|(name, _)| *name == key) {
        Some((_, symbol)) => out.push_str(symbol),
        None => out.push_str(&key.to_uppercase()),
    }
    out
}

/// True for a key with no modifier at all (Delete, F5, a letter). Such
/// bindings must not fire while text is being typed.
pub fn is_bare(binding: &str) -> bool {
    !binding.is_empty() && !binding.contains('+')
}

/// Another command already has the binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub other: String,
}

/// The shortcuts in force: the defaults, and the user's changes over them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bindings {
    defaults: BTreeMap<String, String>,
    /// Only what differs from the defaults ("" where a default was removed).
    custom: BTreeMap<String, String>,
}

impl Bindings {
    /// `defaults` are (command, binding) pairs; `custom` is what was saved.
    /// Saved changes for commands that no longer exist are dropped.
    pub fn new(defaults: impl IntoIterator<Item = (String, String)>, custom: BTreeMap<String, String>) -> Self {
        let defaults: BTreeMap<String, String> =
            defaults.into_iter().map(|(command, binding)| (command, normalize(&binding).unwrap_or_default())).collect();
        let custom = custom
            .into_iter()
            .filter_map(|(command, binding)| Some((command, normalize(&binding)?)))
            .filter(|(command, binding)| defaults.get(command).is_some_and(|d| d != binding))
            .collect();
        Self { defaults, custom }
    }

    /// The binding of `command` right now ("" for none).
    pub fn binding(&self, command: &str) -> &str {
        self.custom.get(command).or_else(|| self.defaults.get(command)).map_or("", String::as_str)
    }

    /// Every command with its binding, in name order.
    pub fn all(&self) -> Vec<(String, String)> {
        self.defaults.keys().map(|command| (command.clone(), self.binding(command).to_string())).collect()
    }

    /// The command that has `binding`, if any.
    pub fn owner(&self, binding: &str) -> Option<&str> {
        if binding.is_empty() {
            return None;
        }
        self.defaults.keys().find(|command| self.binding(command) == binding).map(String::as_str)
    }

    fn set(&mut self, command: &str, binding: String) {
        if self.defaults.get(command) == Some(&binding) {
            self.custom.remove(command);
        } else {
            self.custom.insert(command.to_string(), binding);
        }
    }

    /// Give `command` a binding ("" removes it). Refused, naming the
    /// holder, if another command has it; `reassign` takes it over.
    pub fn assign(&mut self, command: &str, binding: &str) -> Result<(), Conflict> {
        let Some(binding) = normalize(binding) else { return Ok(()) };
        if !self.defaults.contains_key(command) {
            return Ok(());
        }
        match self.owner(&binding) {
            Some(other) if other != command => Err(Conflict { other: other.to_string() }),
            _ => {
                self.set(command, binding);
                Ok(())
            }
        }
    }

    /// Give `command` the binding, taking it away from whoever has it.
    pub fn reassign(&mut self, command: &str, binding: &str) {
        let Some(binding) = normalize(binding) else { return };
        if !self.defaults.contains_key(command) {
            return;
        }
        if let Some(other) = self.owner(&binding).map(str::to_string) {
            if other != command {
                self.set(&other, String::new());
            }
        }
        self.set(command, binding);
    }

    /// Back to the built-in bindings for every command.
    pub fn restore_defaults(&mut self) {
        self.custom.clear();
    }

    /// The changes to save.
    pub fn custom(&self) -> &BTreeMap<String, String> {
        &self.custom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bindings() -> Bindings {
        let defaults = [
            ("File/New Folder", "cmd+shift+n"),
            ("File/New File", "alt+cmd+n"),
            ("Commands/Copy to", "f5"),
            ("Commands/Move to Trash", "cmd+delete"),
            ("Commands/Compare…", ""),
        ];
        Bindings::new(defaults.map(|(c, b)| (c.to_string(), b.to_string())), BTreeMap::new())
    }

    #[test]
    fn bindings_have_one_spelling() {
        assert_eq!(normalize("Shift+CMD+N").as_deref(), Some("shift+cmd+n"));
        assert_eq!(normalize("command+option+delete").as_deref(), Some("alt+cmd+delete"));
        assert_eq!(normalize("opt+ctrl+t").as_deref(), Some("ctrl+alt+t"));
        assert_eq!(normalize("F5").as_deref(), Some("f5"));
        assert_eq!(normalize("delete").as_deref(), Some("delete"));
        assert_eq!(normalize("cmd++").as_deref(), Some("cmd+plus"));
        // The Globe key is a modifier too (macOS binds Full Screen to it).
        assert_eq!(normalize("Fn+F").as_deref(), Some("fn+f"));
        assert_eq!(display("fn+f"), "🌐F");
        assert!(!is_bare("fn+f"));
        assert_eq!(normalize("cmd+,").as_deref(), Some("cmd+,"));
        assert_eq!(normalize("  ").as_deref(), Some(""));
        // Not bindings.
        assert_eq!(normalize("cmd+shift"), None);
        assert_eq!(normalize("cmd+cmd+n"), None);
        assert_eq!(normalize("hyper+n"), None);
        assert_eq!(normalize("cmd+f99"), None);
        assert_eq!(normalize("cmd+ab"), None);
    }

    #[test]
    fn bindings_are_shown_with_the_usual_symbols() {
        assert_eq!(display("cmd+shift+n"), "⇧⌘N");
        assert_eq!(display("alt+cmd+delete"), "⌥⌘⌫");
        assert_eq!(display("ctrl+alt+t"), "⌃⌥T");
        assert_eq!(display("f5"), "F5");
        assert_eq!(display("cmd+up"), "⌘↑");
        assert_eq!(display("cmd+plus"), "⌘+");
        assert_eq!(display(""), "");
        assert!(is_bare("delete") && is_bare("f5") && !is_bare("cmd+delete") && !is_bare(""));
    }

    #[test]
    fn ks02_a_taken_binding_is_reported_and_reassign_moves_it() {
        let mut b = bindings();
        // New File takes over Cmd-Shift-N: that is New Folder's.
        assert_eq!(b.assign("File/New File", "cmd+shift+n"), Err(Conflict { other: "File/New Folder".into() }));
        assert_eq!(b.binding("File/New File"), "alt+cmd+n", "nothing changed yet");
        b.reassign("File/New File", "shift+cmd+n");
        assert_eq!(b.binding("File/New File"), "shift+cmd+n");
        assert_eq!(b.binding("File/New Folder"), "", "the old holder lost it");
        // Now binding New Folder to it again is the conflict the spec describes.
        assert_eq!(b.assign("File/New Folder", "cmd+shift+n"), Err(Conflict { other: "File/New File".into() }));
        b.reassign("File/New Folder", "cmd+shift+n");
        assert_eq!(b.binding("File/New Folder"), "shift+cmd+n");
        assert_eq!(b.binding("File/New File"), "");
    }

    #[test]
    fn assigning_clearing_and_rebinding_to_itself() {
        let mut b = bindings();
        assert_eq!(b.assign("Commands/Compare…", "ctrl+c"), Ok(()));
        assert_eq!(b.owner("ctrl+c"), Some("Commands/Compare…"));
        // Its own binding again is not a conflict.
        assert_eq!(b.assign("Commands/Compare…", "ctrl+c"), Ok(()));
        // KS-03: an Option binding for Move to Trash is just a binding.
        assert_eq!(b.assign("Commands/Move to Trash", "alt+delete"), Ok(()));
        assert_eq!(b.binding("Commands/Move to Trash"), "alt+delete");
        // Removing a shortcut never conflicts, however many have none.
        assert_eq!(b.assign("Commands/Copy to", ""), Ok(()));
        assert_eq!(b.binding("Commands/Copy to"), "");
        assert_eq!(b.owner(""), None);
        // Unknown commands and nonsense are ignored.
        assert_eq!(b.assign("Nope/Nothing", "cmd+q"), Ok(()));
        assert_eq!(b.assign("Commands/Copy to", "not a key"), Ok(()));
        assert_eq!(b.binding("Commands/Copy to"), "");
    }

    #[test]
    fn only_differences_are_saved_and_ks07_restore_gives_the_defaults_back() {
        let mut b = bindings();
        assert!(b.custom().is_empty());
        b.reassign("Commands/Copy to", "cmd+shift+n");
        assert_eq!(b.custom().len(), 2, "the new binding and the one that was taken away");
        // Setting a command back to its default drops the saved change.
        b.reassign("File/New Folder", "cmd+shift+n");
        b.reassign("Commands/Copy to", "f5");
        assert!(b.custom().is_empty());

        b.reassign("Commands/Copy to", "f9");
        let saved = b.custom().clone();
        let again = Bindings::new(bindings().all(), saved);
        assert_eq!(again.binding("Commands/Copy to"), "f9");

        b.restore_defaults();
        assert_eq!(b, bindings());
        assert_eq!(b.binding("Commands/Copy to"), "f5");
    }

    #[test]
    fn saved_changes_for_commands_that_are_gone_are_dropped() {
        let saved = BTreeMap::from([
            ("Old/Removed Command".to_string(), "cmd+j".to_string()),
            ("Commands/Copy to".to_string(), "F9".to_string()),
            ("File/New File".to_string(), "garbage".to_string()),
        ]);
        let b = Bindings::new(bindings().all(), saved);
        assert_eq!(b.custom().len(), 1);
        assert_eq!(b.binding("Commands/Copy to"), "f9");
        assert_eq!(b.binding("File/New File"), "alt+cmd+n");
        assert_eq!(b.all().len(), 5);
    }
}
