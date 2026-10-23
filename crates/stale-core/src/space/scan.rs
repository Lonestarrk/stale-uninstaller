//! Listing and measuring space entries.

use super::catalog::catalog_entries;
use super::{removal, Removal, SpaceEntry, SpaceKind, WinItem};
use crate::fsys::{DirEntryInfo, FsReader};
use crate::ghosts::folders::list_orphan_folders;
use crate::ghosts::group::{REASON_FIRST_WORD, REASON_OTHER_PROFILE, REASON_SIBLING_OWNED, REASON_WORD_OF_NAME};
use crate::ghosts::{InstalledIndex, RootKind};
use crate::keep::KeepList;
use crate::leftovers::{Confidence, Leftover, LeftoverKind};
use crate::leftovers::fs_scan::FsScanRoots;
use crate::own::OwnPaths;
use crate::pathutil::{is_shared_root, norm, normed_under};
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
    "config.msi",
    "$winreagent",
    "$sysreset",
    "$getcurrent",
    "msocache",
    "windows.old",
    "$windows.~bt",
    "$windows.~ws",
    "$windows.~tmp",
];

pub(super) fn never_listed(name: &str) -> bool {
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
    let windows = norm(&fs.windows_dir());
    let profiles: Vec<String> = fs.profile_dirs().iter().map(|p| norm(p)).collect();
    let current_profile = fs.env("USERPROFILE").map(|p| norm(Path::new(&p))).filter(|p| !p.is_empty());
    let mut push = |path: PathBuf, is_dir: bool, kind: SpaceKind, note: Option<&'static str>| {
        if seen.insert(norm(&path)) {
            let modified = fs.modified(&path);
            out.push(SpaceEntry { path, is_dir, kind, size: None, partial: false, modified, note });
        }
    };
    for o in list_orphan_folders(fs, roots, installed, keep) {
        if own.is_own_path(&o.path) {
            continue;
        }
        let name = o.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let other_profile = matches!(o.root_kind, RootKind::AppData | RootKind::LocalPrograms)
            && current_profile.as_deref().is_some_and(|c| !normed_under(&norm(&o.path), c));
        let note = if other_profile {
            Some(REASON_OTHER_PROFILE)
        } else if o.container.as_deref().is_some_and(|c| installed.owns_sibling_in(c)) {
            Some(REASON_SIBLING_OWNED)
        } else if installed.is_word_of_name(&name) {
            Some(REASON_WORD_OF_NAME)
        } else if installed.matches_first_word(&name) {
            Some(REASON_FIRST_WORD)
        } else {
            None
        };
        push(o.path, true, SpaceKind::Orphan(o.root_kind), note);
    }
    for drive in drives {
        for e in fs.list_dir(drive) {
            let name = e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if never_listed(&name)
                || is_shared_root(&e.path)
                || norm(&e.path) == windows
                || profiles.contains(&norm(&e.path))
                || own.is_own_path(&e.path)
                || keep.is_path_kept(&e.path)
                || installed.covers_path(&e.path)
                || (e.is_dir && installed.holds_location(&e.path))
                || (!e.is_dir && e.size < MIN_ROOT_FILE_BYTES)
            {
                continue;
            }
            push(e.path, e.is_dir, SpaceKind::DriveRoot, None);
        }
    }
    for e in catalog_entries(fs, drives) {
        if !own.is_own_path(&e.path) && !keep.is_path_kept(&e.path) {
            push(e.path, e.is_dir, e.kind, None);
        }
    }
    out
}

/// A temp folder's direct children (files and folders) last modified before the cutoff, links
/// and children of unknown age left out: what the cleanup of a temp folder covers.
fn old_children(fs: &dyn FsReader, dir: &Path, now: DateTime<Utc>) -> Vec<DirEntryInfo> {
    let cutoff = now - Duration::days(USER_TEMP_MIN_AGE_DAYS);
    fs.list_dir(dir).into_iter().filter(|e| !fs.is_reparse(&e.path) && fs.modified(&e.path).is_some_and(|m| m < cutoff)).collect()
}

fn is_temp(kind: &SpaceKind) -> bool {
    matches!(kind, SpaceKind::Windows(WinItem::UserTemp | WinItem::WindowsTemp))
}

/// The size of `entry` and whether it is only a lower bound. A temp folder counts only what
/// [`removal_items`] covers: children older than [`USER_TEMP_MIN_AGE_DAYS`] days.
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
    if is_temp(&entry.kind) {
        if !fs.is_dir(&entry.path) {
            return (None, false);
        }
        let (mut total, mut partial) = (0, false);
        for c in old_children(fs, &entry.path, now) {
            if c.is_dir {
                let (size, p) = fs.dir_size_partial(&c.path).unwrap_or((0, true));
                total += size;
                partial |= p;
            } else {
                total += c.size;
            }
        }
        return (Some(total), partial);
    }
    match fs.dir_size_partial(&entry.path) {
        Some((size, partial)) => (Some(size), partial),
        None => (None, false),
    }
}

