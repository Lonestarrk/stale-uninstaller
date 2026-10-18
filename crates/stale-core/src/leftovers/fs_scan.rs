//! Folders a program left behind.

use super::names::{name_matches, publisher_matches};
use super::{Confidence, Leftover, LeftoverKind, ProgramHint};
use crate::fsys::FsReader;
use crate::pathutil::{is_shared_root, norm};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct FsScanRoots {
    pub program_files: Vec<PathBuf>,
    pub program_data: PathBuf,
    /// Relative to each profile dir.
    pub per_profile: Vec<PathBuf>,
    /// Relative to each profile dir.
    pub user_folders: Vec<PathBuf>,
}

impl FsScanRoots {
    pub fn from_env(fs: &dyn FsReader) -> FsScanRoots {
        let mut program_files: Vec<PathBuf> = Vec::new();
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(p) = fs.env(var).map(PathBuf::from).filter(|p| fs.exists(p)) {
                if !program_files.iter().any(|q| norm(q) == norm(&p)) {
                    program_files.push(p);
                }
            }
        }
        let rel = |v: &[&str]| v.iter().map(PathBuf::from).collect();
        FsScanRoots {
            program_files,
            program_data: fs.env("ProgramData").map(PathBuf::from).unwrap_or_default(),
            per_profile: rel(&[r"AppData\Roaming", r"AppData\Local", r"AppData\Local\Programs", r"AppData\Local\Temp"]),
            user_folders: rel(&["Documents", "Pictures", "Desktop", "Downloads"]),
        }
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn scan_folders(fs: &dyn FsReader, roots: &FsScanRoots, hint: &ProgramHint) -> Vec<Leftover> {
    let install = hint.install_location.as_deref().filter(|p| !p.as_os_str().is_empty());
    let install_norm = install.map(norm);
    let is_install = |p: &Path| install_norm.as_deref() == Some(norm(p).as_str());
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<Leftover> = Vec::new();

    let mut push = |path: &Path, confidence: Confidence, note: Option<&str>| {
        if is_shared_root(path) || !seen.insert(norm(path)) {
            return;
        }
        out.push(Leftover {
            kind: LeftoverKind::Dir { path: path.to_path_buf(), size: fs.dir_size(path) },
            confidence,
            note: note.map(str::to_string),
        });
    };

    if let Some(loc) = install {
        if fs.is_dir(loc) {
            push(loc, Confidence::High, None);
        }
    }

    let profiles = fs.profile_dirs();
    let mut name_roots: Vec<PathBuf> = roots.program_files.clone();
    if !roots.program_data.as_os_str().is_empty() {
        name_roots.push(roots.program_data.clone());
    }
    for profile in &profiles {
        name_roots.extend(roots.per_profile.iter().map(|r| profile.join(r)));
    }
    for root in &name_roots {
        for entry in fs.list_dir(root).into_iter().filter(|e| e.is_dir) {
            if name_matches(&file_name(&entry.path), &hint.name) {
                if !is_install(&entry.path) {
                    push(&entry.path, Confidence::Medium, None);
                }
            } else if hint.publisher.as_deref().is_some_and(|p| publisher_matches(&file_name(&entry.path), p)) {
                for child in fs.list_dir(&entry.path).into_iter().filter(|e| e.is_dir) {
                    if name_matches(&file_name(&child.path), &hint.name) && !is_install(&child.path) {
                        push(&child.path, Confidence::Medium, None);
                    }
                }
            }
        }
    }

    for profile in &profiles {
        for rel in &roots.user_folders {
            for entry in fs.list_dir(&profile.join(rel)).into_iter().filter(|e| e.is_dir) {
                if name_matches(&file_name(&entry.path), &hint.name) && !is_install(&entry.path) {
                    push(&entry.path, Confidence::Low, Some("user-folder"));
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

    fn roots() -> FsScanRoots {
        FsScanRoots::from_env(
            &FakeFs::new()
                .dir(r"C:\Program Files", 0)
                .env("ProgramFiles", r"C:\Program Files")
                .env("ProgramData", r"C:\ProgramData"),
        )
    }

    fn hint(name: &str, publisher: Option<&str>, loc: Option<&str>) -> ProgramHint {
        let mut h = ProgramHint::manual(name, loc.map(PathBuf::from));
        h.publisher = publisher.map(str::to_string);
        h
    }

    fn dirs(items: &[Leftover]) -> Vec<(String, Confidence)> {
        let mut v: Vec<_> = items
            .iter()
            .map(|l| match &l.kind {
                LeftoverKind::Dir { path, .. } => (path.to_string_lossy().into_owned(), l.confidence),
                k => panic!("unexpected {k:?}"),
            })
            .collect();
        v.sort_by_key(|(p, _)| p.clone());
        v
    }

    #[test]
    fn from_env_reads_roots() {
        let r = roots();
        assert_eq!(r.program_files, vec![PathBuf::from(r"C:\Program Files")]);
        assert_eq!(r.program_data, PathBuf::from(r"C:\ProgramData"));
        assert_eq!(r.per_profile.len(), 4);
        assert_eq!(r.user_folders.len(), 4);
    }

    #[test]
    fn install_location_is_high() {
        let fs = FakeFs::new().dir(r"D:\Apps\Unity Hub", 500).profile(r"C:\Users\A");
        let out = scan_folders(&fs, &roots(), &hint("Unity Hub", None, Some(r"D:\Apps\Unity Hub")));
        assert_eq!(dirs(&out), vec![(r"D:\Apps\Unity Hub".to_string(), Confidence::High)]);
        assert_eq!(out[0].size(), Some(500));
    }

    #[test]
    fn name_match_in_program_files_and_appdata_is_medium() {
        let fs = FakeFs::new()
            .dir(r"C:\Program Files\Unity Hub", 10)
            .dir(r"C:\Users\A\AppData\Roaming\UnityHub", 5)
            .dir(r"C:\Program Files\Other", 1)
            .profile(r"C:\Users\A");
        let out = scan_folders(&fs, &roots(), &hint("Unity Hub", None, None));
        assert_eq!(
            dirs(&out),
            vec![
                (r"C:\Program Files\Unity Hub".to_string(), Confidence::Medium),
                (r"C:\Users\A\AppData\Roaming\UnityHub".to_string(), Confidence::Medium),
            ]
        );
    }

    #[test]
    fn publisher_folder_then_name_is_medium_but_publisher_itself_not_listed() {
        let fs = FakeFs::new()
            .dir(r"C:\Program Files\Unity Technologies\Unity Hub", 10)
            .dir(r"C:\Program Files\Unity Technologies\Editor", 3);
        let out = scan_folders(&fs, &roots(), &hint("Unity Hub", Some("Unity Technologies"), None));
        assert_eq!(dirs(&out), vec![(r"C:\Program Files\Unity Technologies\Unity Hub".to_string(), Confidence::Medium)]);
    }

    #[test]
    fn user_folders_are_low_with_note() {
        let fs = FakeFs::new().dir(r"C:\Users\A\Documents\Unity Hub", 2).profile(r"C:\Users\A");
        let out = scan_folders(&fs, &roots(), &hint("Unity Hub", None, None));
        assert_eq!(dirs(&out), vec![(r"C:\Users\A\Documents\Unity Hub".to_string(), Confidence::Low)]);
        assert_eq!(out[0].note.as_deref(), Some("user-folder"));
    }

    #[test]
    fn shared_root_install_location_is_not_listed_and_siblings_untouched() {
        let fs = FakeFs::new().dir(r"C:\Program Files\Other", 1);
        let out = scan_folders(&fs, &roots(), &hint("Unity Hub", None, Some(r"C:\Program Files")));
        assert!(out.is_empty());
    }

    #[test]
    fn install_location_is_not_listed_twice() {
        let fs = FakeFs::new().dir(r"C:\Program Files\Unity Hub", 10);
        let out = scan_folders(&fs, &roots(), &hint("Unity Hub", None, Some(r"c:\program files\unity hub")));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].confidence, Confidence::High);
    }

    #[test]
    fn generic_name_only_matches_by_path() {
        let fs = FakeFs::new().dir(r"C:\Program Files\Tools", 1);
        let out = scan_folders(&fs, &roots(), &hint("Tools", None, None));
        assert!(out.is_empty());
    }

    #[test]
    fn every_profile_is_scanned() {
        let fs = FakeFs::new()
            .dir(r"C:\Users\A\AppData\Local\Foo Studio", 1)
            .dir(r"C:\Users\B\AppData\Local\Foo Studio", 2)
            .profile(r"C:\Users\A")
            .profile(r"C:\Users\B");
        let out = scan_folders(&fs, &roots(), &hint("Foo Studio", None, None));
        assert_eq!(out.len(), 2);
    }
}
