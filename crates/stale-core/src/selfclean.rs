//! `StaleUninstaller.exe --uninstall-cleanup`: what Stale's own uninstaller does with the
//! quarantine and `%ProgramData%\Stale Uninstaller` before the program files are removed.

use std::io::ErrorKind;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::quarantine::{list_trusted_manifests, purge, QuarantineManifest, QuarantineOps};

/// The user's answer to the "the quarantine holds N GB" question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Empty the quarantine and remove Stale's data.
    Yes,
    /// Keep the quarantine and the data.
    No,
    /// Cancel the uninstall.
    Cancel,
}

/// What `--uninstall-cleanup` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupAction {
    /// No quarantine: remove the data folder.
    RemoveDataDir,
    /// Purge every quarantine, then remove the data folder.
    PurgeThenRemove,
    /// Leave the quarantine and the data folder as they are.
    KeepAll,
    /// Stop the uninstall (exit code [`EXIT_ABORT`]).
    AbortUninstall,
}

/// The exit code that tells the installer's uninstaller to stop.
pub const EXIT_ABORT: i32 = 2;

/// The action for `manifests` trusted quarantines and the user's `choice` (`None` when nobody was
/// asked, or the question could not be shown: then nothing is deleted).
pub fn cleanup_action(manifests: usize, choice: Option<Choice>) -> CleanupAction {
    match (manifests, choice) {
        (0, _) => CleanupAction::RemoveDataDir,
        (_, Some(Choice::Yes)) => CleanupAction::PurgeThenRemove,
        (_, Some(Choice::Cancel)) => CleanupAction::AbortUninstall,
        (_, Some(Choice::No) | None) => CleanupAction::KeepAll,
    }
}

/// Deletes the tree at `path` without ever following a link: a symlink, junction or other reparse
/// point (`path` itself included) is unlinked, never entered, so nothing outside the tree is
/// touched. A missing `path` is fine.
///
/// An entry that cannot be removed does not stop the rest: its siblings are still removed, the
/// folders above it are left (they are not empty), and every such error is returned.
///
/// Decided on each entry's own attributes (`symlink_metadata`); the data folder this is used on is
/// locked down to Administrators and SYSTEM first, so no unprivileged user can swap an entry for a
/// link between the check and the removal.
pub fn remove_tree_no_follow(path: &Path) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    remove_tree_into(path, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// [`remove_tree_no_follow`], adding each failure to `errors`; true when `path` is gone.
fn remove_tree_into(path: &Path, errors: &mut Vec<String>) -> bool {
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let md = match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => return true,
        Err(e) => {
            errors.push(format!("remove {}: {e}", path.display()));
            return false;
        }
        Ok(md) => md,
    };
    let attrs = md.file_attributes();
    let is_dir = attrs & FILE_ATTRIBUTE_DIRECTORY != 0;
    let result = if md.file_type().is_symlink() || attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        // RemoveDirectory/DeleteFile on a link delete the link itself, never its target.
        if is_dir { std::fs::remove_dir(path) } else { std::fs::remove_file(path) }
    } else if is_dir {
        let entries = match std::fs::read_dir(path) {
            Ok(rd) => rd,
            Err(e) => {
                errors.push(format!("list {}: {e}", path.display()));
                return false;
            }
        };
        let mut all_gone = true;
        for entry in entries {
            match entry {
                Ok(entry) => all_gone &= remove_tree_into(&entry.path(), errors),
                Err(e) => {
                    errors.push(format!("list {}: {e}", path.display()));
                    all_gone = false;
                }
            }
        }
        if !all_gone {
            // Not empty: removing it would only add a second error for the same cause.
            return false;
        }
        retry_writable(path, &md, &|p| std::fs::remove_dir(p))
    } else {
        retry_writable(path, &md, &|p| std::fs::remove_file(p))
    };
    match result {
        Ok(()) => true,
        Err(e) => {
            errors.push(format!("remove {}: {e}", path.display()));
            false
        }
    }
}

