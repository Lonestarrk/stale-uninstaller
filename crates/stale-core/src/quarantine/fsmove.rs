//! Moving quarantined files: [`RealFileMover`] for production, [`FakeFileMover`] for tests.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::{codes, QuarantineId};
use crate::pathutil::{drive_letter, norm};

/// Why [`FileMover::move_path`] did not complete.
#[derive(Clone, Debug, PartialEq)]
pub enum MoveError {
    /// The destination already exists; nothing was touched.
    Exists,
    /// The path (the source or something below it) is in use; nothing was moved.
    Locked(PathBuf),
    /// A symlink/junction was found in the tree of a cross-volume move; nothing was moved.
    LinkInTree(PathBuf),
    /// Everything was copied to `stored`, but the source could not be (fully) deleted.
    /// The copy is kept so it can be recorded and restored.
    PartialMove { stored: PathBuf, detail: String },
    Other(String),
}

impl fmt::Display for MoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MoveError::Exists => f.write_str(codes::DESTINATION_EXISTS),
            MoveError::Locked(p) => write!(f, "{}: {}", codes::LOCKED, p.display()),
            MoveError::LinkInTree(p) => write!(f, "{}: {}", codes::LINK_IN_TREE, p.display()),
            MoveError::PartialMove { stored, detail } => {
                write!(f, "{}: copied to {}; {detail}", codes::PARTIAL_MOVE, stored.display())
            }
            MoveError::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for MoveError {}

pub trait FileMover: Send + Sync {
    /// Moves a file or folder; creates `to`'s parent folders and never overwrites an existing `to`.
    /// Across volumes it copies the whole tree, verifies it, then deletes the source.
    fn move_path(&self, from: &Path, to: &Path) -> Result<(), MoveError>;
    /// Creates `root` (its parent must exist) and locks it down with [`crate::acl::secure_dir`]:
    /// owner Administrators, a protected DACL of exactly Administrators + SYSTEM full control
    /// (inherited below), untrusted children renamed aside. Refuses a root (or parent) that is a
    /// reparse point; a failed lock-down is fatal only for a root that already existed.
    fn ensure_payload_root(&self, root: &Path) -> Result<(), String>;
    /// Creates the folder `p` (its parent must exist) with the locked Administrators + SYSTEM
    /// descriptor and owner Administrators ([`crate::acl::create_locked`]), so the lock-down sweep
    /// recognises it as ours whatever the "default owner" policy says. An existing folder is fine.
    fn create_locked_dir(&self, p: &Path) -> Result<(), String>;
    /// Bytes of a file, or of all files below a folder; 0 when missing.
    fn size_of(&self, p: &Path) -> u64;
    fn exists(&self, p: &Path) -> bool;
    /// Whether `p` itself is a symlink, junction or other reparse point (false when missing).
    fn is_reparse(&self, p: &Path) -> bool;
    /// Deletes a file or folder tree; a missing path is not an error.
    fn remove_all(&self, p: &Path) -> Result<(), String>;
    /// Deletes the empty folders of the tree at `p`, bottom-up, never following a link (a link or
    /// any file keeps its folders). True when `p` is gone (or was missing).
    fn remove_empty_dirs(&self, p: &Path) -> Result<bool, String>;
    /// Whether `<drive>:\` is a writable NTFS volume that can hold a payload root (else use C:).
    fn volume_ok(&self, drive: char) -> bool;
}

/// `root\<id>\<DRIVE>\<original without drive>`, e.g. `D:\Program Files\Unity Hub` →
/// `D:\Stale Uninstaller\quarantine\<id>\D\Program Files\Unity Hub`. `None` for originals without a
/// drive letter (UNC, relative), drive-relative paths (`C:foo`), a bare drive root, or any `..`.
pub fn stored_path_for(root: &Path, id: &QuarantineId, original: &Path) -> Option<PathBuf> {
    let drive = drive_letter(original)?;
    let rest = &original.to_str()?[2..];
    if !rest.starts_with(['\\', '/']) {
        return None;
    }
    let rest = rest.trim_matches(['\\', '/']);
    if rest.is_empty() || rest.split(['\\', '/']).any(|c| c == "..") {
        return None;
    }
    Some(root.join(&id.0).join(drive.to_string()).join(rest))
}

