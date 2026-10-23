//! The drill-down below an opened space entry: its direct children, largest first, each marked
//! with why it may not be removed, if so.

use super::scan::never_listed;
use crate::fsys::FsReader;
use crate::ghosts::InstalledIndex;
use crate::own::OwnPaths;
use crate::pathutil::{drive_letter, is_shared_root, norm, normed_under};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// How many children the drill-down shows; the rest is only counted.
pub const BROWSE_LIMIT: usize = 200;

/// Why a child is shown without a way to remove it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockReason {
    /// Windows, Program Files, a drive root or another system folder.
    System,
    /// An installed program lives here (or this is inside its folder).
    InstalledProgram,
    /// A profile folder itself.
    OtherProfileRoot,
    /// Stale Uninstaller's own folder or file.
    Own,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Child {
    pub path: PathBuf,
    pub is_dir: bool,
    /// `None` when a folder could not be measured.
    pub size: Option<u64>,
    pub blocked: Option<BlockReason>,
}

/// `\PROGRAM FILES` or `\PROGRAM FILES (X86)` on any drive, or anything below ([`norm`]ed input).
fn in_program_files(n: &str) -> bool {
    if drive_letter(Path::new(n)).is_none() {
        return false;
    }
    let Some(rest) = n.get(2..) else { return false };
    [r"\PROGRAM FILES", r"\PROGRAM FILES (X86)"].iter().any(|pf| rest == *pf || rest.strip_prefix(pf).is_some_and(|r| r.starts_with('\\')))
}

/// `windows` and `profiles` are [`norm`]ed.
fn block_reason(path: &Path, windows: &str, profiles: &[String], installed: &InstalledIndex, own: &OwnPaths) -> Option<BlockReason> {
    let n = norm(path);
    if profiles.contains(&n) {
        return Some(BlockReason::OtherProfileRoot);
    }
    let on_drive_root = path.parent().is_some_and(|p| norm(p).len() == 2 && drive_letter(p).is_some());
    let name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    if n.is_empty()
        || n == windows
        || normed_under(&n, windows)
        || in_program_files(&n)
        || is_shared_root(path)
        || (on_drive_root && never_listed(&name))
    {
        return Some(BlockReason::System);
    }
    if own.is_own_path(path) {
        return Some(BlockReason::Own);
    }
    (installed.covers_path(path) || installed.holds_location(path)).then_some(BlockReason::InstalledProgram)
}