/// `remove(path)`; on "access denied" for a read-only entry, clears read-only and tries once more.
///
/// The read-only attribute belongs to the file, not to the name: when the entry is a hard link,
/// clearing it also clears it for every other name of that file, possibly outside the tree. That is
/// accepted because the data folder is locked down to Administrators and SYSTEM before this runs,
/// so after the lock-down only admins can create entries (hard links included) in it.
fn retry_writable(path: &Path, md: &std::fs::Metadata, remove: &dyn Fn(&Path) -> std::io::Result<()>) -> std::io::Result<()> {
    match remove(path) {
        Err(e) if e.kind() == ErrorKind::PermissionDenied && md.permissions().readonly() => {
            let mut perms = md.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            std::fs::set_permissions(path, perms)?;
            remove(path)
        }
        r => r,
    }
}

/// `--ask` exit code: no quarantine, remove the data folder (`--apply remove`).
pub const EXIT_REMOVE: i32 = 10;
/// `--ask` exit code: the user chose Yes, purge then remove (`--apply purge`).
pub const EXIT_PURGE: i32 = 11;
/// `--ask` exit code: keep the quarantine and the data folder (nothing to apply).
pub const EXIT_KEEP: i32 = 12;
/// `--apply` exit code: done.
pub const EXIT_APPLIED: i32 = 0;
/// `--apply` exit code: something could not be purged or removed (the installer's log records it).
pub const EXIT_APPLY_ERRORS: i32 = 1;

/// The `--ask` exit code that tells the installer's uninstaller what was decided.
pub fn exit_code_for(action: CleanupAction) -> i32 {
    match action {
        CleanupAction::RemoveDataDir => EXIT_REMOVE,
        CleanupAction::PurgeThenRemove => EXIT_PURGE,
        CleanupAction::KeepAll => EXIT_KEEP,
        CleanupAction::AbortUninstall => EXIT_ABORT,
    }
}

/// The action an `--ask` exit code stands for; `None` for any other code (a crash, a refused
/// build), which the uninstaller treats as "keep everything".
pub fn action_for_exit_code(code: i32) -> Option<CleanupAction> {
    match code {
        EXIT_REMOVE => Some(CleanupAction::RemoveDataDir),
        EXIT_PURGE => Some(CleanupAction::PurgeThenRemove),
        EXIT_KEEP => Some(CleanupAction::KeepAll),
        EXIT_ABORT => Some(CleanupAction::AbortUninstall),
        _ => None,
    }
}

/// What `--apply` was told to do, decided earlier by `--ask`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyMode {
    /// `--apply remove`: there was no quarantine.
    Remove,
    /// `--apply purge`: the user agreed to empty the quarantine.
    Purge,
}

impl ApplyMode {
    /// `"remove"` or `"purge"`; `None` for anything else.
    pub fn parse(arg: &str) -> Option<ApplyMode> {
        match arg {
            "remove" => Some(ApplyMode::Remove),
            "purge" => Some(ApplyMode::Purge),
            _ => None,
        }
    }
}

/// The action `--apply mode` carries out when `manifests` quarantines (trusted or not) exist now. A
/// quarantine that appeared after `--ask` found none is never purged without consent: `remove`
/// then keeps everything.
pub fn apply_action(mode: ApplyMode, manifests: usize) -> CleanupAction {
    match (mode, manifests) {
        (_, 0) => CleanupAction::RemoveDataDir,
        (ApplyMode::Remove, _) => CleanupAction::KeepAll,
        (ApplyMode::Purge, _) => CleanupAction::PurgeThenRemove,
    }
}

/// How `--apply` ended.
#[derive(Debug, Default, PartialEq)]
pub struct CleanupOutcome {
    /// The data folder was removed.
    pub removed: bool,
    /// Some quarantine could not be purged, so the data folder was kept.
    pub purge_failed: bool,
    /// Quarantines that could not be purged (the data folder is then kept, so their manifests
    /// still describe the payload left on disk) and removal errors.
    pub errors: Vec<String>,
    /// Quarantine folders that are not trusted ([`untrusted_manifest_dirs`]): never deleted, and
    /// while any exist the data folder is kept.
    pub untrusted: usize,
}

