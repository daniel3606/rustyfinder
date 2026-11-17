//! Partial files left by stopped transfers (conflicts spec §6).
//!
//! A smaller file at a target only counts as "partial" (and may be
//! resumed) if a transfer of that very source recorded it here when it
//! stopped. Anything else of the same name is an ordinary conflict. The
//! list is kept on disk so a resume also works after a relaunch.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Most records kept; the oldest go first.
const MAX_RECORDS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialRecord {
    /// `Vfs::id()` of the target's file system, and the target path.
    pub target_fs: String,
    pub target: String,
    /// The same for the source the bytes came from.
    pub source_fs: String,
    pub source: String,
    /// The source as it was: a resume continues only an unchanged source.
    pub source_size: u64,
    pub source_modified_ms: Option<i64>,
}

#[derive(Debug, Default)]
pub struct Partials {
    file: Option<PathBuf>,
    records: Mutex<Vec<PartialRecord>>,
}

impl Partials {
    /// Records that live only as long as this value (tests, previews).
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Records kept in `file` (JSON), loaded now.
    pub fn open(file: impl Into<PathBuf>) -> Self {
        let file = file.into();
        let records =
            std::fs::read(&file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Self { file: Some(file), records: Mutex::new(records) }
    }

    /// Remember that `record.target` holds the first part of `record.source`.
    pub fn record(&self, record: PartialRecord) {
        let mut records = self.records.lock().unwrap();
        records.retain(|r| !(r.target_fs == record.target_fs && r.target == record.target));
        records.push(record);
        let extra = records.len().saturating_sub(MAX_RECORDS);
        records.drain(..extra);
        self.save(&records);
    }

    pub fn find(&self, target_fs: &str, target: &str) -> Option<PartialRecord> {
        self.records.lock().unwrap().iter().find(|r| r.target_fs == target_fs && r.target == target).cloned()
    }

    /// The target is complete, replaced or gone: it is no longer partial.
    pub fn forget(&self, target_fs: &str, target: &str) {
        let mut records = self.records.lock().unwrap();
        let before = records.len();
        records.retain(|r| !(r.target_fs == target_fs && r.target == target));
        if records.len() != before {
            self.save(&records);
        }
    }

    fn save(&self, records: &[PartialRecord]) {
        let Some(file) = &self.file else { return };
        let Ok(bytes) = serde_json::to_vec_pretty(records) else { return };
        // Written beside and renamed over, so a crash never leaves half a file.
        let tmp = file.with_extension("json.tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(target: &str) -> PartialRecord {
        PartialRecord {
            target_fs: "local".into(),
            target: target.into(),
            source_fs: "sftp://t".into(),
            source: "/srv/big.iso".into(),
            source_size: 100,
            source_modified_ms: Some(5),
        }
    }

    #[test]
    fn records_survive_a_relaunch_and_can_be_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("partials.json");
        let partials = Partials::open(&file);
        partials.record(record("/dl/big.iso"));
        partials.record(record("/dl/big.iso")); // replaces, never duplicates
        assert_eq!(Partials::open(&file).find("local", "/dl/big.iso"), Some(record("/dl/big.iso")));
        assert_eq!(partials.find("sftp://t", "/dl/big.iso"), None, "another file system's path is another file");
        partials.forget("local", "/dl/big.iso");
        assert_eq!(Partials::open(&file).find("local", "/dl/big.iso"), None);
    }

    #[test]
    fn the_list_is_capped() {
        let partials = Partials::in_memory();
        for i in 0..MAX_RECORDS + 5 {
            partials.record(record(&format!("/dl/{i}")));
        }
        assert!(partials.find("local", "/dl/0").is_none());
        assert!(partials.find("local", &format!("/dl/{}", MAX_RECORDS + 4)).is_some());
    }
}
