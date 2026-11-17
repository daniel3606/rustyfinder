//! View Options (⌘J): per-folder view settings.

use crate::WindowSession;

/// Whether the folder in a tab has view settings of its own.
#[derive(Debug, Clone, Copy, uniffi::Record)]
pub struct ViewScope {
    /// The folder has its own settings (not inherited or default).
    pub own: bool,
    /// They also apply to every folder below.
    pub subfolders: bool,
}

#[uniffi::export]
impl WindowSession {
    /// Apply the tab's current view to its folder and every folder below.
    pub fn apply_view_to_subfolders(&self, tab: u64) {
        if let Some(c) = self.model.controller(tab) {
            c.apply_view_to_subfolders();
        }
    }

    /// Folders without settings of their own open with the tab's view.
    pub fn use_view_as_defaults(&self, tab: u64) {
        if let Some(c) = self.model.controller(tab) {
            c.use_view_as_defaults();
        }
    }

    /// Drop the folder's own view settings and show it with the defaults.
    pub fn restore_default_view(&self, tab: u64) {
        if let Some(c) = self.model.controller(tab) {
            c.restore_default_view();
        }
    }

    pub fn view_scope(&self, tab: u64) -> ViewScope {
        let (own, subfolders) = self.model.controller(tab).map_or((false, false), |c| c.view_scope());
        ViewScope { own, subfolders }
    }
}
