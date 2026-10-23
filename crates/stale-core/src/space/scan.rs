//! Listing and measuring space entries.

use super::catalog::catalog_entries;
use super::{SpaceEntry, SpaceKind, WinItem};
use crate::fsys::FsReader;
use crate::ghosts::folders::list_orphan_folders;
use crate::ghosts::InstalledIndex;
use crate::keep::KeepList;
use crate::leftovers::fs_scan::FsScanRoots;
use crate::own::OwnPaths;
use crate::pathutil::{is_shared_root, norm};
use chrono::{DateTime, Duration, Utc};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Drive-root files below this size are not listed.
const MIN_ROOT_FILE_BYTES: u64 = 100 * 1024 * 1024;

/// Only files older than this many days count in (and go from) a profile's temp folder.
pub const USER_TEMP_MIN_AGE_DAYS: i64 = 7;

/// Top-level names (lowercase) that are never listed on a drive root. The Windows space hogs are
/// listed through the catalog instead.
const NEVER_LISTED: &[&str] = &[
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "users",
    "$recycle.bin",
    "system volume information",
    "recovery",
    "boot",
    "efi",
    "documents and settings",
    "perflogs",
    "pagefile.sys",
    "hiberfil.sys",
    "swapfile.sys",
    "windows.old",
    "$windows.~bt",
    "$windows.~ws",
];

fn never_listed(name: &str) -> bool {
    let lower = name.to_lowercase();
    NEVER_LISTED.contains(&lower.as_str()) || lower.starts_with("dumpstack.log")
}

/// The entries of the space scan, without sizes: orphan folders of the program roots, unowned
/// folders and big files at the top of each drive, and the Windows catalog paths that exist. Each
/// path appears once (the first source wins); Stale's own folders, kept paths and folders that
/// hold an installed program are left out.
pub fn list_entries(
    fs: &dyn FsReader,
    drives: &[PathBuf],
    roots: &FsScanRoots,
    installed: &InstalledIndex,
    keep: &KeepList,
    own: &OwnPaths,
) -> Vec<SpaceEntry> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<SpaceEntry> = Vec::new();
    let mut push = |path: PathBuf, is_dir: bool, kind: SpaceKind| {
        if seen.insert(norm(&path)) {
            let modified = fs.modified(&path);
            out.push(SpaceEntry { path, is_dir, kind, size: None, partial: false, modified });
        }
    };
    for o in list_orphan_folders(fs, roots, installed, keep) {
        if !own.is_own_path(&o.path) {
            push(o.path, true, SpaceKind::Orphan(o.root_kind));
        }
    }
    for drive in drives {
        for e in fs.list_dir(drive) {
            let name = e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if never_listed(&name)
                || is_shared_root(&e.path)
                || own.is_own_path(&e.path)
                || keep.is_path_kept(&e.path)
                || installed.covers_path(&e.path)
                || (e.is_dir && installed.holds_location(&e.path))
                || (!e.is_dir && e.size < MIN_ROOT_FILE_BYTES)
            {
                continue;
            }
            push(e.path, e.is_dir, SpaceKind::DriveRoot);
        }
    }
    for e in catalog_entries(fs) {
        push(e.path, e.is_dir, e.kind);
    }
    out
}

/// Size of the files under `dir` last modified before `cutoff`. A file with an unknown time is
/// not counted. Links are not followed (the reader lists them as files).
fn size_older_than(fs: &dyn FsReader, dir: &Path, cutoff: DateTime<Utc>) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs.list_dir(&d) {
            if e.is_dir {
                stack.push(e.path);
            } else if fs.modified(&e.path).is_some_and(|m| m < cutoff) {
                total += e.size;
            }
        }
    }
    total
}

/// The size of `entry` and whether it is only a lower bound. A profile's temp folder counts only
/// files older than [`USER_TEMP_MIN_AGE_DAYS`] days.
pub fn measure(fs: &dyn FsReader, entry: &SpaceEntry) -> (Option<u64>, bool) {
    measure_at(fs, entry, Utc::now())
}

/// [`measure`] with `now` given, for the age filter.
pub fn measure_at(fs: &dyn FsReader, entry: &SpaceEntry, now: DateTime<Utc>) -> (Option<u64>, bool) {
    if !entry.is_dir {
        let size = entry
            .path
            .parent()
            .and_then(|p| fs.list_dir(p).into_iter().find(|e| norm(&e.path) == norm(&entry.path)))
            .map(|e| e.size);
        return (size, false);
    }
    if entry.kind == SpaceKind::Windows(WinItem::UserTemp) {
        if !fs.is_dir(&entry.path) {
            return (None, false);
        }
        return (Some(size_older_than(fs, &entry.path, now - Duration::days(USER_TEMP_MIN_AGE_DAYS))), false);
    }
    match fs.dir_size_partial(&entry.path) {
        Some((size, partial)) => (Some(size), partial),
        None => (None, false),
    }
}

