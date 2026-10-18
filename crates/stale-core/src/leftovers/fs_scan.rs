//! Folders a program left behind.

use super::names::{
    child_matches_in_container, name_matches, name_matches_exact, name_matches_first_word_only, matched_helper_suffix,
    publisher_matches,
};
use super::{mark_used_by_others, user_folder_relation, UserFolderRelation, USER_FOLDERS, Confidence, Leftover, LeftoverKind, ProgramHint, NOTE_CONTAINS_USER_DATA, NOTE_SUFFIX_MATCH};
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
            user_folders: rel(&USER_FOLDERS),
        }
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Subfolder names (compared case-insensitively) that suggest an install folder keeps the user's
/// own files, such as game saves or projects.
const USER_DATA_CHILDREN: &[&str] =
    &["userdata", "saves", "saved", "savegames", "screenshots", "profiles", "workspace", "projects"];

/// True when `dir` has a direct child folder named like [`USER_DATA_CHILDREN`].
pub(crate) fn holds_user_data(fs: &dyn FsReader, dir: &Path) -> bool {
    fs.list_dir(dir)
        .iter()
        .filter(|e| e.is_dir)
        .any(|e| USER_DATA_CHILDREN.iter().any(|n| file_name(&e.path).eq_ignore_ascii_case(n)))
}

