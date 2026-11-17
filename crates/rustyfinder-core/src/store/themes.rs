//! The user's own themes (copies of built-in ones, edited) and which
//! theme is in use.

use super::{read_json, same_name, write_json, FavoriteError, Store};
use crate::theme::{builtin_themes, Theme, DEFAULT_THEME};

impl Store {
    fn custom_themes(&self) -> Vec<Theme> {
        read_json::<Vec<Theme>>(&self.dir.join("themes.json"))
            .unwrap_or_default()
            .into_iter()
            .map(Theme::sanitized)
            .collect()
    }

    fn save_custom_themes(&self, themes: &[Theme]) {
        write_json(&self.dir.join("themes.json"), themes);
    }

    /// Built-in themes first, then the user's.
    pub fn themes(&self) -> Vec<Theme> {
        let mut all = builtin_themes();
        all.extend(self.custom_themes());
        all
    }

    /// The theme in use. One that was deleted reads as Default.
    pub fn current_theme(&self) -> Theme {
        let id = self.settings().theme;
        let all = self.themes();
        all.iter().find(|t| t.id == id).cloned().unwrap_or_else(|| all[0].clone())
    }

    pub fn select_theme(&self, id: &str) -> bool {
        if !self.themes().iter().any(|t| t.id == id) {
            return false;
        }
        self.update_settings(|s| s.theme = id.to_string());
        true
    }

    /// A copy of any theme, named "<name> Copy" (made unique), that can be
    /// edited.
    pub fn duplicate_theme(&self, id: &str) -> Option<Theme> {
        let source = self.themes().into_iter().find(|t| t.id == id)?;
        let mut custom = self.custom_themes();
        let all = self.themes();
        let base = format!("{} Copy", source.name);
        let name = std::iter::once(base.clone())
            .chain((2..).map(|n| format!("{base} {n}")))
            .find(|c| !all.iter().any(|t| same_name(&t.name, c)))
            .unwrap();
        let number = (1..).find(|n| !all.iter().any(|t| t.id == format!("custom-{n}"))).unwrap();
        let copy = Theme { id: format!("custom-{number}"), name, builtin: false, ..source };
        custom.push(copy.clone());
        self.save_custom_themes(&custom);
        Some(copy)
    }

    /// Store an edited custom theme (its name is kept as it was; see
    /// `rename_theme`). Built-in themes cannot be changed.
    pub fn save_theme(&self, theme: Theme) -> bool {
        let mut custom = self.custom_themes();
        let Some(slot) = custom.iter_mut().find(|t| t.id == theme.id) else { return false };
        let name = slot.name.clone();
        *slot = Theme { name, builtin: false, ..theme.sanitized() };
        self.save_custom_themes(&custom);
        true
    }

    pub fn rename_theme(&self, id: &str, name: &str) -> Result<(), FavoriteError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(FavoriteError::EmptyName);
        }
        if self.themes().iter().any(|t| t.id != id && same_name(&t.name, name)) {
            return Err(FavoriteError::NameTaken(name.to_string()));
        }
        let mut custom = self.custom_themes();
        let theme = custom.iter_mut().find(|t| t.id == id).ok_or(FavoriteError::Missing)?;
        theme.name = name.to_string();
        self.save_custom_themes(&custom);
        Ok(())
    }

    /// Delete a custom theme. If it was in use, Default takes over (SD-05).
    pub fn delete_theme(&self, id: &str) -> bool {
        let mut custom = self.custom_themes();
        let before = custom.len();
        custom.retain(|t| t.id != id);
        if custom.len() == before {
            return false;
        }
        self.save_custom_themes(&custom);
        if self.settings().theme == id {
            self.update_settings(|s| s.theme = DEFAULT_THEME.to_string());
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Appearance;

    fn store() -> (tempfile::TempDir, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path().join("support"), "/h");
        (tmp, store)
    }

    #[test]
    fn a_fresh_install_uses_the_default_theme() {
        let (_tmp, store) = store();
        assert_eq!(store.current_theme().id, DEFAULT_THEME);
        assert_eq!(store.current_theme().appearance, Appearance::System);
    }

    #[test]
    fn builtins_are_duplicated_then_edited_renamed_and_kept() {
        let (tmp, store) = store();
        assert!(!store.save_theme(Theme { id: "dark".into(), ..Theme::default() }), "built-ins are fixed");
        let copy = store.duplicate_theme("harbor").unwrap();
        assert_eq!((copy.name.as_str(), copy.builtin), ("Harbor Copy", false));
        assert_eq!(store.duplicate_theme("harbor").unwrap().name, "Harbor Copy 2");
        let mut edited = copy.clone();
        edited.set_color("background", "#000");
        edited.inactive_selection = "#123456".into();
        assert!(store.save_theme(edited));
        assert!(matches!(store.rename_theme(&copy.id, "dark"), Err(FavoriteError::NameTaken(_))));
        store.rename_theme(&copy.id, "Night").unwrap();
        assert!(store.select_theme(&copy.id));
        let again = Store::open(tmp.path().join("support"), "/h");
        let current = again.current_theme();
        assert_eq!((current.name.as_str(), current.background.as_str()), ("Night", "#000000"));
        assert_eq!(current.inactive_selection, "#123456");
        assert!(!again.select_theme("no-such-theme"));
    }

    #[test]
    fn sd05_deleting_the_theme_in_use_falls_back_to_default() {
        let (_tmp, store) = store();
        let copy = store.duplicate_theme(DEFAULT_THEME).unwrap();
        store.select_theme(&copy.id);
        assert!(!store.delete_theme("dark"), "built-ins cannot be deleted");
        assert!(store.delete_theme(&copy.id));
        assert_eq!(store.settings().theme, DEFAULT_THEME);
        assert_eq!(store.current_theme().id, DEFAULT_THEME);
        assert!(!store.delete_theme(&copy.id));
    }
}
