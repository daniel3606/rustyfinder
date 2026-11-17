//! Themes: the colours of lists and the sidebar, and whether windows are
//! light, dark, or follow macOS. A colour left empty means "the system's
//! own", so the Default theme is all system colours and follows the
//! appearance and accent colour macOS has.

use serde::{Deserialize, Serialize};

/// The theme a fresh install uses: macOS's own colours.
pub const DEFAULT_THEME: &str = "default";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Appearance {
    /// Light or dark as macOS is set, switching when it does.
    #[default]
    System,
    Light,
    Dark,
}

/// One theme. Colours are "#RRGGBB" or empty for the system colour.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub id: String,
    pub name: String,
    /// Built-in themes can be duplicated but not edited or deleted.
    #[serde(skip)]
    pub builtin: bool,
    /// Light, dark or following macOS (the editor's "dark style").
    pub appearance: Appearance,
    /// Tint for controls and the active-pane line; empty: the accent colour.
    pub base: String,
    pub background: String,
    pub alternate_background: String,
    pub text: String,
    pub secondary_text: String,
    /// Selected items in the pane that has keyboard focus.
    pub selection: String,
    /// Selected items in the other pane.
    pub inactive_selection: String,
    pub sidebar_background: String,
    pub sidebar_text: String,
    pub sidebar_selection: String,
}

impl Theme {
    /// The colour fields, by name, for the editor and for validation.
    pub const COLOR_FIELDS: [&'static str; 10] = [
        "base",
        "background",
        "alternate_background",
        "text",
        "secondary_text",
        "selection",
        "inactive_selection",
        "sidebar_background",
        "sidebar_text",
        "sidebar_selection",
    ];

    pub fn color(&self, field: &str) -> Option<&str> {
        Some(match field {
            "base" => &self.base,
            "background" => &self.background,
            "alternate_background" => &self.alternate_background,
            "text" => &self.text,
            "secondary_text" => &self.secondary_text,
            "selection" => &self.selection,
            "inactive_selection" => &self.inactive_selection,
            "sidebar_background" => &self.sidebar_background,
            "sidebar_text" => &self.sidebar_text,
            "sidebar_selection" => &self.sidebar_selection,
            _ => return None,
        })
    }

    pub fn set_color(&mut self, field: &str, value: &str) -> bool {
        let value = normalize_color(value);
        let Some(value) = value else { return false };
        let slot = match field {
            "base" => &mut self.base,
            "background" => &mut self.background,
            "alternate_background" => &mut self.alternate_background,
            "text" => &mut self.text,
            "secondary_text" => &mut self.secondary_text,
            "selection" => &mut self.selection,
            "inactive_selection" => &mut self.inactive_selection,
            "sidebar_background" => &mut self.sidebar_background,
            "sidebar_text" => &mut self.sidebar_text,
            "sidebar_selection" => &mut self.sidebar_selection,
            _ => return false,
        };
        *slot = value;
        true
    }

    /// Bad colours become the system colour rather than breaking a window.
    pub fn sanitized(mut self) -> Self {
        for field in Self::COLOR_FIELDS {
            let value = self.color(field).unwrap_or_default().to_string();
            if normalize_color(&value).is_none() {
                self.set_color(field, "");
            }
        }
        self
    }
}

/// "#rgb", "rrggbb" or "#RRGGBB" as "#RRGGBB"; "" stays "" (system).
pub fn normalize_color(text: &str) -> Option<String> {
    let hex = text.trim().trim_start_matches('#');
    if hex.is_empty() {
        return Some(String::new());
    }
    let full: String = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 => hex.to_string(),
        _ => return None,
    };
    full.chars().all(|c| c.is_ascii_hexdigit()).then(|| format!("#{}", full.to_ascii_uppercase()))
}

fn palette(id: &str, name: &str, appearance: Appearance, c: [&str; 10]) -> Theme {
    Theme {
        id: id.into(),
        name: name.into(),
        builtin: true,
        appearance,
        base: c[0].into(),
        background: c[1].into(),
        alternate_background: c[2].into(),
        text: c[3].into(),
        secondary_text: c[4].into(),
        selection: c[5].into(),
        inactive_selection: c[6].into(),
        sidebar_background: c[7].into(),
        sidebar_text: c[8].into(),
        sidebar_selection: c[9].into(),
    }
}

/// The themes that come with RustyFinder. The colour themes are
/// RustyFinder's own palettes.
pub fn builtin_themes() -> Vec<Theme> {
    let system = |id: &str, name: &str, appearance| Theme {
        id: id.into(),
        name: name.into(),
        builtin: true,
        appearance,
        ..Theme::default()
    };
    vec![
        system(DEFAULT_THEME, "Default", Appearance::System),
        system("light", "Light", Appearance::Light),
        system("dark", "Dark", Appearance::Dark),
        palette(
            "harbor",
            "Harbor",
            Appearance::Dark,
            [
                "#4FA3C7", "#14202B", "#192836", "#DCE6EE", "#8A9BAA", "#2D6A8C", "#2A3A48", "#101A23", "#C9D6E0",
                "#24506A",
            ],
        ),
        palette(
            "meadow",
            "Meadow",
            Appearance::Light,
            [
                "#3E8E5A", "#F7FAF5", "#EDF3E8", "#1F2A1F", "#5F6E5F", "#B9DDBF", "#DCE5D8", "#E8EFE3", "#2B3A2B",
                "#A7D1AE",
            ],
        ),
        palette(
            "ember",
            "Ember",
            Appearance::Dark,
            [
                "#E07B39", "#1E1814", "#261E19", "#EEE2D6", "#A8968A", "#7A3E1C", "#3A2E27", "#17120F", "#E2D2C4",
                "#5E3218",
            ],
        ),
        palette(
            "sandstone",
            "Sandstone",
            Appearance::Light,
            [
                "#B5703A", "#FBF6EF", "#F3EBDF", "#3A2E24", "#7A6A5C", "#EBCFAF", "#E6DCCF", "#F1E8DB", "#3A2E24",
                "#E3C29C",
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_normalised_or_refused() {
        assert_eq!(normalize_color("#abc").as_deref(), Some("#AABBCC"));
        assert_eq!(normalize_color("12ab9f").as_deref(), Some("#12AB9F"));
        assert_eq!(normalize_color("").as_deref(), Some(""));
        assert_eq!(normalize_color("#12345"), None);
        assert_eq!(normalize_color("zzzzzz"), None);
    }

    #[test]
    fn builtins_have_unique_ids_valid_colours_and_default_follows_macos() {
        let all = builtin_themes();
        let mut ids: Vec<&str> = all.iter().map(|t| t.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
        for theme in &all {
            assert!(theme.builtin);
            for field in Theme::COLOR_FIELDS {
                let value = theme.color(field).unwrap();
                assert_eq!(normalize_color(value).as_deref(), Some(value), "{} {field}", theme.id);
            }
        }
        let default = &all[0];
        assert_eq!((default.id.as_str(), default.appearance), (DEFAULT_THEME, Appearance::System));
        assert!(Theme::COLOR_FIELDS.iter().all(|f| default.color(f).unwrap().is_empty()));
    }

    #[test]
    fn sanitizing_drops_bad_colours() {
        let mut theme = Theme { background: "#nothex".into(), text: "#fff".into(), ..Theme::default() };
        theme = theme.sanitized();
        assert_eq!((theme.background.as_str(), theme.text.as_str()), ("", "#fff"));
        assert!(theme.set_color("text", "#000"));
        assert_eq!(theme.text, "#000000");
        assert!(!theme.set_color("nope", "#000") && !theme.set_color("text", "bag"));
    }
}
