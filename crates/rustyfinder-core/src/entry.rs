/// What a directory entry is, without following symlinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    /// Sockets, FIFOs, devices.
    Other,
}

/// One item in a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// File name exactly as the backend reported it (no normalisation).
    pub name: String,
    pub kind: EntryKind,
    /// True for a directory, or a symlink that resolves to one.
    pub is_dir: bool,
    /// Size in bytes. 0 for directories (their size is computed on demand).
    pub size: u64,
    /// Milliseconds since the Unix epoch.
    pub modified_ms: Option<i64>,
    pub created_ms: Option<i64>,
    /// Dot-file, or flagged hidden by the file system.
    pub is_hidden: bool,
    /// Unix permission bits (lower 12 bits of st_mode), if the backend has them.
    pub mode: Option<u32>,
    /// Symlink destination as stored in the link, if this is a symlink.
    pub link_target: Option<String>,
    /// True for a symlink whose destination does not exist (or loops).
    pub is_broken_link: bool,
    /// Details only some backends know (the local disk knows all of them).
    pub ext: EntryExt,
}

/// The less common facts about an item. Kept apart so backends that know
/// none of them just write `ext: Default::default()`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EntryExt {
    /// When the item was put in its folder (macOS keeps this per item).
    pub added_ms: Option<i64>,
    /// Owner and group ids; names come from `owners::user_name` / `group_name`.
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    /// Finder tag names, in the order Finder lists them.
    pub tags: Vec<String>,
    /// Locked in Finder (the user-immutable flag).
    pub locked: bool,
    /// The content is only in the cloud (an iCloud Drive item not
    /// downloaded); reading it would start a download.
    pub cloud_only: bool,
    /// Owner and group names as a server reports them. Its ids mean
    /// nothing on this Mac, so these win over a local lookup of `uid`/`gid`.
    pub owner: Option<String>,
    pub group: Option<String>,
}

impl Entry {
    /// The owner's name: the server's, or the local user with that id.
    pub fn owner_name(&self) -> Option<String> {
        self.ext.owner.clone().or_else(|| self.ext.uid.map(crate::macmeta::user_name))
    }

    pub fn group_name(&self) -> Option<String> {
        self.ext.group.clone().or_else(|| self.ext.gid.map(crate::macmeta::group_name))
    }

    /// Lower-cased extension without the dot. Empty for folders, dot-files
    /// with no second dot (".zshrc") and names with no dot.
    pub fn extension(&self) -> String {
        if self.is_dir {
            return String::new();
        }
        extension_of(&self.name)
    }
}

/// Lower-cased extension of a file name, "" when there is none.
pub fn extension_of(name: &str) -> String {
    // A search result's name is a relative path: only its last part counts.
    let name = name.rsplit('/').next().unwrap_or(name);
    match name.rfind('.') {
        Some(0) | None => String::new(),
        Some(i) if i + 1 == name.len() => String::new(),
        Some(i) => name[i + 1..].to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_rules() {
        assert_eq!(extension_of("photo.JPG"), "jpg");
        assert_eq!(extension_of("archive.tar.gz"), "gz");
        assert_eq!(extension_of(".zshrc"), "");
        assert_eq!(extension_of("Makefile"), "");
        assert_eq!(extension_of("trailing."), "");
        assert_eq!(extension_of(".config.json"), "json");
        assert_eq!(extension_of("a.dir/readme"), "");
        assert_eq!(extension_of("a.dir/b.TXT"), "txt");
    }
}
