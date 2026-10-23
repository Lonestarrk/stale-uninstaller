//! Signed-out accounts' `NTUSER.DAT`, made readable for the usage sources (UserAssist and Run
//! values) without touching the original.
//!
//! Each hive not already loaded under `HKU` is copied, with its `.LOG1`/`.LOG2`, into a private
//! folder below `%ProgramData%\Stale Uninstaller\hives` (admin-only DACL), and only the copy is
//! loaded, read-only, with `RegLoadAppKeyW`. [`UsersOverlay`] shows each copy as `HKU\<SID>`.
//! Dropping [`OfflineHives`] closes every key and deletes the copies; a crash leaves them to
//! [`clean_stale_copies`] at the next start. The program list, leftover scanners, ghosts and
//! quarantine never see the overlay.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::model::{Hive, RegKeyPath};
use crate::pathutil::{norm, normed_under};
use crate::registry::{key, RegError, RegValue, RegistryReader};

/// A profile's registry hive file.
pub const HIVE_FILE: &str = "NTUSER.DAT";
/// The hive's transaction logs, copied along when present.
const HIVE_LOGS: [&str; 2] = ["NTUSER.DAT.LOG1", "NTUSER.DAT.LOG2"];

/// The private folder holding one scan's hive copies; dropping it removes the folder.
pub struct ScratchDir {
    path: PathBuf,
    remove: Box<dyn Fn(&Path) + Send + Sync>,
}

impl ScratchDir {
    pub fn new(path: PathBuf, remove: Box<dyn Fn(&Path) + Send + Sync>) -> ScratchDir {
        ScratchDir { path, remove }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        (self.remove)(&self.path)
    }
}

/// What [`OfflineHives::load`] asks of the system, so tests can substitute it.
pub trait HiveOps {
    /// Creates this scan's private copy folder.
    fn create_scratch(&self) -> Result<ScratchDir, String>;
    /// Copies `<profile>\NTUSER.DAT` and its logs into the new folder `dest` and returns the
    /// copied hive. `Err` when the profile or its hive is missing, a link, unreadable or locked.
    fn copy_hive(&self, profile: &Path, dest: &Path) -> Result<PathBuf, String>;
    /// Loads the copy `hive` read-only. The reader addresses keys by their path inside the hive;
    /// dropping it closes the key.
    fn load(&self, hive: &Path) -> Result<Box<dyn RegistryReader>, String>;
}

/// The loaded copies of the signed-out accounts' hives, by SID.
pub struct OfflineHives {
    hives: Vec<(String, Box<dyn RegistryReader>)>,
    scratch: Option<ScratchDir>,
}

impl OfflineHives {
    /// No hives.
    pub fn empty() -> OfflineHives {
        OfflineHives { hives: Vec::new(), scratch: None }
    }

    /// Copies and loads the hive of every profile whose SID is not in `loaded_sids` (compared
    /// case-insensitively). A profile whose copy or load fails is skipped (debug log only). The
    /// copy folder is created only when there is a profile to copy.
    pub fn load(profiles: &[(String, PathBuf)], loaded_sids: &HashSet<String>, ops: &dyn HiveOps) -> OfflineHives {
        let loaded: HashSet<String> = loaded_sids.iter().map(|s| s.to_uppercase()).collect();
        let mut out = OfflineHives::empty();
        let wanted: Vec<&(String, PathBuf)> = profiles.iter().filter(|(sid, _)| !loaded.contains(&sid.to_uppercase())).collect();
        if wanted.is_empty() {
            return out;
        }
        let scratch = match ops.create_scratch() {
            Ok(s) => s,
            Err(e) => {
                log::warn!("offline hives: could not create the copy folder: {e}");
                return out;
            }
        };
        let base = scratch.path().to_path_buf();
        out.scratch = Some(scratch);
        for (i, (sid, profile)) in wanted.into_iter().enumerate() {
            if out.hives.iter().any(|(s, _)| s.eq_ignore_ascii_case(sid)) {
                continue;
            }
            let loaded = ops
                .copy_hive(profile, &base.join(i.to_string()))
                .and_then(|hive| ops.load(&hive));
            match loaded {
                Ok(reader) => out.hives.push((sid.clone(), reader)),
                Err(e) => log::debug!("offline hives: skipped {sid} ({}): {e}", profile.display()),
            }
        }
        log::info!("offline hives: {} loaded", out.hives.len());
        out
    }

