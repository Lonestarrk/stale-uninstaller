//! The space scan behind "Sök rester": folders and files that no installed program owns, plus the
//! big Windows space hogs (`Windows.old` and its kind). [`list_entries`] lists without sizes;
//! [`measure`] sizes one entry, so the caller can stream the sizes.

mod catalog;
mod scan;

use crate::ghosts::RootKind;
use chrono::{DateTime, Utc};
use std::path::PathBuf;

pub use scan::{fixed_drives, list_entries, measure, measure_at, removal_items, USER_TEMP_MIN_AGE_DAYS};

/// What a space entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpaceKind {
    /// A folder in a program root that no installed program owns.
    Orphan(RootKind),
    /// A folder, or a file of at least 100 MB, at the top level of a drive.
    DriveRoot,
    /// A Windows-owned space hog.
    Windows(WinItem),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WinItem {
    WindowsOld,
    /// `$Windows.~BT`
    WindowsBt,
    /// `$Windows.~WS`
    WindowsWs,
    /// `$Windows.~TMP`
    WindowsTmp,
    /// `SoftwareDistribution\Download`
    UpdateDownload,
    /// `%SystemRoot%\Temp`
    WindowsTemp,
    /// One profile's `AppData\Local\Temp`; only files older than a week count.
    UserTemp,
    /// `MEMORY.DMP`
    MemoryDump,
    Minidumps,
    LiveKernelReports,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpaceEntry {
    pub path: PathBuf,
    pub is_dir: bool,
    pub kind: SpaceKind,
    /// `None` until measured.
    pub size: Option<u64>,
    /// The size is a lower bound: something below could not be read.
    pub partial: bool,
    pub modified: Option<DateTime<Utc>>,
    /// Language-free reason ([`crate::ghosts::group`]'s `REASON_*` codes) why the entry is
    /// ambiguous; the GUI words it.
    pub note: Option<&'static str>,
}

/// How an entry may be removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    /// Through the quarantine, like any leftover.
    Quarantine,
    /// Through Windows' own cleanup; never the quarantine.
    WindowsCleanup,
}

pub fn removal(kind: &SpaceKind) -> Removal {
    match kind {
        SpaceKind::Windows(WinItem::UserTemp | WinItem::MemoryDump | WinItem::Minidumps | WinItem::LiveKernelReports) => Removal::Quarantine,
        SpaceKind::Windows(_) => Removal::WindowsCleanup,
        SpaceKind::Orphan(_) | SpaceKind::DriveRoot => Removal::Quarantine,
    }
}
