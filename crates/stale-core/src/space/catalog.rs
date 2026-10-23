//! The Windows space hogs that are listed although Windows owns them.

use super::{SpaceEntry, SpaceKind, WinItem};
use crate::fsys::FsReader;
use crate::pathutil::{drive_letter, norm};
use std::path::{Path, PathBuf};

fn entry(fs: &dyn FsReader, path: PathBuf, item: WinItem) -> Option<SpaceEntry> {
    (fs.exists(&path) && !fs.is_reparse(&path)).then(|| SpaceEntry {
        is_dir: fs.is_dir(&path),
        kind: SpaceKind::Windows(item),
        size: None,
        partial: false,
        modified: fs.modified(&path),
        note: None,
        path,
    })
}

/// The catalog paths that exist and are not links: each drive's `$Windows.~BT`, `$Windows.~WS` and
/// `$Windows.~TMP`, the system drive's `Windows.old`, then the dumps, update downloads and temp
/// folder below the Windows folder ([`FsReader::windows_dir`], never the environment), then each
/// profile's `AppData\Local\Temp`.
pub(super) fn catalog_entries(fs: &dyn FsReader, drives: &[PathBuf]) -> Vec<SpaceEntry> {
    let mut out = Vec::new();
    let root = fs.windows_dir();
    let system_drive = drive_letter(&root).map(|l| norm(Path::new(&format!(r"{l}:\"))));
    for drive in drives {
        if system_drive.as_deref() == Some(norm(drive).as_str()) {
            out.extend(entry(fs, drive.join("Windows.old"), WinItem::WindowsOld));
        }
        for (name, item) in [(r"$Windows.~BT", WinItem::WindowsBt), (r"$Windows.~WS", WinItem::WindowsWs), (r"$Windows.~TMP", WinItem::WindowsTmp)] {
            out.extend(entry(fs, drive.join(name), item));
        }
    }
    let rel: [(&Path, WinItem); 5] = [
        (Path::new(r"SoftwareDistribution\Download"), WinItem::UpdateDownload),
        (Path::new("Temp"), WinItem::WindowsTemp),
        (Path::new("MEMORY.DMP"), WinItem::MemoryDump),
        (Path::new("Minidump"), WinItem::Minidumps),
        (Path::new("LiveKernelReports"), WinItem::LiveKernelReports),
    ];
    for (rel, item) in rel {
        out.extend(entry(fs, root.join(rel), item));
    }
    for profile in fs.profile_dirs() {
        out.extend(entry(fs, profile.join(r"AppData\Local\Temp"), WinItem::UserTemp));
    }
    out
}
