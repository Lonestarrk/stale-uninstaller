//! Disk usage of install folders.

use std::path::{Path, PathBuf};

/// Total size of all files under `root`, or `None` when `root` is missing or unreadable.
/// Symlinks and junctions are not followed; unreadable entries below `root` are skipped.
pub fn dir_size(root: &Path) -> Option<u64> {
    dir_size_partial(root).map(|(total, _)| total)
}

/// [`dir_size`] plus whether anything below `root` could not be read (so the size is a lower bound).
pub fn dir_size_partial(root: &Path) -> Option<(u64, bool)> {
    let mut total = 0;
    let mut partial = false;
    let mut stack = vec![];
    for entry in std::fs::read_dir(root).ok()? {
        match entry {
            Ok(entry) => add_entry(&entry, &mut total, &mut stack, &mut partial),
            Err(_) => partial = true,
        }
    }
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            partial = true;
            continue;
        };
        for entry in entries {
            match entry {
                Ok(entry) => add_entry(&entry, &mut total, &mut stack, &mut partial),
                Err(_) => partial = true,
            }
        }
    }
    Some((total, partial))
}

/// Adds a file's size to `total` or queues a directory; links are skipped.
fn add_entry(entry: &std::fs::DirEntry, total: &mut u64, stack: &mut Vec<PathBuf>, partial: &mut bool) {
    // DirEntry::metadata does not traverse links; junctions report as symlinks.
    let Ok(meta) = entry.metadata() else {
        *partial = true;
        return;
    };
    if meta.file_type().is_symlink() {
        return;
    }
    if meta.is_dir() {
        stack.push(entry.path());
    } else {
        *total += meta.len();
    }
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

/// Subfolder levels below the starting folder that `find_exes` searches.
const FIND_EXES_MAX_DEPTH: usize = 3;

/// Files named `name` (case-insensitive) in `dir` and up to three subfolder levels below it.
/// Symlinks and junctions are not followed; unreadable folders are skipped.
pub fn find_exes(dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if depth < FIND_EXES_MAX_DEPTH {
                    stack.push((entry.path(), depth + 1));
                }
            } else if entry.file_name().to_string_lossy().eq_ignore_ascii_case(name) {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_exes_searches_three_levels_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for sub in ["app-1.0", r"a\b\c", r"a\b\c\d"] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
        }
        for f in ["Discord.exe", r"app-1.0\DISCORD.EXE", r"app-1.0\Update.exe", r"a\b\c\discord.exe", r"a\b\c\d\Discord.exe"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let mut expected = vec![d.join("Discord.exe"), d.join(r"app-1.0\DISCORD.EXE"), d.join(r"a\b\c\discord.exe")];
        expected.sort();
        assert_eq!(find_exes(d, "Discord.exe"), expected);
        assert!(find_exes(&d.join("missing"), "Discord.exe").is_empty());
    }

    #[test]
    fn dir_size_sums_files_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.path().join("a").join("ten.bin"), [0u8; 10]).unwrap();
        std::fs::write(nested.join("twenty.bin"), [0u8; 20]).unwrap();
        assert_eq!(dir_size(dir.path()), Some(30));
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(dir_size(empty.path()), Some(0));
    }

    #[test]
    fn dir_size_missing_root_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(dir_size(&dir.path().join("missing")), None);
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