/// The fixed drives (`C:\`, `D:\`, ...); removable, network and optical drives are left out.
pub fn fixed_drives() -> Vec<PathBuf> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    const DRIVE_FIXED: u32 = 3;
    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!(r"{}:\", char::from(b'A' + i)))
        .filter(|root| {
            let w = crate::pathutil::wide(root);
            // SAFETY: `w` is NUL-terminated and outlives the call.
            unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) == DRIVE_FIXED }
        })
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsys::FakeFs;
    use crate::ghosts::RootKind;
    use crate::model::{Hive, Program, ProgramId, RegKeyPath, Scope};
    use crate::space::{removal, Removal};

    const MB: u64 = 1024 * 1024;

    fn base() -> FakeFs {
        FakeFs::new()
            .dir(r"C:\Program Files", 0)
            .dir(r"C:\Program Files (x86)", 0)
            .env("ProgramFiles", r"C:\Program Files")
            .env("ProgramFiles(x86)", r"C:\Program Files (x86)")
            .env("ProgramData", r"C:\ProgramData")
            .env("SystemRoot", r"C:\Windows")
    }

    fn drives() -> Vec<PathBuf> {
        vec![PathBuf::from(r"C:\"), PathBuf::from(r"D:\")]
    }

    fn list_with(fs: &FakeFs, installed: &InstalledIndex, keep: &KeepList, own: &OwnPaths) -> Vec<SpaceEntry> {
        list_entries(fs, &drives(), &FsScanRoots::from_env(fs), installed, keep, own)
    }

    fn list(fs: &FakeFs) -> Vec<SpaceEntry> {
        list_with(fs, &InstalledIndex::default(), &KeepList::default(), &OwnPaths::default())
    }

    fn paths(v: &[SpaceEntry]) -> Vec<String> {
        let mut p: Vec<String> = v.iter().map(|e| e.path.display().to_string()).collect();
        p.sort();
        p
    }

    fn program(name: &str, install: &str) -> Program {
        Program {
            account: None,
            id: ProgramId(name.into()),
            key: RegKeyPath { hive: Hive::LocalMachine, path: format!(r"Software\{name}") },
            name: name.into(),
            publisher: None,
            version: None,
            install_location: Some(PathBuf::from(install)),
            install_date: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            display_icon: None,
            is_msi: false,
            estimated_size_bytes: None,
            drive: None,
            hidden: false,
            scope: Scope::Machine,
        }
    }

    #[test]
    fn drive_root_lists_unknown_folders_and_big_files_only() {
        let fs = base()
            .dir(r"D:\Foton", 200)
            .file(r"D:\big.iso", 150 * MB)
            .file(r"D:\small.txt", 5 * MB)
            .file(r"D:\Foton\x.jpg", 1);
        let got = list(&fs);
        assert_eq!(paths(&got), vec![r"D:\Foton", r"D:\big.iso"]);
        assert!(got.iter().all(|e| e.kind == SpaceKind::DriveRoot && e.size.is_none() && !e.partial));
        let iso = got.iter().find(|e| e.path.ends_with("big.iso")).unwrap();
        assert!(!iso.is_dir);
    }

    #[test]
    fn system_and_own_folders_are_never_listed() {
        let mut fs = base().dir(r"D:\Stale Uninstaller\quarantine", 5).dir(r"D:\Kept", 5).dir(r"D:\Mine", 5);
        for name in [
            "Windows",
            "Program Files",
            "ProgramData",
            "Users",
            "$Recycle.Bin",
            "System Volume Information",
            "Recovery",
            "Boot",
            "EFI",
            "Documents and Settings",
        ] {
            fs = fs.dir(&format!(r"D:\{name}"), 5);
        }
        for name in ["pagefile.sys", "hiberfil.sys", "swapfile.sys", "DumpStack.log.tmp"] {
            fs = fs.file(&format!(r"D:\{name}"), 900 * MB);
        }
        let own = OwnPaths::from_folders([PathBuf::from(r"D:\Stale Uninstaller")]);
        let keep = KeepList { paths: vec![PathBuf::from(r"D:\Kept")], ..KeepList::default() };
        let got = list_with(&fs, &InstalledIndex::default(), &keep, &own);
        assert_eq!(paths(&got), vec![r"D:\Mine"]);
    }

    #[test]
    fn installed_program_folder_on_a_drive_root_is_not_listed() {
        let installed = InstalledIndex::from_programs(&[program("Acme Suite", r"D:\Acme"), program("Deep One", r"D:\Apps\Deep One")]);
        let fs = base().dir(r"D:\Acme\bin", 1).dir(r"D:\Apps\Deep One\bin", 1).dir(r"D:\Other", 1);
        let got = list_with(&fs, &installed, &KeepList::default(), &OwnPaths::default());
        assert_eq!(paths(&got), vec![r"D:\Other"]);
    }

    #[test]
    fn windows_old_is_listed_as_windows_cleanup() {
        let fs = base()
            .dir(r"C:\Windows.old\Users", 10)
            .dir(r"C:\$Windows.~BT\Sources", 5)
            .dir(r"C:\Windows\SoftwareDistribution\Download", 3)
            .dir(r"C:\Windows\Temp", 2);
        let got = list(&fs);
        assert_eq!(
            paths(&got),
            vec![r"C:\$Windows.~BT", r"C:\Windows.old", r"C:\Windows\SoftwareDistribution\Download", r"C:\Windows\Temp"]
        );
        let old = got.iter().find(|e| e.path.ends_with("Windows.old")).unwrap();
        assert_eq!(old.kind, SpaceKind::Windows(WinItem::WindowsOld));
        assert!(old.is_dir);
        assert!(got.iter().all(|e| removal(&e.kind) == Removal::WindowsCleanup));
    }

    #[test]
    fn user_temp_and_dumps_are_quarantinable() {
        let fs = base()
            .file(r"C:\Windows\MEMORY.DMP", 2000)
            .dir(r"C:\Windows\Minidump", 4)
            .dir(r"C:\Windows\LiveKernelReports", 4)
            .dir(r"C:\Users\A\AppData\Local\Temp", 9)
            .dir(r"C:\Users\B\AppData\Local\Temp", 9)
            .profile(r"C:\Users\A")
            .profile(r"C:\Users\B");
        let got = list(&fs);
        assert_eq!(got.len(), 5);
        for e in &got {
            assert_eq!(removal(&e.kind), Removal::Quarantine, "{:?}", e.path);
        }
        let dump = got.iter().find(|e| e.path.ends_with("MEMORY.DMP")).unwrap();
        assert!(!dump.is_dir);
        assert_eq!(got.iter().filter(|e| e.kind == SpaceKind::Windows(WinItem::UserTemp)).count(), 2);
        assert_eq!(removal(&SpaceKind::Windows(WinItem::WindowsTemp)), Removal::WindowsCleanup);
        assert_eq!(removal(&SpaceKind::DriveRoot), Removal::Quarantine);
        assert_eq!(removal(&SpaceKind::Orphan(RootKind::AppData)), Removal::Quarantine);
        assert_eq!(measure(&fs, dump), (Some(2000), false));
    }

    #[test]
    fn user_temp_counts_only_files_older_than_a_week() {
        let now = DateTime::parse_from_rfc3339("2026-10-23T12:00:00Z").unwrap().with_timezone(&Utc);
        let at = |days: i64| now - Duration::days(days);
        let fs = base()
            .file(r"C:\Users\A\AppData\Local\Temp\old.tmp", 100)
            .file(r"C:\Users\A\AppData\Local\Temp\sub\older.tmp", 50)
            .file(r"C:\Users\A\AppData\Local\Temp\new.tmp", 7)
            .file(r"C:\Users\A\AppData\Local\Temp\unknown.tmp", 3)
            .profile(r"C:\Users\A")
            .modified(r"C:\Users\A\AppData\Local\Temp\old.tmp", at(30))
            .modified(r"C:\Users\A\AppData\Local\Temp\sub\older.tmp", at(8))
            .modified(r"C:\Users\A\AppData\Local\Temp\new.tmp", at(1));
        let temp = list(&fs).into_iter().find(|e| e.kind == SpaceKind::Windows(WinItem::UserTemp)).unwrap();
        assert_eq!(measure_at(&fs, &temp, now), (Some(150), false));
    }

    #[test]
    fn orphan_seen_twice_is_listed_once() {
        let fs = base().dir(r"D:\Gone Thing", 5).dir(r"C:\Program Files\Gone Tool", 5);
        let mut roots = FsScanRoots::from_env(&fs);
        // A drive root that is both a program root and a drive.
        roots.program_files.push(PathBuf::from(r"D:\"));
        let got = list_entries(&fs, &drives(), &roots, &InstalledIndex::default(), &KeepList::default(), &OwnPaths::default());
        assert_eq!(paths(&got), vec![r"C:\Program Files\Gone Tool", r"D:\Gone Thing"]);
        let gone = got.iter().find(|e| e.path.ends_with("Gone Thing")).unwrap();
        assert_eq!(gone.kind, SpaceKind::Orphan(RootKind::ProgramFiles));
    }

    #[test]
    fn unreadable_subfolder_marks_size_partial() {
        let fs = base().dir(r"C:\Windows.old\Users", 10).dir(r"C:\Windows.old\Locked", 99).unreadable(r"C:\Windows.old\Locked");
        let old = list(&fs).into_iter().find(|e| e.kind == SpaceKind::Windows(WinItem::WindowsOld)).unwrap();
        assert_eq!(measure(&fs, &old), (Some(10), true));
        let fs = base().dir(r"C:\Windows.old\Users", 10);
        assert_eq!(measure(&fs, &old), (Some(10), false));
        assert_eq!(measure(&fs, &SpaceEntry { path: PathBuf::from(r"C:\Nope"), ..old }), (None, false));
    }

    #[test]
    fn real_dir_size_partial_reports_complete_size() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a").join("f"), b"12345").unwrap();
        assert_eq!(crate::size::dir_size_partial(dir.path()), Some((5, false)));
        assert_eq!(crate::size::dir_size_partial(&dir.path().join("missing")), None);
    }

    #[test]
    #[ignore]
    fn live_fixed_drives_include_the_system_drive() {
        assert!(!fixed_drives().is_empty());
    }
}