pub fn scan_folders(fs: &dyn FsReader, roots: &FsScanRoots, hint: &ProgramHint) -> Vec<Leftover> {
    let install_dir = hint.effective_install_dir();
    let install = install_dir.as_deref();
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

    let profiles = fs.profile_dirs();
    if let Some(loc) = install.filter(|l| fs.is_dir(l)) {
        match user_folder_relation(&roots.user_folders, &profiles, loc) {
            Some(UserFolderRelation::IsFolderOrAncestor) => {}
            Some(UserFolderRelation::Under) => push(loc, Confidence::Low, Some("user-folder")),
            None if holds_user_data(fs, loc) => push(loc, Confidence::Medium, Some(NOTE_CONTAINS_USER_DATA)),
            None => push(loc, Confidence::High, None),
        }
    }
    let mut name_roots: Vec<PathBuf> = roots.program_files.clone();
    if !roots.program_data.as_os_str().is_empty() {
        name_roots.push(roots.program_data.clone());
    }
    for profile in &profiles {
        name_roots.extend(roots.per_profile.iter().map(|r| profile.join(r)));
    }
    for root in &name_roots {
        for entry in fs.list_dir(root).into_iter().filter(|e| e.is_dir) {
            let name = file_name(&entry.path);
            if name_matches_exact(&name, &hint.name) {
                if !is_install(&entry.path) {
                    push(&entry.path, Confidence::Medium, None);
                }
            } else if let Some(suffix) = matched_helper_suffix(&name, &hint.name) {
                // `<name>-data` may hold the user's own databases: listed, never preselected.
                let confidence = if suffix == "data" { Confidence::Low } else { Confidence::Medium };
                if !is_install(&entry.path) {
                    push(&entry.path, confidence, Some(NOTE_SUFFIX_MATCH));
                }
            } else if name_matches_first_word_only(&name, &hint.name)
                || hint.publisher.as_deref().is_some_and(|p| publisher_matches(&name, p))
            {
                // A vendor container (publisher or first-word match such as `Google` for
                // `Google Chrome`): only the program's own child is listed, never the container.
                for child in fs.list_dir(&entry.path).into_iter().filter(|e| e.is_dir) {
                    if child_matches_in_container(&file_name(&child.path), &name, &hint.name) && !is_install(&child.path) {
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
    mark_used_by_others(&mut out, &hint.other_locations);
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
    fn install_location_with_userdata_child_is_medium_with_note() {
        let fs = FakeFs::new().dir(r"D:\Games\Foo\bin", 10).dir(r"D:\Games\Foo\SaveGames", 5).profile(r"C:\Users\A");
        let out = scan_folders(&fs, &roots(), &hint("Foo", None, Some(r"D:\Games\Foo")));
        assert_eq!(dirs(&out), vec![(r"D:\Games\Foo".to_string(), Confidence::Medium)]);
        assert_eq!(out[0].note.as_deref(), Some(NOTE_CONTAINS_USER_DATA));
        assert_eq!(NOTE_CONTAINS_USER_DATA, "contains-user-data");

        // A file with such a name is not a user-data folder.
        let fs = FakeFs::new().dir(r"D:\Games\Foo\bin", 10).file(r"D:\Games\Foo\saves", 5).profile(r"C:\Users\A");
        let out = scan_folders(&fs, &roots(), &hint("Foo", None, Some(r"D:\Games\Foo")));
        assert_eq!(dirs(&out), vec![(r"D:\Games\Foo".to_string(), Confidence::High)]);
        assert_eq!(out[0].note, None);
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
    fn first_word_vendor_folder_lists_only_matching_child() {
        let fs = FakeFs::new()
            .dir(r"C:\Program Files\Google\Chrome", 10)
            .dir(r"C:\Program Files\Google\Update", 3);
        let out = scan_folders(&fs, &roots(), &hint("Google Chrome", None, None));
        assert_eq!(dirs(&out), vec![(r"C:\Program Files\Google\Chrome".to_string(), Confidence::Medium)]);

        // A container holding only the program's folder: the child is listed, never the container.
        let fs = FakeFs::new().dir(r"C:\Program Files\Google\Chrome", 10);
        let out = scan_folders(&fs, &roots(), &hint("Google Chrome", None, None));
        assert_eq!(dirs(&out), vec![(r"C:\Program Files\Google\Chrome".to_string(), Confidence::Medium)]);
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
    fn install_folder_from_uninstall_string_is_high() {
        let dir = r"C:\Users\A\AppData\Local\Programs\Obsidian";
        let fs = FakeFs::new().dir(dir, 7).profile(r"C:\Users\A");
        let mut h = hint("Obsidian", None, None);
        h.uninstall_string = Some(format!(r#""{dir}\Uninstall Obsidian.exe" /currentuser"#));
        let out = scan_folders(&fs, &roots(), &h);
        assert_eq!(dirs(&out), vec![(dir.to_string(), Confidence::High)]);
    }

    #[test]
    fn obsidian_updater_folder_is_medium_suffix_match() {
        let fs = FakeFs::new()
            .dir(r"C:\Users\A\AppData\Local\obsidian-updater", 325)
            .dir(r"C:\Users\A\AppData\Local\obsidian-plugins", 1)
            .profile(r"C:\Users\A");
        let out = scan_folders(&fs, &roots(), &hint("Obsidian", None, None));
        assert_eq!(dirs(&out), vec![(r"C:\Users\A\AppData\Local\obsidian-updater".to_string(), Confidence::Medium)]);
        assert_eq!(out[0].note.as_deref(), Some("suffix-match"));

        // A `-data` folder may hold the user's own databases: Low.
        let fs = FakeFs::new().dir(r"C:\Users\A\AppData\Roaming\Obsidian_Data", 3).profile(r"C:\Users\A");
        let out = scan_folders(&fs, &roots(), &hint("Obsidian", None, None));
        assert_eq!(dirs(&out), vec![(r"C:\Users\A\AppData\Roaming\Obsidian_Data".to_string(), Confidence::Low)]);
        assert_eq!(out[0].note.as_deref(), Some("suffix-match"));
    }

    #[test]
    fn generic_name_only_matches_by_path() {
        let fs = FakeFs::new().dir(r"C:\Program Files\Tools", 1);
        let out = scan_folders(&fs, &roots(), &hint("Tools", None, None));
        assert!(out.is_empty());
    }

    #[test]
    fn install_location_under_downloads_is_low_with_note() {
        let fs = FakeFs::new().dir(r"C:\Users\A\Downloads\Foo", 9).profile(r"C:\Users\A");
        let out = scan_folders(&fs, &roots(), &hint("Foo Studio", None, Some(r"C:\Users\A\Downloads\Foo")));
        assert_eq!(dirs(&out), vec![(r"C:\Users\A\Downloads\Foo".to_string(), Confidence::Low)]);
        assert_eq!(out[0].note.as_deref(), Some("user-folder"));
    }

    #[test]
    fn install_location_equal_to_documents_is_not_listed() {
        let fs = FakeFs::new().dir(r"C:\Users\A\Documents", 9).profile(r"C:\Users\A");
        let out = scan_folders(&fs, &roots(), &hint("Foo Studio", None, Some(r"C:\Users\A\Documents")));
        assert!(out.is_empty());
    }

    #[test]
    fn drive_root_install_location_is_not_listed() {
        let fs = FakeFs::new().dir(r"C:\Foo", 1);
        let out = scan_folders(&fs, &roots(), &hint("Foo Studio", None, Some(r"C:\")));
        assert!(out.is_empty());
    }

    #[test]
    fn trailing_backslash_install_location_dedupes() {
        let fs = FakeFs::new().dir(r"C:\Program Files\Foo Studio", 4);
        let out = scan_folders(&fs, &roots(), &hint("Foo Studio", None, Some(r"C:\Program Files\Foo Studio\")));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].confidence, Confidence::High);
    }

    fn installed(id: &str, name: &str, install: &str) -> crate::model::Program {
        use crate::model::{Hive, Program, ProgramId, RegKeyPath, Scope};
        Program {
            account: None,
            id: ProgramId(id.into()),
            key: RegKeyPath { hive: Hive::LocalMachine, path: format!(r"Software\{id}") },
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
            drive: Some('C'),
            hidden: false,
            scope: Scope::Machine,
        }
    }

    fn with_others(h: ProgramHint, me: &str, programs: &[crate::model::Program]) -> ProgramHint {
        h.with_other_programs(programs, &crate::model::ProgramId(me.into()))
    }

    fn find<'a>(items: &'a [Leftover], path: &str) -> &'a Leftover {
        items
            .iter()
            .find(|l| matches!(&l.kind, LeftoverKind::Dir { path: p, .. } if norm(p) == norm(Path::new(path))))
            .unwrap_or_else(|| panic!("{path} not listed: {items:?}"))
    }

    #[test]
    fn sibling_version_folder_is_low_with_note() {
        let foundation = r"C:\Program Files\Blender Foundation";
        let (v36, v40) = (format!(r"{foundation}\Blender 3.6"), format!(r"{foundation}\Blender 4.0"));
        let fs = FakeFs::new().dir(&v36, 10).dir(&v40, 20).profile(r"C:\Users\A");
        let programs = [installed("b36", "Blender 3.6", &v36), installed("b40", "Blender 4.0", &v40)];
        let h = with_others(hint("Blender 3.6", Some("Blender Foundation"), Some(&v36)), "b36", &programs);
        let out = scan_folders(&fs, &roots(), &h);
        let mine = find(&out, &v36);
        assert_eq!((mine.confidence, mine.note.as_deref()), (Confidence::High, None));
        let sibling = find(&out, &v40);
        assert_eq!((sibling.confidence, sibling.note.as_deref()), (Confidence::Low, Some("used-by-other-program")));
        assert!(out.iter().filter(|l| l.confidence != Confidence::Low).all(|l| l.size() == Some(10)), "{out:?}");

        // Without the other program the sibling would have been preselected.
        let out = scan_folders(&fs, &roots(), &hint("Blender 3.6", Some("Blender Foundation"), Some(&v36)));
        assert_eq!(find(&out, &v40).confidence, Confidence::Medium);
    }

    #[test]
    fn python_parent_folder_is_low() {
        let base = r"C:\Users\A\AppData\Local\Programs\Python";
        let (p311, p312) = (format!(r"{base}\Python311"), format!(r"{base}\Python312"));
        let fs = FakeFs::new().dir(&p311, 10).dir(&p312, 20).profile(r"C:\Users\A");
        let programs = [
            installed("py311", "Python 3.11.4 (64-bit)", &format!(r"{p311}\")),
            installed("py312", "Python 3.12.0 (64-bit)", &format!(r"{p312}\")),
        ];
        let h = with_others(hint("Python 3.11.4 (64-bit)", None, Some(&format!(r"{p311}\"))), "py311", &programs);
        let out = scan_folders(&fs, &roots(), &h);
        assert_eq!(find(&out, &p311).confidence, Confidence::High);
        let parent = find(&out, base);
        assert_eq!((parent.confidence, parent.note.as_deref()), (Confidence::Low, Some("used-by-other-program")));
    }

    #[test]
    fn install_folder_shared_with_another_product_is_low() {
        let dir = r"C:\Program Files\Vendor Suite";
        let fs = FakeFs::new().dir(dir, 10);
        let programs = [installed("tool", "Vendor Tool", dir), installed("suite", "Vendor Suite", dir)];
        let h = with_others(hint("Vendor Tool", None, Some(dir)), "tool", &programs);
        let out = scan_folders(&fs, &roots(), &h);
        assert_eq!(find(&out, dir).confidence, Confidence::Low);
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