impl CleanupOutcome {
    /// The `--apply` exit code: [`EXIT_APPLY_ERRORS`] when anything failed, else [`EXIT_APPLIED`].
    pub fn exit_code(&self) -> i32 {
        if self.errors.is_empty() { EXIT_APPLIED } else { EXIT_APPLY_ERRORS }
    }
}

/// The trusted quarantines below `ops.manifest_dir`: folders that are not reparse points and that
/// `ops.trusted_dir` vouches for.
fn trusted_manifests(ops: &QuarantineOps) -> Vec<QuarantineManifest> {
    list_trusted_manifests(&ops.manifest_dir, &*ops.trusted_dir, &|p| ops.mover.is_reparse(p))
}

/// The folders below `ops.manifest_dir` that `ops.trusted_dir` does not vouch for (it refuses them
/// or cannot judge them), reparse points not counted: a quarantine Stale cannot read safely. They
/// count as a quarantine that is present, and are never deleted.
pub fn untrusted_manifest_dirs(ops: &QuarantineOps) -> usize {
    let Ok(rd) = std::fs::read_dir(&ops.manifest_dir) else { return 0 };
    rd.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .filter(|p| !ops.mover.is_reparse(p) && (ops.trusted_dir)(p) != Some(true))
        .count()
}

/// `--ask`: lists the quarantines and asks `ask(total bytes of the trusted ones)` when there are
/// any, trusted or not ([`untrusted_manifest_dirs`]). Only decides: nothing is purged, moved or
/// deleted, because the installer's uninstaller asks for its own confirmation after this, and
/// `--apply` acts only once that is given.
pub fn decide(ops: &QuarantineOps, ask: &mut dyn FnMut(u64) -> Option<Choice>) -> CleanupAction {
    let manifests = trusted_manifests(ops);
    let count = manifests.len() + untrusted_manifest_dirs(ops);
    let choice = if count == 0 { None } else { ask(manifests.iter().map(|m| m.total_size()).sum()) };
    cleanup_action(count, choice)
}

/// `--apply mode`: re-lists the quarantines and carries out [`apply_action`]: purges the trusted
/// ones ([`purge`], which never deletes unknown payload) and/or removes `data_dir` with
/// [`remove_tree_no_follow`], then prunes the empty folders of `payload_parents` and of
/// `<drive>:\Stale Uninstaller` on every drive a purged manifest stored payload on (removable
/// drives too), which never removes a file or follows a link. The data folder is kept when any
/// purge fails or any untrusted quarantine folder remains ([`untrusted_manifest_dirs`]).
pub fn apply(ops: &QuarantineOps, data_dir: &Path, payload_parents: &[PathBuf], mode: ApplyMode) -> CleanupOutcome {
    let manifests = trusted_manifests(ops);
    let untrusted = untrusted_manifest_dirs(ops);
    let action = apply_action(mode, manifests.len() + untrusted);
    let mut out = CleanupOutcome { untrusted, ..CleanupOutcome::default() };
    if !matches!(action, CleanupAction::RemoveDataDir | CleanupAction::PurgeThenRemove) {
        return out;
    }
    let mut parents: Vec<PathBuf> = payload_parents.to_vec();
    for d in manifests.iter().flat_map(|m| m.moved.iter()).filter_map(|i| crate::pathutil::drive_letter(&i.stored)) {
        if let Some(parent) = (ops.payload_root_for)(d).parent() {
            if !parents.iter().any(|p| p.as_os_str().eq_ignore_ascii_case(parent.as_os_str())) {
                parents.push(parent.to_path_buf());
            }
        }
    }
    if action == CleanupAction::PurgeThenRemove {
        for m in &manifests {
            if let Err(e) = purge(ops, m) {
                out.errors.push(format!("quarantine {}: {e}", m.id.0));
            }
        }
    }
    out.purge_failed = !out.errors.is_empty();
    if !out.purge_failed && untrusted == 0 {
        match remove_tree_no_follow(data_dir) {
            Ok(()) => out.removed = true,
            Err(mut e) => out.errors.append(&mut e),
        }
    }
    for p in &parents {
        // Only empty folders go; anything left (unknown payload, a link) keeps its folders.
        let _ = ops.mover.remove_empty_dirs(p);
    }
    out
}

