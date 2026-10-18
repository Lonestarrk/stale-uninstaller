//! Moving quarantined files: [`RealFileMover`] for production, [`FakeFileMover`] for tests.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::QuarantineId;
use crate::pathutil::{drive_letter, norm};

pub trait FileMover: Send + Sync {
    /// Moves a file or folder; creates `to`'s parent folders. Across volumes it copies, then deletes.
    fn move_path(&self, from: &Path, to: &Path) -> Result<(), String>;
    /// Creates `root` and restricts it to Administrators and SYSTEM (a failed ACL change is only logged).
    fn ensure_payload_root(&self, root: &Path) -> Result<(), String>;
    /// Bytes of a file, or of all files below a folder; 0 when missing.
    fn size_of(&self, p: &Path) -> u64;
    fn exists(&self, p: &Path) -> bool;
    /// Deletes a file or folder tree; a missing path is not an error.
    fn remove_all(&self, p: &Path) -> Result<(), String>;
    /// Whether `<drive>:\` is a writable NTFS volume that can hold a payload root (else use C:).
    fn volume_ok(&self, drive: char) -> bool;
}

/// `root\<id>\<DRIVE>\<original without drive>`, e.g. `D:\Program Files\Unity Hub` →
/// `D:\Stale Uninstaller\quarantine\<id>\D\Program Files\Unity Hub`. `None` for originals without a
/// drive letter (UNC, relative) and for a bare drive root.
pub fn stored_path_for(root: &Path, id: &QuarantineId, original: &Path) -> Option<PathBuf> {
    let drive = drive_letter(original)?;
    let rest = original.to_str()?[2..].trim_matches(['\\', '/']);
    if rest.is_empty() {
        return None;
    }
    Some(root.join(&id.0).join(drive.to_string()).join(rest))
}

const ERROR_NOT_SAME_DEVICE: i32 = 17;
const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;

/// Moves files on the real file system.
pub struct RealFileMover;

/// Copies `from` (file or folder tree) to `to`. Symlinks and junctions are skipped with a warning.
fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    let md = std::fs::symlink_metadata(from)?;
    if md.file_type().is_symlink() {
        log::warn!("quarantine: skipping link {}", from.display());
        return Ok(());
    }
    if !md.is_dir() {
        return std::fs::copy(from, to).map(|_| ());
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
    }
    Ok(())
}

fn remove_any(p: &Path) -> std::io::Result<()> {
    let md = std::fs::symlink_metadata(p)?;
    if md.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) }
}

/// The cross-volume fallback: copy, then delete the source. A failed copy removes the partial
/// copy; a failed delete leaves the full copy at `to` and returns the error.
fn move_by_copy(from: &Path, to: &Path) -> Result<(), String> {
    if let Err(e) = copy_recursive(from, to) {
        let _ = remove_any(to);
        return Err(format!("copy {} -> {}: {e}", from.display(), to.display()));
    }
    remove_any(from).map_err(|e| format!("remove {} after copy: {e}", from.display()))
}

impl FileMover for RealFileMover {
    fn move_path(&self, from: &Path, to: &Path) -> Result<(), String> {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        match std::fs::rename(from, to) {
            Ok(()) => Ok(()),
            Err(e) if e.raw_os_error() == Some(ERROR_NOT_SAME_DEVICE) => move_by_copy(from, to),
            Err(e) => Err(format!("move {} -> {}: {e}", from.display(), to.display())),
        }
    }

    fn ensure_payload_root(&self, root: &Path) -> Result<(), String> {
        std::fs::create_dir_all(root).map_err(|e| format!("create {}: {e}", root.display()))?;
        let args = [
            root.as_os_str(),
            "/inheritance:r".as_ref(),
            "/grant:r".as_ref(),
            "*S-1-5-32-544:(OI)(CI)F".as_ref(),
            "*S-1-5-18:(OI)(CI)F".as_ref(),
        ];
        if let Err(e) = crate::regwrite::run_hidden("icacls", &args) {
            log::warn!("quarantine: icacls on {} failed: {e}", root.display());
        }
        Ok(())
    }

    fn size_of(&self, p: &Path) -> u64 {
        match std::fs::symlink_metadata(p) {
            Ok(md) if md.is_dir() => crate::size::dir_size(p).unwrap_or(0),
            Ok(md) => md.len(),
            Err(_) => 0,
        }
    }

    fn exists(&self, p: &Path) -> bool {
        std::fs::symlink_metadata(p).is_ok()
    }

    fn remove_all(&self, p: &Path) -> Result<(), String> {
        match remove_any(p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r.map_err(|e| format!("remove {}: {e}", p.display())),
        }
    }

