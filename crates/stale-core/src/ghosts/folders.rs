//! Orphan folders: folders in program roots that belong to no installed program.

use super::{is_container_folder, is_skipped_folder, Evidence, InstalledIndex, RootKind};
use crate::fsys::FsReader;
use crate::keep::KeepList;
use crate::leftovers::fs_scan::{holds_user_data, FsScanRoots};
use crate::leftovers::names::is_matchable;
use crate::pathutil::{is_shared_root, norm};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The kind of a per-profile root, for the only per-profile roots scanned here: `AppData\Roaming`,
/// `AppData\Local` and `AppData\Local\Programs`. Other entries of [`FsScanRoots::per_profile`]
/// (`AppData\Local\Temp`) are not program roots.
pub(super) fn per_profile_kind(rel: &Path) -> Option<RootKind> {
    match norm(rel).as_str() {
        r"APPDATA\ROAMING" | r"APPDATA\LOCAL" => Some(RootKind::AppData),
        r"APPDATA\LOCAL\PROGRAMS" => Some(RootKind::LocalPrograms),
        _ => None,
    }
}

fn scan_roots(fs: &dyn FsReader, roots: &FsScanRoots) -> Vec<(PathBuf, RootKind)> {
    let mut out: Vec<(PathBuf, RootKind)> = roots.program_files.iter().map(|p| (p.clone(), RootKind::ProgramFiles)).collect();
    if !roots.program_data.as_os_str().is_empty() {
        out.push((roots.program_data.clone(), RootKind::ProgramData));
    }
    for profile in fs.profile_dirs() {
        for rel in &roots.per_profile {
            if let Some(kind) = per_profile_kind(rel) {
                out.push((profile.join(rel), kind));
            }
        }
    }
    out
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Two or more subfolders and no files at the top level: the shape of a publisher folder.
fn has_container_shape(fs: &dyn FsReader, dir: &Path) -> bool {
    let entries = fs.list_dir(dir);
    !entries.iter().any(|e| !e.is_dir) && entries.iter().filter(|e| e.is_dir).count() >= 2
}

enum Verdict {
    Orphan,
    Container,
    Ignore,
}

/// Classifies the folder `dir`; `container` is the name of the publisher folder it sits in, if
/// any. Only top-level folders can be containers, so the descent is one level deep.
fn classify(fs: &dyn FsReader, dir: &Path, container: Option<&str>, installed: &InstalledIndex, keep: &KeepList) -> Verdict {
    let name = file_name(dir);
    if is_shared_root(dir) || keep.is_path_kept(dir) || installed.covers_path(dir) || name.starts_with('.') {
        return Verdict::Ignore;
    }
    if container.is_none()
        && (is_container_folder(&name) || (!is_skipped_folder(&name) && !is_matchable(&name) && has_container_shape(fs, dir)))
    {
        return Verdict::Container;
    }
    let owned = installed.matches_name(&name)
        || installed.holds_location(dir)
        || container.is_some_and(|c| installed.matches_in_container(&name, c));
    if is_skipped_folder(&name) || !is_matchable(&name) || owned {
        Verdict::Ignore
    } else {
        Verdict::Orphan
    }
}

/// Folders in the program roots (Program Files, ProgramData and each profile's
/// `AppData\Roaming`, `AppData\Local`, `AppData\Local\Programs`) that no installed program owns.
/// Publisher containers are never listed themselves; their children are examined instead.
pub fn orphan_folders(fs: &dyn FsReader, roots: &FsScanRoots, installed: &InstalledIndex, keep: &KeepList) -> Vec<Evidence> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<Evidence> = Vec::new();
    let mut push = |dir: &Path, root_kind: RootKind| {
        if seen.insert(norm(dir)) {
            out.push(Evidence::Folder {
                path: dir.to_path_buf(),
                size: fs.dir_size(dir),
                modified: fs.modified(dir),
                root_kind,
                holds_user_data: holds_user_data(fs, dir),
            });
        }
    };
    for (root, kind) in scan_roots(fs, roots) {
        for entry in fs.list_dir(&root).into_iter().filter(|e| e.is_dir) {
            match classify(fs, &entry.path, None, installed, keep) {
                Verdict::Orphan => push(&entry.path, kind),
                Verdict::Ignore => {}
                Verdict::Container => {
                    let container = file_name(&entry.path);
                    for child in fs.list_dir(&entry.path).into_iter().filter(|e| e.is_dir) {
                        if let Verdict::Orphan = classify(fs, &child.path, Some(&container), installed, keep) {
                            push(&child.path, kind);
                        }
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsys::FakeFs;
    use crate::ghosts::RootKind;
    use crate::model::{Hive, Program, ProgramId, RegKeyPath, Scope};
    use chrono::{DateTime, Utc};
    use std::path::PathBuf;

    fn roots() -> FsScanRoots {
        FsScanRoots::from_env(
            &FakeFs::new()
                .dir(r"C:\Program Files", 0)
                .dir(r"C:\Program Files (x86)", 0)
                .env("ProgramFiles", r"C:\Program Files")
                .env("ProgramFiles(x86)", r"C:\Program Files (x86)")
                .env("ProgramData", r"C:\ProgramData"),
        )
    }

    fn program(name: &str, install: Option<&str>, hidden: bool) -> Program {
        Program {
            account: None,
            id: ProgramId(name.into()),
            key: RegKeyPath { hive: Hive::LocalMachine, path: format!(r"Software\{name}") },
            name: name.into(),
            publisher: None,
            version: None,
            install_location: install.map(PathBuf::from),
            install_date: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            display_icon: None,
            is_msi: false,
            estimated_size_bytes: None,
            drive: None,
            hidden,
            scope: Scope::Machine,
        }
    }

    fn scan(fs: &FakeFs, programs: &[Program], keep: &KeepList) -> Vec<Evidence> {
        orphan_folders(fs, &roots(), &InstalledIndex::from_programs(programs), keep)
    }

    fn paths(ev: &[Evidence]) -> Vec<String> {
        let mut v: Vec<String> = ev
            .iter()
            .map(|e| match e {
                Evidence::Folder { path, .. } => path.display().to_string(),
                e => panic!("unexpected {e:?}"),
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn orphan_folder_in_program_files_is_evidence() {
        let at = DateTime::parse_from_rfc3339("2023-01-02T03:04:05Z").unwrap().with_timezone(&Utc);
        let fs = FakeFs::new()
            .file(r"C:\Program Files\Old Tool\old.exe", 40)
            .file(r"C:\Program Files\Old Tool\lib.dll", 2)
            .modified(r"C:\Program Files\Old Tool", at)
            .dir(r"C:\ProgramData\Gone Corp App", 7);
        let ev = scan(&fs, &[], &KeepList::default());
        assert_eq!(paths(&ev), vec![r"C:\Program Files\Old Tool", r"C:\ProgramData\Gone Corp App"]);
        let old = ev.iter().find(|e| matches!(e, Evidence::Folder { path, .. } if path.ends_with("Old Tool"))).unwrap();
        assert_eq!(
            old,
            &Evidence::Folder {
                path: PathBuf::from(r"C:\Program Files\Old Tool"),
                size: Some(42),
                modified: Some(at),
                root_kind: RootKind::ProgramFiles,
                holds_user_data: false,
            }
        );
        let data = ev.iter().find(|e| matches!(e, Evidence::Folder { path, .. } if path.ends_with("Gone Corp App"))).unwrap();
        assert!(matches!(data, Evidence::Folder { root_kind: RootKind::ProgramData, size: Some(7), modified: None, .. }));
    }

    #[test]
    fn user_data_child_is_flagged() {
        let fs = FakeFs::new()
            .dir(r"C:\Program Files\Old Game\Saves\slot1", 5)
            .dir(r"C:\Program Files\Old Tool\bin", 1)
            .dir(r"C:\Program Files\Deep Game\data\saves", 1);
        let ev = scan(&fs, &[], &KeepList::default());
        let mut flagged: Vec<(String, bool)> = ev
            .iter()
            .map(|e| match e {
                Evidence::Folder { path, holds_user_data, .. } => (path.display().to_string(), *holds_user_data),
                e => panic!("unexpected {e:?}"),
            })
            .collect();
        flagged.sort();
        assert_eq!(
            flagged,
            vec![
                (r"C:\Program Files\Deep Game".to_string(), false),
                (r"C:\Program Files\Old Game".to_string(), true),
                (r"C:\Program Files\Old Tool".to_string(), false),
            ]
        );
    }

    #[test]
    fn installed_program_folder_is_not_orphan() {
        let mut icon = program("Icon Only", None, false);
        icon.display_icon = Some(PathBuf::from(r"C:\Program Files (x86)\Shiny\app.ico"));
        let mut uninst = program("Uninst Only", None, false);
        uninst.uninstall_string = Some(r#""C:\Program Files\Removo\bin\unins000.exe" /S"#.into());
        let programs = [
            program("Totally Different", Some(r"C:\Program Files\Acme Suite\"), false),
            program("Hidden Component", Some(r"C:\ProgramData\Helper Bits"), true),
            icon,
            uninst,
        ];
        let fs = FakeFs::new()
            .dir(r"C:\Program Files\Acme Suite", 1)
            .dir(r"C:\ProgramData\Helper Bits", 1)
            .dir(r"C:\Program Files (x86)\Shiny", 1)
            // Holds an installed program's folder (its uninstaller folder) below it.
            .dir(r"C:\Program Files\Removo\bin", 1)
            .dir(r"C:\Program Files\Orphaned", 1);
        assert_eq!(paths(&scan(&fs, &programs, &KeepList::default())), vec![r"C:\Program Files\Orphaned"]);
    }

    #[test]
    fn name_match_to_installed_is_not_orphan() {
        let programs = [program("Unity Hub 3.4 (x64)", Some(r"D:\Apps\Unity Hub"), false), program("Notepad++", None, false)];
        let fs = FakeFs::new()
            .dir(r"C:\Program Files\UnityHub", 1)
            .dir(r"C:\ProgramData\notepad++", 1)
            .dir(r"C:\Program Files\Something Else", 1);
        assert_eq!(paths(&scan(&fs, &programs, &KeepList::default())), vec![r"C:\Program Files\Something Else"]);
    }

    #[test]
    fn publisher_container_descends_one_level() {
        let programs = [
            program("Google Chrome", Some(r"C:\Program Files\Google\Chrome\Application"), false),
            program("Mozilla Firefox", Some(r"C:\Program Files\Mozilla Firefox"), false),
        ];
        let fs = FakeFs::new()
            // Known container: the installed child stays, the dead one is listed.
            .dir(r"C:\Program Files\Google\Chrome\Application", 1)
            .dir(r"C:\Program Files\Google\Picasa", 1)
            // Unmatchable name with 2+ subfolders and no files: a container, descended one level only.
            .dir(r"C:\Program Files\Tools\Gone Viewer", 1)
            .dir(r"C:\Program Files\Tools\Common\Deep Thing", 1)
            // A matchable product folder with subfolders and no files is itself the candidate.
            .dir(r"C:\Program Files\Big Product\bin", 1)
            .dir(r"C:\Program Files\Big Product\data", 1)
            // Container child named after an installed program minus the container's words.
            .dir(r"C:\Users\A\AppData\Roaming\Mozilla\Firefox", 1)
            .dir(r"C:\Users\A\AppData\Roaming\Mozilla\Thunderbird", 1)
            .profile(r"C:\Users\A");
        assert_eq!(
            paths(&scan(&fs, &programs, &KeepList::default())),
            vec![
                r"C:\Program Files\Big Product",
                r"C:\Program Files\Google\Picasa",
                r"C:\Program Files\Tools\Gone Viewer",
                r"C:\Users\A\AppData\Roaming\Mozilla\Thunderbird",
            ]
        );
    }

    #[test]
    fn skip_list_and_shared_roots_ignored() {
        let mut fs = FakeFs::new().profile(r"C:\Users\A");
        for name in [
            "Microsoft",
            "Common Files",
            "Windows Defender Advanced Threat Protection",
            "WindowsApps",
            "windows nt",
            "Package Cache",
            "NVIDIA Corporation",
            "dotnet",
            "Reference Assemblies",
            "Uninstall Information",
        ] {
            fs = fs.dir(&format!(r"C:\Program Files\{name}\x"), 1).dir(&format!(r"C:\Program Files\{name}\y"), 1);
        }
        for name in ["Packages", "Temp", "D3DSCache", "SquirrelTemp", "Programs", "CrashPad", ".cache", ".vscode", "npm-cache"] {
            fs = fs.dir(&format!(r"C:\Users\A\AppData\Local\{name}"), 1);
        }
        let fs = fs
            // Not scanned at all: Temp and the user folders.
            .dir(r"C:\Users\A\AppData\Local\Temp\Gone Thing", 1)
            .dir(r"C:\Users\A\Documents\Gone Thing", 1)
            // A drive's `Program Files` is a shared root on any drive.
            .dir(r"D:\Program Files\Deep App", 1);
        let mut roots = roots();
        roots.program_files.push(PathBuf::from(r"D:\"));
        let ev = orphan_folders(&fs, &roots, &InstalledIndex::default(), &KeepList::default());
        assert_eq!(paths(&ev), Vec::<String>::new());
    }

    #[test]
    fn kept_path_is_ignored() {
        let fs = FakeFs::new()
            .dir(r"C:\Program Files\Keep Me", 1)
            .dir(r"C:\Program Files\Vendor Box\Inner App", 1)
            .dir(r"C:\Program Files\Drop Me", 1);
        let keep = KeepList { paths: vec![PathBuf::from(r"c:\program files\keep me\")], ..KeepList::default() };
        assert_eq!(paths(&scan(&fs, &[], &keep)), vec![r"C:\Program Files\Drop Me", r"C:\Program Files\Vendor Box"]);
        let keep = KeepList { paths: vec![PathBuf::from(r"C:\Program Files\Vendor Box")], ..KeepList::default() };
        assert_eq!(paths(&scan(&fs, &[], &keep)), vec![r"C:\Program Files\Drop Me", r"C:\Program Files\Keep Me"]);
    }

    #[test]
    fn appdata_roots_scanned_per_profile() {
        let fs = FakeFs::new()
            .dir(r"C:\Users\A\AppData\Roaming\Gone Roamer", 1)
            .dir(r"C:\Users\B\AppData\Local\Gone Local", 1)
            .dir(r"C:\Users\B\AppData\Local\Programs\Gone Program", 1)
            .profile(r"C:\Users\A")
            .profile(r"C:\Users\B");
        let ev = scan(&fs, &[], &KeepList::default());
        assert_eq!(
            paths(&ev),
            vec![
                r"C:\Users\A\AppData\Roaming\Gone Roamer",
                r"C:\Users\B\AppData\Local\Gone Local",
                r"C:\Users\B\AppData\Local\Programs\Gone Program",
            ]
        );
        let kind = |suffix: &str| {
            ev.iter()
                .find_map(|e| match e {
                    Evidence::Folder { path, root_kind, .. } if path.ends_with(suffix) => Some(*root_kind),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(kind("Gone Roamer"), RootKind::AppData);
        assert_eq!(kind("Gone Local"), RootKind::AppData);
        assert_eq!(kind("Gone Program"), RootKind::LocalPrograms);
    }
}