/// What removing `entry` through the quarantine moves, as leftovers (Medium). A temp folder
/// yields its old direct children, never the folder itself; any other quarantinable entry yields
/// itself; a [`Removal::WindowsCleanup`] entry yields nothing.
pub fn removal_items(fs: &dyn FsReader, entry: &SpaceEntry, now: DateTime<Utc>) -> Vec<Leftover> {
    let item = |path: PathBuf, is_dir: bool, size: Option<u64>| Leftover {
        kind: if is_dir { LeftoverKind::Dir { path, size } } else { LeftoverKind::File { path, size } },
        confidence: Confidence::Medium,
        note: None,
    };
    if is_temp(&entry.kind) {
        return old_children(fs, &entry.path, now).into_iter().map(|c| item(c.path, c.is_dir, (!c.is_dir).then_some(c.size))).collect();
    }
    if removal(&entry.kind) == Removal::WindowsCleanup {
        return Vec::new();
    }
    vec![item(entry.path.clone(), entry.is_dir, entry.size)]
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

    fn temp_fs(now: DateTime<Utc>) -> FakeFs {
        let at = |days: i64| now - Duration::days(days);
        base()
            .file(r"C:\Users\A\AppData\Local\Temp\old.tmp", 100)
            .file(r"C:\Users\A\AppData\Local\Temp\sub\older.tmp", 50)
            .file(r"C:\Users\A\AppData\Local\Temp\new.tmp", 7)
            .file(r"C:\Users\A\AppData\Local\Temp\unknown.tmp", 3)
            .file(r"C:\Users\A\AppData\Local\Temp\link.tmp", 1000)
            .profile(r"C:\Users\A")
            .modified(r"C:\Users\A\AppData\Local\Temp\old.tmp", at(30))
            .modified(r"C:\Users\A\AppData\Local\Temp\sub", at(8))
            .modified(r"C:\Users\A\AppData\Local\Temp\new.tmp", at(1))
            .modified(r"C:\Users\A\AppData\Local\Temp\link.tmp", at(30))
            .reparse(r"C:\Users\A\AppData\Local\Temp\link.tmp")
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-23T12:00:00Z").unwrap().with_timezone(&Utc)
    }

    #[test]
    fn user_temp_counts_only_children_older_than_a_week() {
        let fs = temp_fs(now());
        let temp = list(&fs).into_iter().find(|e| e.kind == SpaceKind::Windows(WinItem::UserTemp)).unwrap();
        assert_eq!(measure_at(&fs, &temp, now()), (Some(150), false));
    }

    #[test]
    fn temp_removal_items_are_the_old_children_never_the_folder() {
        let fs = temp_fs(now());
        let temp = list(&fs).into_iter().find(|e| e.kind == SpaceKind::Windows(WinItem::UserTemp)).unwrap();
        let mut got: Vec<(String, bool)> = removal_items(&fs, &temp, now())
            .into_iter()
            .map(|l| {
                assert_eq!(l.confidence, Confidence::Medium);
                match l.kind {
                    LeftoverKind::Dir { path, .. } => (path.display().to_string(), true),
                    LeftoverKind::File { path, .. } => (path.display().to_string(), false),
                    k => panic!("{k:?}"),
                }
            })
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![(r"C:\Users\A\AppData\Local\Temp\old.tmp".to_string(), false), (r"C:\Users\A\AppData\Local\Temp\sub".to_string(), true)]
        );
    }

    #[test]
    fn removal_items_of_other_entries() {
        let fs = base().dir(r"D:\Foton", 9).dir(r"C:\Windows.old\x", 1);
        let got = list(&fs);
        let foton = got.iter().find(|e| e.path.ends_with("Foton")).unwrap();
        let items = removal_items(&fs, foton, now());
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0].kind, LeftoverKind::Dir { path, .. } if path.ends_with("Foton")));
        let old = got.iter().find(|e| e.path.ends_with("Windows.old")).unwrap();
        assert!(removal_items(&fs, old, now()).is_empty());
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
    fn spoofed_system_root_changes_nothing() {
        let fs = base()
            .env("SystemRoot", r"D:\Data")
            .file(r"D:\Data\MEMORY.DMP", 5)
            .dir(r"D:\Data\Minidump", 5)
            .dir(r"C:\Windows\Minidump", 5);
        let got = list(&fs);
        let dumps: Vec<&SpaceEntry> = got.iter().filter(|e| matches!(e.kind, SpaceKind::Windows(_))).collect();
        assert_eq!(dumps.len(), 1);
        assert_eq!(dumps[0].path, PathBuf::from(r"C:\Windows\Minidump"));
        // The pinned Windows folder is the one that counts, wherever it is.
        let fs = FakeFs::new().windows_dir(r"E:\WINNT").dir(r"E:\WINNT\Minidump", 1).dir(r"E:\Windows.old", 1);
        let got = list_entries(&fs, &[PathBuf::from(r"E:\")], &FsScanRoots::from_env(&fs), &InstalledIndex::default(), &KeepList::default(), &OwnPaths::default());
        assert_eq!(paths(&got), vec![r"E:\WINNT\Minidump", r"E:\Windows.old"]);
    }

    #[test]
    fn linked_kept_and_own_catalog_entries_are_not_listed() {
        let fs = base()
            .dir(r"C:\Windows\Temp", 1)
            .dir(r"C:\Windows\Minidump", 1)
            .dir(r"C:\Windows\LiveKernelReports", 1)
            .dir(r"C:\Windows.old", 1)
            .reparse(r"C:\Windows\Minidump")
            .reparse(r"C:\Windows.old");
        let keep = KeepList { paths: vec![PathBuf::from(r"C:\Windows\LiveKernelReports")], ..KeepList::default() };
        let own = OwnPaths::from_folders([PathBuf::from(r"C:\Windows\Temp")]);
        assert!(list_with(&fs, &InstalledIndex::default(), &keep, &own).is_empty());
    }

    #[test]
    fn more_system_folders_and_profiles_are_never_listed_in_any_case() {
        let fs = base()
            .dir(r"D:\config.msi", 1)
            .dir(r"D:\$WINREAGENT", 1)
            .dir(r"D:\$SysReset", 1)
            .dir(r"D:\$GetCurrent", 1)
            .dir(r"D:\msocache", 1)
            .dir(r"D:\Jimmy", 1)
            .dir(r"D:\Other", 1)
            .profile(r"d:\JIMMY");
        assert_eq!(paths(&list(&fs)), vec![r"D:\Other"]);
    }

    #[test]
    fn upgrade_folders_on_any_drive_are_windows_items() {
        let fs = base().dir(r"F:\$WINDOWS.~TMP\x", 7).dir(r"D:\$Windows.~WS\Sources", 5).dir(r"D:\Windows.old", 5);
        let drives = [PathBuf::from(r"C:\"), PathBuf::from(r"D:\"), PathBuf::from(r"F:\")];
        let got = list_entries(&fs, &drives, &FsScanRoots::from_env(&fs), &InstalledIndex::default(), &KeepList::default(), &OwnPaths::default());
        let kind = |suffix: &str| got.iter().find(|e| e.path.ends_with(suffix)).map(|e| e.kind);
        assert_eq!(kind("$Windows.~TMP"), Some(SpaceKind::Windows(WinItem::WindowsTmp)));
        assert_eq!(kind("$Windows.~WS"), Some(SpaceKind::Windows(WinItem::WindowsWs)));
        // `Windows.old` belongs to the system drive only; elsewhere it is not listed at all.
        assert_eq!(kind("Windows.old"), None);
        for e in &got {
            assert_eq!(removal(&e.kind), Removal::WindowsCleanup);
        }
    }

    #[test]
    fn ambiguous_orphans_carry_a_note() {
        let installed = InstalledIndex::from_programs(&[
            program("Google Chrome", r"C:\Program Files\Google\Chrome\Application"),
            program("Microsoft Visual Studio Code", r"C:\Users\A\AppData\Local\Programs\Microsoft VS Code"),
        ]);
        let fs = base()
            .dir(r"C:\Program Files\Google\GoogleUpdater", 1)
            .dir(r"C:\Program Files\Google\Chrome\Application", 1)
            .dir(r"C:\Users\A\AppData\Roaming\Code", 1)
            .dir(r"C:\Users\B\AppData\Roaming\Gone Roamer", 1)
            .dir(r"C:\Users\A\AppData\Roaming\Gone Mine", 1)
            .dir(r"C:\Program Files\Gone Tool", 1)
            .env("USERPROFILE", r"C:\Users\A")
            .profile(r"C:\Users\A")
            .profile(r"C:\Users\B");
        let got = list_with(&fs, &installed, &KeepList::default(), &OwnPaths::default());
        let note = |suffix: &str| got.iter().find(|e| e.path.ends_with(suffix)).unwrap().note;
        assert_eq!(note("GoogleUpdater"), Some(REASON_SIBLING_OWNED));
        assert_eq!(note("Code"), Some(REASON_WORD_OF_NAME));
        assert_eq!(note("Gone Roamer"), Some(REASON_OTHER_PROFILE));
        assert_eq!(note("Gone Mine"), None);
        assert_eq!(note("Gone Tool"), None);
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
