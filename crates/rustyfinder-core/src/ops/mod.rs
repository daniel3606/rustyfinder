//! File operations: copy, move, delete, duplicate, rename, new items.
//!
//! - `names`    naming rules (duplicates, keep-both, validation)
//! - `engine`   the transfer and delete engine with conflict handling
//! - `activity` the queue of running operations, undo/redo and the log
//! - `simple`   one-step operations: rename, new folder, new file, symlink
//! - `privileged` retrying a step with administrator rights
//! - `throttle` bandwidth limits shared by running transfers
//! - `partials` partial files of stopped transfers, for Resume

pub mod activity;
pub mod engine;
pub mod names;
pub mod partials;
pub mod privileged;
pub mod simple;
pub mod throttle;

#[cfg(test)]
mod activity_tests;
#[cfg(test)]
mod engine_tests;
#[cfg(test)]
mod transfer_tests;

pub use activity::{
    ActivityId, ActivityInfo, ActivityManager, ActivityObserver, ActivityResult, ActivityState, FinishReport, LogEntry,
    Platform, Prompt, Request, SyncStep,
};
pub use engine::{
    ConflictAnswer, ConflictChoice, ConflictInfo, ConflictKind, ConflictPolicy, Effect, Engine, ErrorChoice, ErrorInfo,
    ErrorPolicy, OpContext, Outcome, Policies, Privileged, Progress, ResumePolicy, TransferMode,
};
pub use partials::{PartialRecord, Partials};
pub use throttle::RateLimiter;