const ERROR_NOT_SAME_DEVICE: i32 = 17;
const ERROR_SHARING_VIOLATION: i32 = 32;
const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// Moves files on the real file system.
pub struct RealFileMover;

fn is_reparse(md: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    md.file_type().is_symlink() || md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Whether `p` itself (not its target) is a symlink, junction or other reparse point.
pub(crate) fn is_reparse_point(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|md| is_reparse(&md))
}

fn other(what: &str, p: &Path, e: std::io::Error) -> MoveError {
    MoveError::Other(format!("{what} {}: {e}", p.display()))
}

/// Copies `from` (file or folder tree) to `to`; stops at the first symlink/junction.
fn copy_recursive(from: &Path, to: &Path) -> Result<(), MoveError> {
    let md = std::fs::symlink_metadata(from).map_err(|e| other("read", from, e))?;
    if is_reparse(&md) {
        return Err(MoveError::LinkInTree(from.to_path_buf()));
    }
    if !md.is_dir() {
        return std::fs::copy(from, to).map(|_| ()).map_err(|e| other("copy", from, e));
    }
    std::fs::create_dir_all(to).map_err(|e| other("create", to, e))?;
    for entry in std::fs::read_dir(from).map_err(|e| other("list", from, e))? {
        let entry = entry.map_err(|e| other("list", from, e))?;
        copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
    }
    Ok(())
}

