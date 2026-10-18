//! Steps the uninstall queue runs before the leftover scan: a System Restore point and the
//! program's own (vendor) uninstaller.

pub mod orchestrator;
pub mod restore_point;
pub mod runner;

pub use orchestrator::{run_queue, QueueItem, QueueOptions, UninstallEvent, UninstallSources};
pub use restore_point::{FakeRestorePoint, RestorePointApi, SrClient};
pub use runner::{
    run_vendor_uninstaller, still_installed, uninstall_command, FakeProcessRunner, JobObjectRunner, ProcessRunner,
};

use serde::{Deserialize, Serialize};

/// `Normal` runs the vendor uninstaller first; `Forced` skips it and only removes leftovers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UninstallMode {
    Normal,
    Forced,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RestorePointStatus {
    /// Sequence number of the new restore point.
    Created(i64),
    /// Windows declined because a restore point was created recently (24 h rule).
    SkippedFrequency,
    /// System Restore is turned off.
    Disabled,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum VendorResult {
    Uninstalled { exit_code: i32 },
    /// The uninstaller failed to start (`None`) or ran but the program is still there.
    NotUninstalled { exit_code: Option<i32> },
    NoUninstaller,
}
