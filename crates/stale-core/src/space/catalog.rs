//! The Windows space hogs that are listed although Windows owns them.

use super::{SpaceEntry, SpaceKind, WinItem};
use crate::fsys::FsReader;
use crate::pathutil::drive_letter;
use std::path::{Path, PathBuf};

fn entry(fs: &dyn FsReader, path: PathBuf, item: WinItem) -> Option<SpaceEntry> {
    fs.exists(&path).then(|| SpaceEntry {
        is_dir: fs.is_dir(&path),
        kind: SpaceKind::Windows(item),
        size: None,
        partial: false,
        modified: fs.modified(&path),
        path,
    })
}

/// The catalog paths that exist: the system drive's `Windows.old`, `$Windows.~BT` and
/// `$Windows.~WS`, then the dumps, update downloads and temp folder below `%SystemRoot%`, then
/// each profile's `AppData\Local\Temp`.
pub(super) fn catalog_entries(fs: &dyn FsReader) -> Vec<SpaceEntry> {
    let mut out = Vec::new();
    if let Some(root) = fs.env("SystemRoot").map(PathBuf::from).filter(|r| !r.as_os_str().is_empty()) {
        if let Some(letter) = drive_letter(&root) {
            let drive = PathBuf::from(format!(r"{letter}:\"));
            for (name, item) in [("Windows.old", WinItem::WindowsOld), (r"$Windows.~BT", WinItem::WindowsBt), (r"$Windows.~WS", WinItem::WindowsWs)] {
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
    }
    for profile in fs.profile_dirs() {
        out.extend(entry(fs, profile.join(r"AppData\Local\Temp"), WinItem::UserTemp));
    }
    out
}
