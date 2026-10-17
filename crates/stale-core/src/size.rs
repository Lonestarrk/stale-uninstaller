//! Disk usage of install folders.

use std::path::{Path, PathBuf};

/// Total size of all files under `root`. Symlinks and junctions are not followed;
/// unreadable entries are skipped. A missing `root` is 0.
pub fn dir_size(root: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            // DirEntry::metadata does not traverse links; junctions report as symlinks.
            let Ok(meta) = entry.metadata() else { continue };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

/// `*.exe` files directly in `dir` (not in subfolders) with their sizes.
pub fn list_root_exes(dir: &Path) -> Vec<(PathBuf, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            let path = entry.path();
            let is_exe = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"));
            (meta.is_file() && is_exe).then_some((path, meta.len()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_size_sums_files_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.path().join("a").join("ten.bin"), [0u8; 10]).unwrap();
        std::fs::write(nested.join("twenty.bin"), [0u8; 20]).unwrap();
        assert_eq!(dir_size(dir.path()), 30);
        assert_eq!(dir_size(&dir.path().join("missing")), 0);
    }

    #[test]
    fn list_root_exes_is_non_recursive_and_exe_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("App.EXE"), [0u8; 5]).unwrap();
        std::fs::write(dir.path().join("readme.txt"), [0u8; 3]).unwrap();
        std::fs::write(dir.path().join("sub").join("deep.exe"), [0u8; 7]).unwrap();
        assert_eq!(list_root_exes(dir.path()), vec![(dir.path().join("App.EXE"), 5)]);
    }
}
