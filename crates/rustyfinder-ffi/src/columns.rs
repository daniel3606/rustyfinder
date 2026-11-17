//! Listings for Column view's other columns: the tab's pane only holds the
//! folder in focus, the ancestor and preview columns are listed here.

use std::sync::Arc;

use rustyfinder_core::Snapshot;

use crate::{RustyFinderApp, RustyFinderError, SnapshotHandle, ViewOptions};

#[uniffi::export]
impl RustyFinderApp {
    /// List `path` on `connection` into a sorted snapshot. Blocking: call
    /// it off the main thread.
    pub fn list_folder(
        &self,
        connection: u64,
        path: String,
        view: ViewOptions,
    ) -> Result<Arc<SnapshotHandle>, RustyFinderError> {
        let vfs = self
            .registry
            .get(connection)
            .ok_or_else(|| RustyFinderError::Io { message: "The connection is closed.".into() })?;
        let entries = vfs.list(&path)?;
        let mut view: rustyfinder_core::ViewSpec = view.into();
        view.filter.clear();
        Ok(Arc::new(SnapshotHandle { inner: Arc::new(Snapshot::listed(path, entries, view, Some(vfs))) }))
    }
}