    /// The SIDs of the loaded copies.
    pub fn sids(&self) -> Vec<String> {
        self.hives.iter().map(|(s, _)| s.clone()).collect()
    }

    fn reader(&self, sid: &str) -> Option<&dyn RegistryReader> {
        self.hives.iter().find(|(s, _)| s.eq_ignore_ascii_case(sid)).map(|(_, r)| &**r)
    }
}

impl Drop for OfflineHives {
    /// Every key is closed before the copies are deleted.
    fn drop(&mut self) {
        self.hives.clear();
        self.scratch.take();
    }
}

/// The `(SID, profile folder)` pairs of the registry's ProfileList, without service profiles
/// (below the Windows folder `windows`) and without `.bak` leftovers.
pub fn profile_sids(reg: &dyn RegistryReader, windows: &Path) -> Vec<(String, PathBuf)> {
    let windows = norm(windows);
    let root = key(Hive::LocalMachine, crate::fsys::PROFILE_LIST);
    reg.subkeys(&root)
        .unwrap_or_default()
        .into_iter()
        .filter(|sid| sid.starts_with("S-") && !sid.contains('.'))
        .filter_map(|sid| {
            let values = reg.values(&key(Hive::LocalMachine, &format!(r"{}\{sid}", crate::fsys::PROFILE_LIST))).ok()?;
            let dir = values.into_iter().find_map(|(n, v)| match v {
                RegValue::Str(s) | RegValue::ExpandStr(s) if n.eq_ignore_ascii_case("ProfileImagePath") => {
                    let s = crate::pathutil::expand_env(s.trim());
                    (!s.is_empty()).then(|| PathBuf::from(s))
                }
                _ => None,
            })?;
            let n = norm(&dir);
            (n != windows && !normed_under(&n, &windows)).then_some((sid, dir))
        })
        .collect()
}

/// The SIDs whose hive is loaded under `HKU` (the `_Classes` hives and `.DEFAULT` left out).
pub fn loaded_sids(reg: &dyn RegistryReader) -> HashSet<String> {
    reg.subkeys(&key(Hive::Users, ""))
        .unwrap_or_default()
        .into_iter()
        .filter(|s| !s.eq_ignore_ascii_case(".DEFAULT") && !s.to_lowercase().ends_with("_classes"))
        .collect()
}

/// `live`, with each offline copy shown as `HKU\<SID>`.
pub struct UsersOverlay<'a> {
    pub live: &'a dyn RegistryReader,
    pub offline: &'a OfflineHives,
}

impl UsersOverlay<'_> {
    /// The offline reader and the path inside its hive, for a key below `HKU\<offline SID>`.
    fn route(&self, k: &RegKeyPath) -> Option<(&dyn RegistryReader, RegKeyPath)> {
        if k.hive != Hive::Users {
            return None;
        }
        let path = k.path.trim_matches('\\');
        let (sid, rest) = path.split_once('\\').unwrap_or((path, ""));
        Some((self.offline.reader(sid)?, key(Hive::Users, rest)))
    }
}