    fn volume_ok(&self, drive: char) -> bool {
        use windows::Win32::Storage::FileSystem::GetVolumeInformationW;
        use windows::core::PCWSTR;
        if !drive.is_ascii_alphabetic() {
            return false;
        }
        let root = format!(r"{}:\", drive.to_ascii_uppercase());
        if !Path::new(&root).exists() {
            return false;
        }
        let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        let mut flags = 0u32;
        let mut fs_name = [0u16; 64];
        // SAFETY: `wide` is NUL-terminated and outlives the call; the out-pointers are valid locals.
        let ok = unsafe {
            GetVolumeInformationW(PCWSTR(wide.as_ptr()), None, None, None, Some(&mut flags), Some(&mut fs_name))
        }
        .is_ok();
        let len = fs_name.iter().position(|&c| c == 0).unwrap_or(fs_name.len());
        ok && String::from_utf16_lossy(&fs_name[..len]) == "NTFS" && flags & FILE_READ_ONLY_VOLUME == 0
    }
}

/// In-memory file tree for tests. Paths compare case-insensitively; folders that only hold
/// added files exist implicitly. Every `move_path`/`remove_all`/`ensure_payload_root` call is logged.
#[derive(Default)]
pub struct FakeFileMover {
    /// norm(path) → (path, Some(size) for a file / None for a folder).
    entries: Mutex<BTreeMap<String, (PathBuf, Option<u64>)>>,
    /// norm(path) → path.
    locked: Mutex<BTreeMap<String, PathBuf>>,
    volumes: Mutex<BTreeMap<char, bool>>,
    /// `"move <from> -> <to>"`, `"remove <p>"`, `"ensure <root>"`.
    pub log: Mutex<Vec<String>>,
}

/// `key` is `base` or below it (both normalized).
fn under(key: &str, base: &str) -> bool {
    key.strip_prefix(base).is_some_and(|rest| rest.is_empty() || rest.starts_with('\\'))
}

impl FakeFileMover {
    pub fn new() -> FakeFileMover {
        FakeFileMover::default()
    }

    pub fn add_dir(&self, path: &str) {
        let p = PathBuf::from(path);
        self.entries.lock().unwrap().insert(norm(&p), (p, None));
    }

    pub fn add_file(&self, path: &str, size: u64) {
        let p = PathBuf::from(path);
        self.entries.lock().unwrap().insert(norm(&p), (p, Some(size)));
    }

    /// Makes any move or removal of `path`, or of a folder containing it, fail.
    pub fn lock(&self, path: &str) {
        let p = PathBuf::from(path);
        self.locked.lock().unwrap().insert(norm(&p), p);
    }

    /// Volumes are OK unless set otherwise.
    pub fn set_volume_ok(&self, drive: char, ok: bool) {
        self.volumes.lock().unwrap().insert(drive.to_ascii_uppercase(), ok);
    }