/// `<drive>:\Stale Uninstaller` on every fixed drive: where quarantine payload roots live. [`apply`]
/// adds the drives its manifests name (a removable drive, say).
pub fn live_payload_parents() -> Vec<PathBuf> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    const DRIVE_FIXED: u32 = 3;
    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| char::from(b'A' + i))
        .filter(|d| {
            let root: Vec<u16> = format!(r"{d}:\").encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: `root` is NUL-terminated and outlives the call.
            unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) == DRIVE_FIXED }
        })
        .filter_map(|d| crate::paths::quarantine_payload_root(d).parent().map(Path::to_path_buf))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_manifests_removes_data_dir() {
        assert_eq!(cleanup_action(0, None), CleanupAction::RemoveDataDir);
    }

    #[test]
    fn yes_purges_then_removes() {
        assert_eq!(cleanup_action(3, Some(Choice::Yes)), CleanupAction::PurgeThenRemove);
    }

    #[test]
    fn no_keeps_all() {
        assert_eq!(cleanup_action(1, Some(Choice::No)), CleanupAction::KeepAll);
    }

    #[test]
    fn cancel_aborts() {
        assert_eq!(cleanup_action(2, Some(Choice::Cancel)), CleanupAction::AbortUninstall);
    }

    #[test]
    fn unanswered_question_keeps_all() {
        assert_eq!(cleanup_action(2, None), CleanupAction::KeepAll);
    }

    /// `mklink /J link target`; false when it could not be created.
    fn junction(link: &Path, target: &Path) -> bool {
        std::process::Command::new(crate::proc::system32_tool("cmd.exe"))
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn removal_unlinks_a_junction_and_keeps_its_target() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("keep.txt"), "x").unwrap();
        std::fs::create_dir(outside.path().join("sub")).unwrap();
        std::fs::write(outside.path().join("sub").join("keep2.txt"), "y").unwrap();

        let base = tempfile::tempdir().unwrap();
        let data = base.path().join("Stale Uninstaller");
        std::fs::create_dir_all(data.join("quarantine").join("20261018-000000-abcd")).unwrap();
        std::fs::write(data.join("settings.json"), "{}").unwrap();
        std::fs::write(data.join("quarantine").join("20261018-000000-abcd").join("manifest.json"), "{}").unwrap();
        let ro = data.join("readonly.txt");
        std::fs::write(&ro, "r").unwrap();
        let mut perms = std::fs::metadata(&ro).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&ro, perms).unwrap();
        assert!(junction(&data.join("quarantine").join("link"), outside.path()), "mklink /J failed");

        remove_tree_no_follow(&data).unwrap();

        assert!(std::fs::symlink_metadata(&data).is_err(), "data folder still there");
        assert!(outside.path().join("keep.txt").is_file());
        assert!(outside.path().join("sub").join("keep2.txt").is_file());
    }

    #[test]
    fn removal_of_a_junction_root_unlinks_only_the_link() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("keep.txt"), "x").unwrap();
        let base = tempfile::tempdir().unwrap();
        let link = base.path().join("link");
        assert!(junction(&link, outside.path()), "mklink /J failed");

        remove_tree_no_follow(&link).unwrap();

        assert!(std::fs::symlink_metadata(&link).is_err());
        assert!(outside.path().join("keep.txt").is_file());
    }

    use crate::quarantine::fsmove::FakeFileMover;
    use crate::quarantine::store::save_manifest;
    use crate::quarantine::{MovedItem, QuarantineId, QuarantineManifest};
    use std::sync::Arc;

    fn manifest(id: &str, size: u64) -> QuarantineManifest {
        QuarantineManifest {
            id: QuarantineId(id.to_string()),
            program_name: id.to_string(),
            created: chrono::Utc::now(),
            moved: if size == 0 {
                vec![]
            } else {
                vec![MovedItem { original: PathBuf::from(r"C:\x"), stored: PathBuf::from(r"C:\s"), is_dir: false, size }]
            },
            keys: vec![],
            values: vec![],
            services: vec![],
            tasks: vec![],
            failures: vec![],
            notes: vec![],
            pending: vec![],
        }
    }

    /// A data folder in a tempdir with `<data>\quarantine` as the manifest folder; folders named
    /// `untrusted-*` are not vouched for.
    fn env() -> (tempfile::TempDir, PathBuf, QuarantineOps) {
        let base = tempfile::tempdir().unwrap();
        let data = base.path().join("Stale Uninstaller");
        let manifest_dir = data.join("quarantine");
        std::fs::create_dir_all(&manifest_dir).unwrap();
        std::fs::write(data.join("settings.json"), "{}").unwrap();
        let ops = QuarantineOps {
            mover: Arc::new(FakeFileMover::new()),
            regw: Arc::new(crate::regwrite::FakeRegistryWriter::default()),
            reg: Arc::new(crate::registry::FakeRegistry::new()),
            fs: Arc::new(crate::fsys::FakeFs::new()),
            manifest_dir,
            payload_root_for: Box::new(crate::paths::quarantine_payload_root),
            tasks_root: PathBuf::from(r"C:\Windows\System32\Tasks"),
            stop_service: Box::new(|_| Ok(())),
            delete_task: Box::new(|_| Ok(())),
            create_task: Box::new(|_, _| Ok(())),
            trusted_dir: Box::new(|p| Some(!p.file_name().unwrap().to_string_lossy().starts_with("untrusted"))),
            reinherit: Box::new(|_| Ok(())),
        };
        (base, data, ops)
    }

    #[test]
    fn exit_codes_map_to_actions() {
        let all = [
            CleanupAction::RemoveDataDir,
            CleanupAction::PurgeThenRemove,
            CleanupAction::KeepAll,
            CleanupAction::AbortUninstall,
        ];
        assert_eq!(all.map(exit_code_for), [10, 11, 12, 2]);
        for a in all {
            assert_eq!(action_for_exit_code(exit_code_for(a)), Some(a));
        }
        assert_eq!(action_for_exit_code(0), None);
        assert_eq!(action_for_exit_code(1), None);
        assert_eq!(ApplyMode::parse("remove"), Some(ApplyMode::Remove));
        assert_eq!(ApplyMode::parse("purge"), Some(ApplyMode::Purge));
        assert_eq!(ApplyMode::parse("Purge"), None);
    }

    #[test]
    fn apply_remove_keeps_all_when_manifests_appeared() {
        assert_eq!(apply_action(ApplyMode::Remove, 0), CleanupAction::RemoveDataDir);
        assert_eq!(apply_action(ApplyMode::Remove, 1), CleanupAction::KeepAll);
        assert_eq!(apply_action(ApplyMode::Purge, 2), CleanupAction::PurgeThenRemove);
        assert_eq!(apply_action(ApplyMode::Purge, 0), CleanupAction::RemoveDataDir);

        let (_base, data, ops) = env();
        save_manifest(&ops.manifest_dir, &manifest("a-1", 10)).unwrap();
        let out = apply(&ops, &data, &[], ApplyMode::Remove);
        assert_eq!(out, CleanupOutcome::default());
        assert!(ops.manifest_dir.join("a-1").join("manifest.json").is_file());
        assert!(data.join("settings.json").is_file());
    }

    #[test]
    fn ask_never_touches_disk() {
        for (answer, expected) in [
            (Some(Choice::Yes), CleanupAction::PurgeThenRemove),
            (Some(Choice::No), CleanupAction::KeepAll),
            (Some(Choice::Cancel), CleanupAction::AbortUninstall),
            (None, CleanupAction::KeepAll),
        ] {
            let fake = Arc::new(FakeFileMover::new());
            let (_base, data, mut ops) = env();
            ops.mover = fake.clone();
            save_manifest(&ops.manifest_dir, &manifest("a-1", 1000)).unwrap();
            save_manifest(&ops.manifest_dir, &manifest("b-2", 24)).unwrap();
            save_manifest(&ops.manifest_dir, &manifest("untrusted-3", 7)).unwrap();
            let mut asked = None;
            let got = decide(&ops, &mut |total| {
                asked = Some(total);
                answer
            });
            assert_eq!(got, expected);
            assert_eq!(asked, Some(1024), "untrusted quarantines are not counted");
            assert_eq!(*fake.log.lock().unwrap(), Vec::<String>::new(), "no move, removal or prune");
            for id in ["a-1", "b-2", "untrusted-3"] {
                assert!(ops.manifest_dir.join(id).join("manifest.json").is_file());
            }
            assert!(data.join("settings.json").is_file());
        }
    }

    #[test]
    fn ask_without_quarantines_decides_removal_without_asking() {
        let (_base, data, ops) = env();
        let mut asked = false;
        let got = decide(&ops, &mut |_| {
            asked = true;
            Some(Choice::No)
        });
        assert!(!asked);
        assert_eq!(got, CleanupAction::RemoveDataDir);
        assert!(data.join("settings.json").is_file());
    }

    /// A quarantine folder Stale cannot vouch for still counts as a quarantine: asked about (with
    /// the trusted size, here none), kept when not answered, and `--apply remove` keeps it all.
    #[test]
    fn untrusted_manifest_folder_blocks_remove() {
        let (_base, data, ops) = env();
        save_manifest(&ops.manifest_dir, &manifest("untrusted-1", 5)).unwrap();
        // One the trust check cannot judge counts too.
        let (_base2, _data2, mut unjudged) = env();
        save_manifest(&unjudged.manifest_dir, &manifest("a-1", 5)).unwrap();
        unjudged.trusted_dir = Box::new(|_| None);
        assert_eq!(untrusted_manifest_dirs(&ops), 1);
        assert_eq!(untrusted_manifest_dirs(&unjudged), 1);

        let mut asked = None;
        let got = decide(&ops, &mut |total| {
            asked = Some(total);
            Some(Choice::No)
        });
        assert_eq!(got, CleanupAction::KeepAll);
        assert_eq!(asked, Some(0), "only trusted quarantines are sized");
        assert_eq!(decide(&ops, &mut |_| None), CleanupAction::KeepAll, "silent: kept");
        assert_eq!(decide(&ops, &mut |_| Some(Choice::Cancel)), CleanupAction::AbortUninstall);

        let out = apply(&ops, &data, &[], ApplyMode::Remove);
        assert_eq!(out, CleanupOutcome { untrusted: 1, ..CleanupOutcome::default() });
        assert!(ops.manifest_dir.join("untrusted-1").join("manifest.json").is_file());
        assert!(data.join("settings.json").is_file());
    }

    #[test]
    fn purge_keeps_data_dir_when_untrusted_manifests_remain() {
        let (_base, data, ops) = env();
        save_manifest(&ops.manifest_dir, &manifest("a-1", 0)).unwrap();
        save_manifest(&ops.manifest_dir, &manifest("untrusted-2", 7)).unwrap();
        assert_eq!(decide(&ops, &mut |_| Some(Choice::Yes)), CleanupAction::PurgeThenRemove);
        let out = apply(&ops, &data, &[], ApplyMode::Purge);
        assert_eq!(out, CleanupOutcome { untrusted: 1, ..CleanupOutcome::default() });
        assert_eq!(out.exit_code(), EXIT_APPLIED);
        assert!(!ops.manifest_dir.join("a-1").exists(), "the trusted quarantine is purged");
        assert!(ops.manifest_dir.join("untrusted-2").join("manifest.json").is_file(), "never deleted");
        assert!(data.join("settings.json").is_file(), "the data folder stays");
    }

    #[test]
    fn apply_remove_removes_the_data_folder_without_quarantines() {
        let (_base, data, ops) = env();
        let out = apply(&ops, &data, &[], ApplyMode::Remove);
        assert_eq!(out, CleanupOutcome { removed: true, ..CleanupOutcome::default() });
        assert_eq!(out.exit_code(), EXIT_APPLIED);
        assert!(std::fs::symlink_metadata(&data).is_err());
    }

    /// Payload stored on a drive that is not fixed (a USB disk) still gets its empty
    /// `<drive>:\Stale Uninstaller` pruned, once per drive.
    #[test]
    fn apply_prunes_payload_parents_on_every_manifest_drive() {
        let fake = Arc::new(FakeFileMover::new());
        let (_base, data, mut ops) = env();
        ops.mover = fake.clone();
        let mut m = manifest("a-1", 3);
        m.moved[0].stored = PathBuf::from(r"R:\Stale Uninstaller\quarantine\a-1\R\x");
        let second = MovedItem { stored: PathBuf::from(r"r:\Stale Uninstaller\quarantine\a-1\R\y"), ..m.moved[0].clone() };
        m.moved.push(second);
        save_manifest(&ops.manifest_dir, &m).unwrap();
        let _ = apply(&ops, &data, &[PathBuf::from(r"C:\Stale Uninstaller")], ApplyMode::Purge);
        let prunes: Vec<String> = fake.log.lock().unwrap().iter().filter(|l| l.starts_with("prune ")).cloned().collect();
        assert_eq!(prunes, vec![r"prune C:\Stale Uninstaller".to_string(), r"prune R:\Stale Uninstaller".to_string()]);
    }

    #[test]
    fn apply_exit_code_reports_errors() {
        assert_eq!(CleanupOutcome::default().exit_code(), EXIT_APPLIED);
        let failed = CleanupOutcome { errors: vec!["remove x: denied".into()], ..CleanupOutcome::default() };
        assert_eq!(failed.exit_code(), EXIT_APPLY_ERRORS);
    }

    /// An entry that cannot be removed (held open without delete sharing) is reported, and its
    /// siblings are removed anyway.
    #[test]
    fn removal_continues_after_an_error() {
        use std::os::windows::fs::OpenOptionsExt;
        let base = tempfile::tempdir().unwrap();
        let data = base.path().join("Stale Uninstaller");
        std::fs::create_dir_all(data.join("a")).unwrap();
        std::fs::create_dir_all(data.join("b")).unwrap();
        std::fs::write(data.join("a").join("locked.txt"), "x").unwrap();
        std::fs::write(data.join("b").join("ok.txt"), "y").unwrap();
        std::fs::write(data.join("c.txt"), "z").unwrap();
        let held = std::fs::OpenOptions::new().read(true).share_mode(0x1).open(data.join("a").join("locked.txt")).unwrap();

        let errors = remove_tree_no_follow(&data).unwrap_err();

        assert_eq!(errors.len(), 1, "only the cause, not the folders above it: {errors:?}");
        assert!(errors[0].contains("locked.txt"), "{errors:?}");
        assert!(!data.join("b").exists() && !data.join("c.txt").exists(), "siblings after the error are removed");
        assert!(data.join("a").join("locked.txt").is_file());
        drop(held);
        remove_tree_no_follow(&data).unwrap();
        assert!(std::fs::symlink_metadata(&data).is_err());
    }

    #[test]
    fn apply_purge_purges_then_removes_the_data_folder() {
        let (_base, data, ops) = env();
        save_manifest(&ops.manifest_dir, &manifest("a-1", 0)).unwrap();
        let out = apply(&ops, &data, &[], ApplyMode::Purge);
        assert_eq!(out, CleanupOutcome { removed: true, ..CleanupOutcome::default() });
        assert!(std::fs::symlink_metadata(&data).is_err());
    }

    #[test]
    fn removal_of_a_missing_path_is_fine() {
        let base = tempfile::tempdir().unwrap();
        remove_tree_no_follow(&base.path().join("missing")).unwrap();
    }
}