impl RegistryReader for UsersOverlay<'_> {
    fn subkeys(&self, k: &RegKeyPath) -> Result<Vec<String>, RegError> {
        if let Some((reader, inner)) = self.route(k) {
            return reader.subkeys(&inner);
        }
        if k.hive != Hive::Users || !k.path.trim_matches('\\').is_empty() {
            return self.live.subkeys(k);
        }
        let mut sids = match self.live.subkeys(k) {
            Ok(s) => s,
            Err(_) if !self.offline.hives.is_empty() => Vec::new(),
            Err(e) => return Err(e),
        };
        for sid in self.offline.sids() {
            if !sids.iter().any(|s| s.eq_ignore_ascii_case(&sid)) {
                sids.push(sid);
            }
        }
        Ok(sids)
    }

    fn values(&self, k: &RegKeyPath) -> Result<Vec<(String, RegValue)>, RegError> {
        match self.route(k) {
            Some((reader, inner)) => reader.values(&inner),
            None => self.live.values(k),
        }
    }

    fn key_exists(&self, k: &RegKeyPath) -> bool {
        match self.route(k) {
            Some((reader, inner)) => reader.key_exists(&inner),
            None => self.live.key_exists(k),
        }
    }
}

/// Where the usage readers find the current scan's [`OfflineHives`]; empty outside usage
/// gathering.
#[derive(Clone, Default)]
pub struct OfflineSlot(Arc<Mutex<Option<OfflineHives>>>);

impl OfflineSlot {
    /// Puts `hives` in the slot until the returned guard is dropped.
    pub fn fill(&self, hives: OfflineHives) -> SlotGuard {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(hives);
        SlotGuard(self.clone())
    }

    /// A reader that is `live` plus whatever the slot holds at each call ([`UsersOverlay`]).
    pub fn reader(&self, live: Arc<dyn RegistryReader>) -> SlotReader {
        SlotReader { live, slot: self.clone() }
    }

    /// True while hives are in the slot.
    pub fn is_filled(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }
}

/// Empties its [`OfflineSlot`] on drop, which closes the keys and deletes the copies.
pub struct SlotGuard(OfflineSlot);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        let hives = self.0 .0.lock().unwrap_or_else(|e| e.into_inner()).take();
        drop(hives);
    }
}

/// See [`OfflineSlot::reader`].
pub struct SlotReader {
    live: Arc<dyn RegistryReader>,
    slot: OfflineSlot,
}

impl SlotReader {
    fn with<T>(&self, f: impl FnOnce(&dyn RegistryReader) -> T) -> T {
        let held = self.slot.0.lock().unwrap_or_else(|e| e.into_inner());
        match held.as_ref() {
            Some(offline) => f(&UsersOverlay { live: &*self.live, offline }),
            None => f(&*self.live),
        }
    }
}

impl RegistryReader for SlotReader {
    fn subkeys(&self, k: &RegKeyPath) -> Result<Vec<String>, RegError> {
        self.with(|r| r.subkeys(k))
    }
    fn values(&self, k: &RegKeyPath) -> Result<Vec<(String, RegValue)>, RegError> {
        self.with(|r| r.values(k))
    }
    fn key_exists(&self, k: &RegKeyPath) -> bool {
        self.with(|r| r.key_exists(k))
    }
}

const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// The real system: copies below `root` (`%ProgramData%\Stale Uninstaller\hives` for
/// [`LiveHiveOps::new`]), loads them with `RegLoadAppKeyW`.
pub struct LiveHiveOps {
    root: PathBuf,
}

impl LiveHiveOps {
    pub fn new() -> LiveHiveOps {
        LiveHiveOps { root: crate::paths::hives_dir() }
    }

    /// Copies below `root` instead (the live test).
    pub fn with_root(root: PathBuf) -> LiveHiveOps {
        LiveHiveOps { root }
    }
}

impl Default for LiveHiveOps {
    fn default() -> Self {
        LiveHiveOps::new()
    }
}