/// The direct children of `dir`, largest first (unknown sizes last), at most `limit` of them, and
/// how many more there are. Links are skipped; folders are measured.
pub fn children(fs: &dyn FsReader, dir: &Path, installed: &InstalledIndex, own: &OwnPaths, limit: usize) -> (Vec<Child>, usize) {
    let windows = norm(&fs.windows_dir());
    let profiles: Vec<String> = fs.profile_dirs().iter().map(|p| norm(p)).collect();
    let mut out: Vec<Child> = fs
        .list_dir(dir)
        .into_iter()
        .filter(|e| !fs.is_reparse(&e.path))
        .map(|e| {
            let size = if e.is_dir { fs.dir_size_partial(&e.path).map(|(s, _)| s) } else { Some(e.size) };
            let blocked = block_reason(&e.path, &windows, &profiles, installed, own);
            Child { path: e.path, is_dir: e.is_dir, size, blocked }
        })
        .collect();
    out.sort_by(|a, b| {
        let by_size = match (a.size, b.size) {
            (Some(x), Some(y)) => y.cmp(&x),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        by_size.then_with(|| norm(&a.path).cmp(&norm(&b.path)))
    });
    let more = out.len().saturating_sub(limit);
    out.truncate(limit);
    (out, more)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsys::FakeFs;
    use crate::model::{Hive, Program, ProgramId, RegKeyPath, Scope};

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

    fn none() -> (InstalledIndex, OwnPaths) {
        (InstalledIndex::default(), OwnPaths::default())
    }

    #[test]
    fn children_sorted_largest_first_with_limit_and_more_count() {
        let fs = FakeFs::new()
            .file(r"D:\Data\small.bin", 10)
            .dir(r"D:\Data\Big", 500)
            .file(r"D:\Data\mid.bin", 100)
            .dir(r"D:\Data\Tiny", 1);
        let (i, o) = none();
        let (all, more) = children(&fs, Path::new(r"D:\Data"), &i, &o, 10);
        assert_eq!(more, 0);
        assert_eq!(all.iter().map(|c| c.size).collect::<Vec<_>>(), vec![Some(500), Some(100), Some(10), Some(1)]);
        let (top, more) = children(&fs, Path::new(r"D:\Data"), &i, &o, 2);
        assert_eq!((top.len(), more), (2, 2));
        assert!(top[0].path.ends_with("Big") && top[0].is_dir && top[1].path.ends_with("mid.bin") && !top[1].is_dir);
        assert!(top.iter().all(|c| c.blocked.is_none()));
        assert_eq!(children(&fs, Path::new(r"D:\Data"), &i, &o, 0).1, 4);
    }

    #[test]
    fn system_installed_and_own_children_are_blocked() {
        let fs = FakeFs::new()
            .dir(r"C:\Windows\System32", 5)
            .dir(r"C:\Program Files\Foo", 4)
            .dir(r"C:\Program Files (x86)\Bar", 4)
            .dir(r"C:\Users\Other", 3)
            .dir(r"C:\$Recycle.Bin", 3)
            .dir(r"C:\Stale Uninstaller\quarantine", 2)
            .dir(r"C:\Apps\Tool\bin", 2)
            .dir(r"C:\Apps\Suite\data", 2)
            .dir(r"C:\Free", 1)
            .profile(r"C:\Users\Other");
        let installed = InstalledIndex::from_programs(&[program("Tool", r"C:\Apps\Tool\bin"), program("Suite", r"C:\Apps\Suite")]);
        let own = OwnPaths::from_folders([PathBuf::from(r"C:\Stale Uninstaller")]);
        let at = |d: &str, name: &str| {
            let (v, _) = children(&fs, Path::new(d), &installed, &own, 10);
            v.into_iter().find(|c| c.path.ends_with(name)).unwrap().blocked
        };
        for name in ["Windows", "Program Files", "Program Files (x86)", "$Recycle.Bin", "Users"] {
            assert_eq!(at(r"C:\", name), Some(BlockReason::System), "{name}");
        }
        assert_eq!(at(r"C:\", "Stale Uninstaller"), Some(BlockReason::Own));
        assert_eq!(at(r"C:\", "Apps"), Some(BlockReason::InstalledProgram));
        assert_eq!(at(r"C:\", "Free"), None);
        assert_eq!(at(r"C:\Users", "Other"), Some(BlockReason::OtherProfileRoot));
        assert_eq!(at(r"C:\Windows", "System32"), Some(BlockReason::System));
        assert_eq!(at(r"C:\Program Files", "Foo"), Some(BlockReason::System));
        assert_eq!(at(r"C:\Stale Uninstaller", "quarantine"), Some(BlockReason::Own));
        // Equal to, holding, and inside an installed location.
        assert_eq!(at(r"C:\Apps", "Tool"), Some(BlockReason::InstalledProgram));
        assert_eq!(at(r"C:\Apps\Tool", "bin"), Some(BlockReason::InstalledProgram));
        assert_eq!(at(r"C:\Apps\Suite", "data"), Some(BlockReason::InstalledProgram));
    }

    #[test]
    fn links_are_skipped() {
        let fs = FakeFs::new().dir(r"D:\Data\Real", 5).dir(r"D:\Data\Link", 9).reparse(r"D:\Data\Link").file(r"D:\Data\f.bin", 1);
        let (i, o) = none();
        let (c, more) = children(&fs, Path::new(r"D:\Data"), &i, &o, 10);
        assert_eq!(more, 0);
        let names: Vec<String> = c.iter().map(|c| c.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["Real", "f.bin"]);
    }
}