/// Checks that every file below `from` exists below `to` with the same length.
fn verify_copy(from: &Path, to: &Path) -> Result<(), MoveError> {
    let failed = |p: &Path| MoveError::Other(format!("copy verification failed for {}", p.display()));
    let md = std::fs::symlink_metadata(from).map_err(|e| other("read", from, e))?;
    let copy = std::fs::symlink_metadata(to).map_err(|_| failed(from))?;
    if md.is_dir() != copy.is_dir() || (!md.is_dir() && md.len() != copy.len()) {
        return Err(failed(from));
    }
    if md.is_dir() {
        for entry in std::fs::read_dir(from).map_err(|e| other("list", from, e))? {
            let entry = entry.map_err(|e| other("list", from, e))?;
            verify_copy(&entry.path(), &to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// See [`FileMover::remove_empty_dirs`].
fn prune_empty(p: &Path) -> std::io::Result<bool> {
    let md = match std::fs::symlink_metadata(p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        r => r?,
    };
    if is_reparse(&md) || !md.is_dir() {
        return Ok(false);
    }
    let mut empty = true;
    for entry in std::fs::read_dir(p)? {
        if !prune_empty(&entry?.path())? {
            empty = false;
        }
    }
    if empty {
        std::fs::remove_dir(p)?;
    }
    Ok(empty)
}

fn remove_any(p: &Path) -> std::io::Result<()> {
    let md = std::fs::symlink_metadata(p)?;
    if md.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) }
}

/// The cross-volume fallback: copy the whole tree, verify it, then delete the source.
/// A failed copy/verify removes the partial copy (`to` did not exist before) and leaves the source
/// untouched; a failed delete keeps the full copy and returns [`MoveError::PartialMove`].
fn move_by_copy(from: &Path, to: &Path) -> Result<(), MoveError> {
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(MoveError::Exists);
    }
    if let Err(e) = copy_recursive(from, to).and_then(|()| verify_copy(from, to)) {
        let _ = remove_any(to);
        return Err(e);
    }
    remove_any(from).map_err(|e| MoveError::PartialMove {
        stored: to.to_path_buf(),
        detail: format!("remove {}: {e}", from.display()),
    })
}

impl FileMover for RealFileMover {
    fn move_path(&self, from: &Path, to: &Path) -> Result<(), MoveError> {
        if std::fs::symlink_metadata(to).is_ok() {
            return Err(MoveError::Exists);
        }
        std::fs::symlink_metadata(from).map_err(|e| other("read", from, e))?;
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| other("create", parent, e))?;
        }
        match std::fs::rename(from, to) {
            Ok(()) => Ok(()),
            Err(e) if e.raw_os_error() == Some(ERROR_NOT_SAME_DEVICE) => move_by_copy(from, to),
            Err(e) if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => Err(MoveError::Locked(from.to_path_buf())),
            Err(e) => Err(MoveError::Other(format!("move {} -> {}: {e}", from.display(), to.display()))),
        }
    }

    fn ensure_payload_root(&self, root: &Path) -> Result<(), String> {
        for p in root.parent().into_iter().chain([root]) {
            if is_reparse_point(p) {
                return Err(format!("{}: {}", codes::IS_REPARSE, p.display()));
            }
        }
        for warning in crate::acl::secure_dir(root, false)? {
            log::warn!("quarantine: {warning}");
        }
        Ok(())
    }

    fn create_locked_dir(&self, p: &Path) -> Result<(), String> {
        if std::fs::symlink_metadata(p).is_ok() {
            return Ok(());
        }
        crate::acl::create_locked(p)
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

    fn is_reparse(&self, p: &Path) -> bool {
        is_reparse_point(p)
    }

    fn remove_all(&self, p: &Path) -> Result<(), String> {
        match remove_any(p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r.map_err(|e| format!("remove {}: {e}", p.display())),
        }
    }

    fn remove_empty_dirs(&self, p: &Path) -> Result<bool, String> {
        prune_empty(p).map_err(|e| format!("remove {}: {e}", p.display()))
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
        let wide = crate::pathutil::wide(&root);
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
    /// norm(path) of reparse points.
    reparse: Mutex<BTreeMap<String, PathBuf>>,
    /// norm(path) of sources whose move copies but cannot delete the source.
    partial: Mutex<BTreeMap<String, PathBuf>>,
    /// `"move <from> -> <to>"`, `"remove <p>"`, `"prune <p>"`, `"ensure <root>"`, `"create-locked <p>"`.
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

    /// Marks `path` as a symlink/junction for [`FileMover::is_reparse`].
    pub fn set_reparse(&self, path: &str) {
        let p = PathBuf::from(path);
        self.reparse.lock().unwrap().insert(norm(&p), p);
    }

    /// Makes a move of exactly `path` copy it to the destination but keep the source,
    /// returning [`MoveError::PartialMove`] like a failed cross-volume delete.
    pub fn fail_partially(&self, path: &str) {
        let p = PathBuf::from(path);
        self.partial.lock().unwrap().insert(norm(&p), p);
    }

    fn locked_under(&self, p: &Path) -> Option<PathBuf> {
        let base = norm(p);
        self.locked.lock().unwrap().iter().find(|(k, _)| under(k, &base)).map(|(_, path)| path.clone())
    }
}

impl FileMover for FakeFileMover {
    fn move_path(&self, from: &Path, to: &Path) -> Result<(), MoveError> {
        self.log.lock().unwrap().push(format!("move {} -> {}", from.display(), to.display()));
        if self.exists(to) {
            return Err(MoveError::Exists);
        }
        if !self.exists(from) {
            return Err(MoveError::Other(format!("not found: {}", from.display())));
        }
        if let Some(locked) = self.locked_under(from) {
            return Err(MoveError::Locked(locked));
        }
        let base = norm(from);
        let keep_source = self.partial.lock().unwrap().contains_key(&base);
        let depth = from.components().count();
        let mut entries = self.entries.lock().unwrap();
        let moving: Vec<String> = entries.keys().filter(|k| under(k, &base)).cloned().collect();
        for k in moving {
            let (path, size) = if keep_source { entries[&k].clone() } else { entries.remove(&k).unwrap() };
            let dest: PathBuf = to.components().chain(path.components().skip(depth)).collect();
            entries.insert(norm(&dest), (dest, size));
        }
        if keep_source {
            return Err(MoveError::PartialMove { stored: to.to_path_buf(), detail: "forced partial move".to_string() });
        }
        // Like a real move, the (now possibly empty) source folder stays; drive roots are implicit.
        if let Some(parent) = from.parent().filter(|p| p.parent().is_some()) {
            entries.entry(norm(parent)).or_insert_with(|| (parent.to_path_buf(), None));
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

    fn create_locked_dir(&self, p: &Path) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("create-locked {}", p.display()));
        if !self.exists(p) {
            self.entries.lock().unwrap().insert(norm(p), (p.to_path_buf(), None));
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

    fn is_reparse(&self, p: &Path) -> bool {
        self.reparse.lock().unwrap().contains_key(&norm(p))
    }

    fn remove_all(&self, p: &Path) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("remove {}", p.display()));
        if let Some(locked) = self.locked_under(p) {
            return Err(MoveError::Locked(locked).to_string());
        }
        let base = norm(p);
        self.entries.lock().unwrap().retain(|k, _| !under(k, &base));
        Ok(())
    }

    fn remove_empty_dirs(&self, p: &Path) -> Result<bool, String> {
        self.log.lock().unwrap().push(format!("prune {}", p.display()));
        let base = norm(p);
        let reparse = self.reparse.lock().unwrap();
        let mut entries = self.entries.lock().unwrap();
        // Files and links keep every folder above them.
        let keepers: Vec<String> = entries
            .iter()
            .filter(|(k, (_, size))| under(k, &base) && (size.is_some() || reparse.contains_key(*k)))
            .map(|(k, _)| k.clone())
            .collect();
        entries.retain(|k, _| !under(k, &base) || keepers.iter().any(|f| under(f, k)));
        Ok(keepers.is_empty())
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
        assert_eq!(stored_path_for(root, &id(), Path::new(r"\\server\share\Unity")), None);
        assert_eq!(stored_path_for(root, &id(), Path::new(r"D:\")), None);
    }

    #[test]
    fn stored_path_rejects_parent_components_and_drive_relative_paths() {
        let root = Path::new(r"D:\Stale Uninstaller\quarantine");
        for bad in [r"C:foo", r"C:foo\bar", r"C:\a\..\b", r"C:\a\..", r"C:/a/../../Windows", r"C:\.."] {
            assert_eq!(stored_path_for(root, &id(), Path::new(bad)), None, "{bad}");
        }
        assert!(stored_path_for(root, &id(), Path::new(r"C:\a..b\c")).is_some());
    }

    #[test]
    fn fake_mover_moves_and_reports_locked_file() {
        let m = FakeFileMover::new();
        m.add_dir(r"C:\App");
        m.add_file(r"C:\App\a.dll", 10);
        m.add_file(r"C:\App\sub\b.dll", 5);
        m.add_file(r"C:\Other\c.txt", 3);
        m.add_file(r"C:\Apples.txt", 1);
        m.add_file(r"D:\Q\x\Taken", 2);
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
            Err(MoveError::Locked(PathBuf::from(r"C:\Other\c.txt")))
        );
        assert!(m.exists(Path::new(r"C:\Other\c.txt")));
        assert!(!m.exists(Path::new(r"D:\Q\x\C\Other")));

        assert_eq!(m.move_path(Path::new(r"C:\Apples.txt"), Path::new(r"d:\q\x\taken")), Err(MoveError::Exists));
        assert!(m.exists(Path::new(r"C:\Apples.txt")));
        assert_eq!(m.size_of(Path::new(r"D:\Q\x\Taken")), 2);

        m.remove_all(dest).unwrap();
        assert!(!m.exists(dest));
        assert!(m.volume_ok('D'));
        assert!(!m.volume_ok('e'));
        assert_eq!(
            *m.log.lock().unwrap(),
            vec![
                r"move C:\App -> D:\Q\x\C\App",
                r"move C:\Other -> D:\Q\x\C\Other",
                r"move C:\Apples.txt -> d:\q\x\taken",
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

    use crate::testutil::junction;

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
        assert!(matches!(m.move_path(&src, &dest), Err(MoveError::Other(_))));
    }

    #[test]
    fn real_move_refuses_existing_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let m = RealFileMover;

        let (new, old) = (tmp.path().join("new.txt"), tmp.path().join("old.txt"));
        std::fs::write(&new, b"new").unwrap();
        std::fs::write(&old, b"old").unwrap();
        assert_eq!(m.move_path(&new, &old), Err(MoveError::Exists));
        assert_eq!(std::fs::read(&new).unwrap(), b"new");
        assert_eq!(std::fs::read(&old).unwrap(), b"old");

        let src = tree(tmp.path());
        let dest = tmp.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("keep.txt"), b"keep").unwrap();
        assert_eq!(m.move_path(&src, &dest), Err(MoveError::Exists));
        assert_eq!(move_by_copy(&src, &dest), Err(MoveError::Exists));
        assert_eq!(std::fs::read(dest.join("keep.txt")).unwrap(), b"keep");
        assert!(!dest.join("a.dll").exists());
        assert_eq!(m.size_of(&src), 15);
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
        assert!(matches!(move_by_copy(&file, &tmp.path().join("three.txt")), Err(MoveError::Other(_))));
        assert!(!tmp.path().join("three.txt").exists());
    }

    #[test]
    fn move_by_copy_keeps_copy_when_source_delete_fails() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 1;
        let tmp = tempfile::tempdir().unwrap();
        let src = tree(tmp.path());
        let dest = tmp.path().join("dest");
        // Readable (so it can be copied) but not deletable while this handle is open.
        let handle = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(src.join("sub").join("b.dll")).unwrap();
        let err = move_by_copy(&src, &dest).unwrap_err();
        assert!(matches!(&err, MoveError::PartialMove { stored, .. } if stored == &dest), "{err:?}");
        assert_eq!(std::fs::read(dest.join("a.dll")).unwrap(), b"0123456789");
        assert_eq!(std::fs::read(dest.join("sub").join("b.dll")).unwrap(), b"01234");
        assert!(src.join("sub").join("b.dll").exists());
        drop(handle);
    }

    #[test]
    fn move_by_copy_refuses_links_in_tree_before_deleting_anything() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tree(tmp.path());
        let target = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("precious.txt"), b"keep").unwrap();
        let link = src.join("link");
        if !junction(&link, &target) {
            eprintln!("skipped: cannot create a junction here");
            return;
        }
        let dest = tmp.path().join("dest");
        assert_eq!(move_by_copy(&src, &dest), Err(MoveError::LinkInTree(link.clone())));
        assert!(!dest.exists(), "partial copy is cleaned up");
        assert_eq!(RealFileMover.size_of(&src), 15);
        assert_eq!(std::fs::read(target.join("precious.txt")).unwrap(), b"keep");

        assert_eq!(move_by_copy(&link, &dest), Err(MoveError::LinkInTree(link.clone())));
        assert!(!dest.exists());
        assert!(link.exists());
    }

    #[test]
    fn remove_empty_dirs_keeps_files_and_links() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("id");
        std::fs::create_dir_all(root.join("C").join("a").join("b")).unwrap();
        std::fs::create_dir_all(root.join("D").join("x")).unwrap();
        std::fs::write(root.join("D").join("x").join("keep.txt"), b"k").unwrap();
        let m = RealFileMover;
        assert_eq!(m.remove_empty_dirs(&root), Ok(false));
        assert!(!root.join("C").exists(), "empty branch removed");
        assert!(root.join("D").join("x").join("keep.txt").exists());
        std::fs::remove_file(root.join("D").join("x").join("keep.txt")).unwrap();
        assert_eq!(m.remove_empty_dirs(&root), Ok(true));
        assert!(!root.exists());
        assert_eq!(m.remove_empty_dirs(&root), Ok(true), "missing is gone");

        let fake = FakeFileMover::new();
        fake.add_dir(r"C:\Q\id\C\empty\deeper");
        fake.add_file(r"C:\Q\id\D\stray.bin", 1);
        assert_eq!(fake.remove_empty_dirs(Path::new(r"C:\Q\id")), Ok(false));
        assert!(!fake.exists(Path::new(r"C:\Q\id\C")));
        assert!(fake.exists(Path::new(r"C:\Q\id\D\stray.bin")));
    }

    #[test]
    fn ensure_payload_root_refuses_reparse_points() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        std::fs::create_dir_all(&target).unwrap();
        let link = tmp.path().join("Stale Uninstaller");
        if !junction(&link, &target) {
            eprintln!("skipped: cannot create a junction here");
            return;
        }
        assert!(RealFileMover.ensure_payload_root(&link.join("quarantine")).is_err());
        assert!(RealFileMover.ensure_payload_root(&link).is_err());
        assert!(!target.join("quarantine").exists(), "nothing created through the link");
    }

    #[test]
    #[ignore]
    fn real_volume_ok_for_system_drive() {
        assert!(RealFileMover.volume_ok('c'));
        assert!(!RealFileMover.volume_ok('1'));
    }
}
