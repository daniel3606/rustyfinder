//! rustyfinder-core: all file-manager logic, no UI.
//!
//! Layout:
//! - `vfs`      the backend trait every file system (local, SFTP, S3...) implements
//! - `local`    the local-disk backend
//! - `links`    AppleDouble companions; symlink targets with a time limit
//! - `entry`    one directory entry
//! - `snapshot` an immutable, sorted and filtered view of one directory
//! - `sort`     Finder-like natural ordering
//! - `paths`    slash-separated path helpers shared by all backends
//! - `registry` the open file systems: local disk plus remote connections
//! - `rename`   the multi-rename rules, preview and safe ordering
//! - `perm`     permission bits as text; measuring folders
//! - `fuzzy`    fuzzy matching (Quick Open) and patterns (Quick Select)
//! - `goto`     Go to Folder: typed paths, completion, destinations
//! - `history`  back/forward navigation stack
//! - `pane`     the per-tab controller that ties the above together
//! - `watch`    folder change notifications
//! - `window`   one window: two panes of tabs, and the rules between them
//! - `ops`      copy, move, delete and friends, with conflict handling
//! - `url`      parsing a connection URL pasted into the Connect panel
//! - `search`   finding items by name, extension, kind or content
//! - `archive`  archives as read-only folders; building new archives
//! - `tools`    user-defined shell commands run on the selection
//! - `checksum` MD5 and SHA digests of files
//! - `scm`      git status of the items in a folder
//! - `tags`     Finder tag lists: toggling, common tags, completion
//! - `shortcuts` key bindings over the defaults, with conflict detection
//! - `sync`     folder comparison: what a sync would add, update, delete
//! - `text`     guessing the encoding of a text file
//! - `store`    settings, favorites, recent folders, saved session
//! - `app_deleter` the files an app leaves in the Library folders
//! - `editors`  which application Edit (⌘E) uses for a file
//! - `store`    settings, favorites, recent folders, saved session, tag list
//! - `theme`    list and sidebar colours, light/dark
//! - `outside`  rustyfinder:// links and the Reveal service
//! - `volumes`  eject rules, share lists, privacy-protected folders

pub mod app_deleter;
pub mod archive;
mod archive_formats;
pub mod checksum;
pub mod compare_tools;
pub mod connections;
pub mod editors;
pub mod entry;
pub mod error;
pub mod fetch;
pub mod folder_views;
pub mod fuzzy;
pub mod goto;
pub mod group;
pub mod history;
pub mod links;
pub mod local;
pub mod macmeta;
pub mod netguard;
pub mod ops;
pub mod outside;
pub mod pane;
pub mod paths;
pub mod perm;
pub mod registry;
pub mod rename;
pub mod scm;
pub mod search;
pub mod shortcuts;
pub mod snapshot;
pub mod sort;
pub mod store;
pub mod sync;
pub mod tags;
pub mod text;
pub mod theme;
pub mod tools;
pub mod url;
pub mod vfs;
pub mod volumes;
pub mod watch;
pub mod window;

pub use entry::{Entry, EntryExt, EntryKind};
pub use error::{VfsError, VfsResult};
pub use group::{Group, GroupKey};
pub use history::History;
pub use local::LocalFs;
pub use ops::{
    ActivityInfo, ActivityManager, ActivityObserver, ActivityResult, ActivityState, ConflictAnswer, ConflictChoice,
    ConflictInfo, ConflictKind, ConflictPolicy, ErrorChoice, ErrorPolicy, FinishReport, LogEntry, Platform, Prompt,
    Request, TransferMode,
};
pub use pane::{PaneController, PaneObserver, PaneState, SearchStatus};
pub use registry::{ConnId, ConnInfo, Location, VfsRegistry, LOCAL};
pub use snapshot::{Layout, Snapshot, ViewSpec};
pub use sort::{SortKey, SortSpec};
pub use store::{Favorite, FavoriteError, FavoriteGroup, SavedWindow, Settings, Store};
pub use vfs::{Space, Vfs};
pub use window::{CloseOutcome, PaneTabs, Side, TabId, TabInfo, WindowInfo, WindowModel, WindowObserver};

/// Version of the core, shown in the About box and used to prove the FFI link.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