fn is_link(md: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Copies the file `from` (opened with backup semantics, never through a reparse point, sharing
/// read and write so the owner is never locked out) to the new file `to`.
fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::windows::fs::OpenOptionsExt;
    let mut src = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(from)
        .map_err(|e| format!("open {}: {e}", from.display()))?;
    let md = src.metadata().map_err(|e| format!("read {}: {e}", from.display()))?;
    if is_link(&md) || !md.is_file() {
        return Err(format!("{} is not a plain file", from.display()));
    }
    let mut dst = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|e| format!("create {}: {e}", to.display()))?;
    std::io::copy(&mut src, &mut dst).map_err(|e| format!("copy {}: {e}", from.display()))?;
    Ok(())
}

/// Removes `dir` without following links, retrying while the registry still holds a hive file
/// open (an app hive unloads when its last key closes).
fn remove_copies(dir: &Path) {
    for attempt in 0..10 {
        match crate::selfclean::remove_tree_no_follow(dir) {
            Ok(()) => return,
            Err(errors) if attempt == 9 => errors.iter().for_each(|e| log::warn!("offline hives: {e}")),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(200)),
        }
    }
}

impl HiveOps for LiveHiveOps {
    fn create_scratch(&self) -> Result<ScratchDir, String> {
        match std::fs::symlink_metadata(&self.root) {
            Ok(md) if is_link(&md) || !md.is_dir() => {
                return Err(format!("{} is a link or not a folder", self.root.display()))
            }
            Ok(_) => {}
            Err(_) => crate::acl::create_locked(&self.root)?,
        }
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
        let dir = self.root.join(format!("{:x}-{nanos:x}", std::process::id()));
        crate::acl::create_locked(&dir)?;
        Ok(ScratchDir::new(dir, Box::new(remove_copies)))
    }