    fn check_locked(&self, p: &Path) -> Result<(), String> {
        let base = norm(p);
        match self.locked.lock().unwrap().iter().find(|(k, _)| under(k, &base)) {
            Some((_, path)) => Err(format!("locked: {}", path.display())),
            None => Ok(()),
        }
    }
}

impl FileMover for FakeFileMover {
    fn move_path(&self, from: &Path, to: &Path) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("move {} -> {}", from.display(), to.display()));
        if !self.exists(from) {
            return Err(format!("not found: {}", from.display()));
        }
        if self.exists(to) {
            return Err(format!("already exists: {}", to.display()));
        }
        self.check_locked(from)?;
        let base = norm(from);
        let depth = from.components().count();
        let mut entries = self.entries.lock().unwrap();
        let moving: Vec<String> = entries.keys().filter(|k| under(k, &base)).cloned().collect();
        for k in moving {
            let (path, size) = entries.remove(&k).unwrap();
            let dest: PathBuf = to.components().chain(path.components().skip(depth)).collect();
            entries.insert(norm(&dest), (dest, size));
        }
        Ok(())
    }

    fn ensure_payload_root(&self, root: &Path) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("ensure {}", root.display()));
        if !self.exists(root) {
            self.entries.lock().unwrap().insert(norm(root), (root.to_path_buf(), None));
        }
        Ok(())
    }

    fn size_of(&self, p: &Path) -> u64 {
        let base = norm(p);
        self.entries.lock().unwrap().iter().filter(|(k, _)| under(k, &base)).filter_map(|(_, (_, s))| *s).sum()
    }

    fn exists(&self, p: &Path) -> bool {
        let base = norm(p);
        self.entries.lock().unwrap().keys().any(|k| under(k, &base))
    }

    fn remove_all(&self, p: &Path) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("remove {}", p.display()));
        self.check_locked(p)?;
        let base = norm(p);
        self.entries.lock().unwrap().retain(|k, _| !under(k, &base));
        Ok(())
    }

    fn volume_ok(&self, drive: char) -> bool {
        self.volumes.lock().unwrap().get(&drive.to_ascii_uppercase()).copied().unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> QuarantineId {
        QuarantineId("20261018-120000-ab12".to_string())
    }

    #[test]
    fn stored_path_keeps_relative_structure() {
        let root = Path::new(r"D:\Stale Uninstaller\quarantine");
        assert_eq!(
            stored_path_for(root, &id(), Path::new(r"D:\Program Files\Unity Hub")),
            Some(PathBuf::from(r"D:\Stale Uninstaller\quarantine\20261018-120000-ab12\D\Program Files\Unity Hub"))
        );
        assert_eq!(
            stored_path_for(root, &id(), Path::new(r"c:\Users\A\AppData\Local\Unity\")),
            Some(PathBuf::from(r"D:\Stale Uninstaller\quarantine\20261018-120000-ab12\C\Users\A\AppData\Local\Unity"))
        );
        assert_eq!(stored_path_for(root, &id(), Path::new(r"\server\share\Unity")), None);
        assert_eq!(stored_path_for(root, &id(), Path::new(r"D:\")), None);
    }

    #[test]
    fn fake_mover_moves_and_reports_locked_file() {
        let m = FakeFileMover::new();
        m.add_dir(r"C:\App");
        m.add_file(r"C:\App\a.dll", 10);
        m.add_file(r"C:\App\sub\b.dll", 5);
        m.add_file(r"C:\Other\c.txt", 3);
        m.add_file(r"C:\Apples.txt", 1);
        m.lock(r"C:\Other\c.txt");
        m.set_volume_ok('E', false);
        assert_eq!(m.size_of(Path::new(r"c:\app")), 15);

        let dest = Path::new(r"D:\Q\x\C\App");
        m.move_path(Path::new(r"C:\App"), dest).unwrap();
        assert!(!m.exists(Path::new(r"C:\App")));
        assert!(!m.exists(Path::new(r"C:\App\sub\b.dll")));
        assert!(m.exists(Path::new(r"C:\Apples.txt")), "sibling with a shared name prefix stays");
        assert!(m.exists(Path::new(r"D:\Q\x\C\App\sub\b.dll")));
        assert_eq!(m.size_of(dest), 15);

        assert_eq!(
            m.move_path(Path::new(r"C:\Other"), Path::new(r"D:\Q\x\C\Other")),
            Err(r"locked: C:\Other\c.txt".to_string())
        );
        assert!(m.exists(Path::new(r"C:\Other\c.txt")));
        assert!(!m.exists(Path::new(r"D:\Q\x\C\Other")));

        m.remove_all(dest).unwrap();
        assert!(!m.exists(dest));
        assert!(m.volume_ok('D'));
        assert!(!m.volume_ok('e'));
        assert_eq!(
            *m.log.lock().unwrap(),
            vec![
                r"move C:\App -> D:\Q\x\C\App",
                r"move C:\Other -> D:\Q\x\C\Other",
                r"remove D:\Q\x\C\App",
            ]
        );
    }

    fn tree(root: &Path) -> PathBuf {
        let src = root.join("src").join("App");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.dll"), b"0123456789").unwrap();
        std::fs::write(src.join("sub").join("b.dll"), b"01234").unwrap();
        src
    }

    #[test]
    fn real_mover_moves_tree_and_creates_parents() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tree(tmp.path());
        let dest = tmp.path().join("q").join("id").join("C").join("App");
        let m = RealFileMover;
        assert_eq!(m.size_of(&src), 15);
        m.move_path(&src, &dest).unwrap();
        assert!(!m.exists(&src));
        assert_eq!(std::fs::read(dest.join("sub").join("b.dll")).unwrap(), b"01234");
        assert_eq!(m.size_of(&dest.join("a.dll")), 10);
        m.remove_all(&dest).unwrap();
        assert!(!m.exists(&dest));
        m.remove_all(&dest).unwrap();
        assert!(m.move_path(&src, &dest).is_err());
    }

    #[test]
    fn move_by_copy_copies_tree_then_removes_source() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tree(tmp.path());
        let dest = tmp.path().join("dest");
        move_by_copy(&src, &dest).unwrap();
        assert!(!src.exists());
        assert_eq!(std::fs::read(dest.join("a.dll")).unwrap(), b"0123456789");
        assert_eq!(std::fs::read(dest.join("sub").join("b.dll")).unwrap(), b"01234");

        let file = tmp.path().join("one.txt");
        std::fs::write(&file, b"x").unwrap();
        move_by_copy(&file, &tmp.path().join("two.txt")).unwrap();
        assert!(!file.exists() && tmp.path().join("two.txt").is_file());
        assert!(move_by_copy(&file, &tmp.path().join("three.txt")).is_err());
        assert!(!tmp.path().join("three.txt").exists());
    }

    #[test]
    #[ignore]
    fn real_volume_ok_for_system_drive() {
        assert!(RealFileMover.volume_ok('c'));
        assert!(!RealFileMover.volume_ok('1'));
    }
}