    fn copy_hive(&self, profile: &Path, dest: &Path) -> Result<PathBuf, String> {
        let md = std::fs::symlink_metadata(profile).map_err(|e| format!("{}: {e}", profile.display()))?;
        if is_link(&md) || !md.is_dir() {
            return Err(format!("{} is a link or not a folder", profile.display()));
        }
        std::fs::create_dir(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
        copy_file(&profile.join(HIVE_FILE), &dest.join(HIVE_FILE))?;
        for log in HIVE_LOGS {
            let from = profile.join(log);
            if std::fs::symlink_metadata(&from).is_ok() {
                copy_file(&from, &dest.join(log))?;
            }
        }
        Ok(dest.join(HIVE_FILE))
    }

    fn load(&self, hive: &Path) -> Result<Box<dyn RegistryReader>, String> {
        Ok(Box::new(crate::registry::HiveFileReader::load(hive)?))
    }
}

/// Removes `%ProgramData%\Stale Uninstaller\hives` (copies a crash left behind), never following
/// links. Called once at start-up, after the data folder is locked down.
pub fn clean_stale_copies() {
    let root = crate::paths::hives_dir();
    if std::fs::symlink_metadata(&root).is_err() {
        return;
    }
    if let Err(errors) = crate::selfclean::remove_tree_no_follow(&root) {
        errors.iter().for_each(|e| log::warn!("stale hive copies: {e}"));
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use super::{HiveOps, ScratchDir, HIVE_FILE};
    use crate::model::RegKeyPath;
    use crate::registry::{FakeRegistry, RegError, RegValue, RegistryReader};

    pub(crate) const SCRATCH: &str = r"X:\scratch";

    /// Profiles as in-memory hives; every call is recorded in `log`.
    #[derive(Default)]
    pub(crate) struct FakeHiveOps {
        /// By profile folder; moved to `copies` when copied.
        pub hives: Mutex<HashMap<PathBuf, FakeRegistry>>,
        copies: Mutex<HashMap<PathBuf, FakeRegistry>>,
        pub locked: HashSet<PathBuf>,
        pub log: Arc<Mutex<Vec<String>>>,
    }

    impl FakeHiveOps {
        pub(crate) fn with(mut self, profile: &str, hive: FakeRegistry) -> Self {
            self.hives.get_mut().unwrap().insert(PathBuf::from(profile), hive);
            self
        }

        pub(crate) fn calls(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }

        fn push(&self, s: String) {
            self.log.lock().unwrap().push(s);
        }
    }

    struct Recorded {
        inner: FakeRegistry,
        name: String,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Drop for Recorded {
        fn drop(&mut self) {
            self.log.lock().unwrap().push(format!("close {}", self.name));
        }
    }

    impl RegistryReader for Recorded {
        fn subkeys(&self, key: &RegKeyPath) -> Result<Vec<String>, RegError> {
            self.inner.subkeys(key)
        }
        fn values(&self, key: &RegKeyPath) -> Result<Vec<(String, RegValue)>, RegError> {
            self.inner.values(key)
        }
        fn key_exists(&self, key: &RegKeyPath) -> bool {
            self.inner.key_exists(key)
        }
    }

    impl HiveOps for FakeHiveOps {
        fn create_scratch(&self) -> Result<ScratchDir, String> {
            self.push(format!("create {SCRATCH}"));
            let log = self.log.clone();
            Ok(ScratchDir::new(
                PathBuf::from(SCRATCH),
                Box::new(move |p: &Path| log.lock().unwrap().push(format!("remove {}", p.display()))),
            ))
        }

        fn copy_hive(&self, profile: &Path, dest: &Path) -> Result<PathBuf, String> {
            if self.locked.contains(profile) {
                return Err("sharing violation".into());
            }
            let hive = self.hives.lock().unwrap().remove(profile).ok_or("missing")?;
            self.push(format!("copy {} -> {}", profile.display(), dest.display()));
            let copy = dest.join(HIVE_FILE);
            self.copies.lock().unwrap().insert(copy.clone(), hive);
            Ok(copy)
        }

        fn load(&self, hive: &Path) -> Result<Box<dyn RegistryReader>, String> {
            let inner = self.copies.lock().unwrap().remove(hive).ok_or("not copied")?;
            self.push(format!("load {}", hive.display()));
            Ok(Box::new(Recorded { inner, name: hive.display().to_string(), log: self.log.clone() }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{FakeHiveOps, SCRATCH};
    use super::*;
    use crate::registry::FakeRegistry;

    const A: &str = "S-1-5-21-9-1001";
    const B: &str = "S-1-5-21-9-1002";

    fn hive_with(path: &str, name: &str) -> FakeRegistry {
        let mut r = FakeRegistry::new();
        r.set(key(Hive::Users, path), name, RegValue::Dword(1));
        r
    }

    fn live_with_sid(sid: &str) -> FakeRegistry {
        let mut live = FakeRegistry::new();
        live.set(key(Hive::Users, &format!(r"{sid}\Software\Live")), "x", RegValue::Dword(1));
        live.add_key(key(Hive::Users, &format!("{sid}_Classes")));
        live
    }

    #[test]
    fn overlay_lists_live_and_offline_sids() {
        let live = live_with_sid(A);
        let ops = FakeHiveOps::default().with(r"C:\Users\b", hive_with(r"Software\Off", "y"));
        let hives = OfflineHives::load(&[(B.into(), r"C:\Users\b".into())], &HashSet::new(), &ops);
        assert_eq!(hives.sids(), vec![B.to_string()]);
        let overlay = UsersOverlay { live: &live, offline: &hives };
        let mut sids = overlay.subkeys(&key(Hive::Users, "")).unwrap();
        sids.sort();
        assert_eq!(sids, vec![A.to_string(), format!("{A}_Classes"), B.to_string()]);
        // Offline keys come from the copy, live ones from the live registry.
        assert_eq!(overlay.subkeys(&key(Hive::Users, &format!(r"{B}\Software"))).unwrap(), vec!["Off".to_string()]);
        assert!(overlay.key_exists(&key(Hive::Users, &format!(r"{}\software\off", B.to_lowercase()))));
        assert_eq!(overlay.values(&key(Hive::Users, &format!(r"{B}\Software\Off"))).unwrap().len(), 1);
        assert_eq!(overlay.subkeys(&key(Hive::Users, &format!(r"{A}\Software"))).unwrap(), vec!["Live".to_string()]);
        assert!(!overlay.key_exists(&key(Hive::Users, &format!(r"{A}\Software\Off"))));
        assert!(overlay.subkeys(&key(Hive::LocalMachine, "")).unwrap().is_empty());
    }

    #[test]
    fn loaded_sid_is_not_copied() {
        let live = live_with_sid(A);
        let ops = FakeHiveOps::default()
            .with(r"C:\Users\a", hive_with("Software", "x"))
            .with(r"C:\Users\b", hive_with("Software", "x"));
        let profiles = [(A.to_lowercase(), PathBuf::from(r"C:\Users\a")), (B.into(), PathBuf::from(r"C:\Users\b"))];
        let loaded = loaded_sids(&live);
        assert_eq!(loaded, HashSet::from([A.to_string()]), "_Classes and .DEFAULT are not SIDs");
        let hives = OfflineHives::load(&profiles, &loaded, &ops);
        assert_eq!(hives.sids(), vec![B.to_string()]);
        assert!(ops.calls().iter().all(|c| !c.contains(r"C:\Users\a")), "{:?}", ops.calls());

        // Nothing to copy: no copy folder at all.
        let ops = FakeHiveOps::default();
        let hives = OfflineHives::load(&profiles[..1], &loaded, &ops);
        assert!(hives.sids().is_empty());
        drop(hives);
        assert!(ops.calls().is_empty(), "{:?}", ops.calls());
    }

    #[test]
    fn service_and_missing_profiles_are_skipped() {
        let mut reg = FakeRegistry::new();
        let list = crate::fsys::PROFILE_LIST;
        for (sid, dir) in [
            ("S-1-5-18", r"%systemroot%\system32\config\systemprofile"),
            ("S-1-5-19", r"C:\WINDOWS\ServiceProfiles\LocalService"),
            ("S-1-5-80-1", r"C:\Windows\ServiceProfiles\MSSQLSERVER"),
            (A, r"C:\Users\a"),
            (B, r"D:\Profiles\b"),
            ("S-1-5-21-9-1003.bak", r"C:\Users\old"),
        ] {
            reg.set(key(Hive::LocalMachine, &format!(r"{list}\{sid}")), "ProfileImagePath", RegValue::ExpandStr(dir.into()));
        }
        reg.add_key(key(Hive::LocalMachine, &format!(r"{list}\S-1-5-21-9-1004")));
        let _env = crate::env_lock();
        let profiles = profile_sids(&reg, Path::new(r"C:\Windows"));
        assert_eq!(profiles, vec![(A.to_string(), PathBuf::from(r"C:\Users\a")), (B.to_string(), PathBuf::from(r"D:\Profiles\b"))]);

        // B's folder (or hive) is missing: skipped, A loads.
        let ops = FakeHiveOps::default().with(r"C:\Users\a", hive_with("Software", "x"));
        let hives = OfflineHives::load(&profiles, &HashSet::new(), &ops);
        assert_eq!(hives.sids(), vec![A.to_string()]);
    }

    #[test]
    fn locked_hive_is_skipped_silently() {
        let mut ops = FakeHiveOps::default()
            .with(r"C:\Users\a", hive_with("Software", "x"))
            .with(r"C:\Users\b", hive_with("Software", "x"));
        ops.locked.insert(PathBuf::from(r"C:\Users\a"));
        let profiles = [(A.into(), PathBuf::from(r"C:\Users\a")), (B.into(), PathBuf::from(r"C:\Users\b"))];
        let hives = OfflineHives::load(&profiles, &HashSet::new(), &ops);
        assert_eq!(hives.sids(), vec![B.to_string()]);
        assert!(!ops.calls().iter().any(|c| c.starts_with(r"copy C:\Users\a")));
    }

    #[test]
    fn drop_closes_keys_and_removes_copies() {
        let ops = FakeHiveOps::default()
            .with(r"C:\Users\a", hive_with("Software", "x"))
            .with(r"C:\Users\b", hive_with("Software", "x"));
        let profiles = [(A.into(), PathBuf::from(r"C:\Users\a")), (B.into(), PathBuf::from(r"C:\Users\b"))];
        let slot = OfflineSlot::default();
        let guard = slot.fill(OfflineHives::load(&profiles, &HashSet::new(), &ops));
        assert!(slot.is_filled());
        assert!(!ops.calls().iter().any(|c| c.starts_with("close") || c.starts_with("remove")));
        drop(guard);
        assert!(!slot.is_filled());
        let a = Path::new(SCRATCH).join("0").join(HIVE_FILE).display().to_string();
        let b = Path::new(SCRATCH).join("1").join(HIVE_FILE).display().to_string();
        let calls = ops.calls();
        let tail: Vec<&str> = calls[calls.len() - 3..].iter().map(String::as_str).collect();
        assert_eq!(tail, [format!("close {a}"), format!("close {b}"), format!("remove {SCRATCH}")]);
    }

    #[test]
    fn slot_reader_is_live_when_empty() {
        let slot = OfflineSlot::default();
        let reader = slot.reader(Arc::new(live_with_sid(A)));
        assert_eq!(loaded_sids(&reader), HashSet::from([A.to_string()]));
        let ops = FakeHiveOps::default().with(r"C:\Users\b", hive_with("Software", "x"));
        let guard = slot.fill(OfflineHives::load(&[(B.into(), r"C:\Users\b".into())], &HashSet::new(), &ops));
        assert_eq!(loaded_sids(&reader), HashSet::from([A.to_string(), B.to_string()]));
        drop(guard);
        assert_eq!(loaded_sids(&reader), HashSet::from([A.to_string()]));
    }

    /// Elevated. Copies and loads the hive of a signed-out account when one exists, else the
    /// Default profile's (never loaded), below a temporary folder; checks that dropping the hives
    /// unloads them and deletes the copies. The originals are only read.
    #[test]
    #[ignore]
    fn offline_hive_copy_loads_and_unloads() {
        use crate::registry::WinRegistry;
        let loaded = loaded_sids(&WinRegistry);
        let mut profiles: Vec<(String, PathBuf)> = profile_sids(&WinRegistry, &crate::paths::windows_dir())
            .into_iter()
            .filter(|(sid, _)| !loaded.iter().any(|l| l.eq_ignore_ascii_case(sid)))
            .collect();
        println!("signed-out profiles: {profiles:?}");
        if profiles.is_empty() {
            let default = crate::paths::user_profiles_root().join("Default");
            println!("no signed-out account; using {}", default.display());
            profiles.push(("S-1-5-21-0-0-0-DEFAULT".into(), default));
        }
        let tmp = tempfile::tempdir().unwrap();
        let ops = LiveHiveOps::with_root(tmp.path().join("hives"));
        let hives = OfflineHives::load(&profiles, &loaded, &ops);
        println!("loaded: {:?}", hives.sids());
        assert!(!hives.sids().is_empty(), "no hive could be copied and loaded");
        let overlay = UsersOverlay { live: &WinRegistry, offline: &hives };
        for sid in hives.sids() {
            let subs = overlay.subkeys(&key(Hive::Users, &sid)).unwrap();
            println!("{sid}: {subs:?}");
            assert!(subs.iter().any(|s| s.eq_ignore_ascii_case("Software")));
            let ua = overlay.subkeys(&key(Hive::Users, &format!(r"{sid}\Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist")));
            println!("{sid} UserAssist: {ua:?}");
        }
        let copies: Vec<PathBuf> = std::fs::read_dir(tmp.path().join("hives")).unwrap().flatten().map(|e| e.path()).collect();
        assert_eq!(copies.len(), 1);
        drop(hives);
        assert!(!copies[0].exists(), "copies are deleted once the keys are closed");
    }
}
