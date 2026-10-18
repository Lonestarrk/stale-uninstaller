//! Quarantine operations: moving chosen leftovers aside, restoring them, purging them for good.
//!
//! A quarantine `<id>` keeps its manifest, `.reg` exports and task XML copies in
//! `<manifest dir>\<id>\`, and the moved files in `<drive>:\Stale Uninstaller\quarantine\<id>\`.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

use super::fsmove::{stored_path_for, FileMover, MoveError, RealFileMover};
use super::store::{list_trusted_manifests, new_id, owned_by_admins, save_manifest, valid_id};
use super::{
    ExportedKey, Failure, MovedItem, PendingItem, PendingKind, QuarantineId, QuarantineManifest, RemovedService,
    RemovedTask, RemovedValue,
};
use crate::fsys::{FsReader, RealFs};
use crate::leftovers::{Leftover, LeftoverKind};
use crate::model::{Hive, RegKeyPath};
use crate::pathutil::{drive_letter, norm};
use crate::proc::{run_hidden, system32_tool};
use crate::registry::{key, RegistryReader, WinRegistry};
use crate::regwrite::{RegistryWriter, WinRegistryWriter};

const SERVICES_KEY: &str = r"SYSTEM\CurrentControlSet\Services";

/// `Failure::error` for a payload `<id>` folder that still holds files no manifest entry accounts
/// for after a restore or purge; the folder and the manifest are kept.
pub const UNKNOWN_CONTENT_LEFT: &str = "unknown-content-left";

/// Prefix of the purge error `"drive-missing: X:"`: a drive holding stored items is not connected,
/// so nothing is purged and the manifest is kept.
pub const DRIVE_MISSING: &str = "drive-missing";

/// A system command run on a service or task name.
pub type NameCommand = Box<dyn Fn(&str) -> Result<(), String> + Send + Sync>;
/// Re-creates a task `name` from an XML file.
pub type CreateTaskCommand = Box<dyn Fn(&str, &Path) -> Result<(), String> + Send + Sync>;
/// Whether a folder is owned by a trusted account; `None` when that cannot be told.
pub type TrustCheck = Box<dyn Fn(&Path) -> Option<bool> + Send + Sync>;

/// Everything quarantine needs from the system; `live()` for production, fakes in tests.
pub struct QuarantineOps {
    pub mover: Arc<dyn FileMover>,
    pub regw: Arc<dyn RegistryWriter>,
    pub reg: Arc<dyn RegistryReader>,
    pub fs: Arc<dyn FsReader>,
    pub manifest_dir: PathBuf,
    pub payload_root_for: Box<dyn Fn(char) -> PathBuf + Send + Sync>,
    /// `%SystemRoot%\System32\Tasks`; task XML is only read from below it.
    pub tasks_root: PathBuf,
    /// `sc stop` + `sc delete`.
    pub stop_service: NameCommand,
    /// `schtasks /delete /tn <name> /f`.
    pub delete_task: NameCommand,
    /// `schtasks /create /xml <file> /tn <name> /f`.
    pub create_task: CreateTaskCommand,
    /// Whether a `<manifest dir>\<id>` folder may be acted on: `Some(true)` only when it is owned by
    /// Administrators or SYSTEM (`owned_by_admins`), so a planted manifest is never restored or purged.
    pub trusted_dir: TrustCheck,
}

fn tool(name: &str, args: &[&OsStr]) -> Result<(), String> {
    run_hidden(&system32_tool(name), args)
}

/// Task Scheduler path of a task named relative to the Tasks folder.
fn task_path(name: &str) -> String {
    if name.starts_with('\\') { name.to_string() } else { format!(r"\{name}") }
}

/// The state name on the `STATE` line of `sc query` output (`"STOPPED"`, `"RUNNING"`,
/// `"STOP_PENDING"`, ...); a line with only the number gives `"STOPPED"` for 1. `None` without
/// such a line.
fn parse_sc_state(output: &str) -> Option<String> {
    let line = output.lines().find(|l| l.trim_start().starts_with("STATE"))?;
    let (_, rest) = line.split_once(':')?;
    let mut tokens = rest.split_whitespace();
    let code = tokens.next()?;
    match tokens.next() {
        Some(state) => Some(state.to_string()),
        None => (code == "1").then(|| "STOPPED".to_string()),
    }
}

/// `sc query`'s exit code when the service does not exist.
const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;
const SERVICE_POLL: std::time::Duration = std::time::Duration::from_millis(500);
const SERVICE_STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// `sc stop` (a service that is not running, error 1062, is fine), then `sc query` every 500 ms
/// for up to 30 s until it is STOPPED (or gone), then `sc delete`. A service still running after
/// that is `Err("service-still-running")` and is not deleted.
fn live_stop_service(name: &str) -> Result<(), String> {
    match tool("sc.exe", &["stop".as_ref(), name.as_ref()]) {
        Err(e) if !e.contains("1062") => return Err(e),
        _ => {}
    }
    let sc = system32_tool("sc.exe");
    let started = std::time::Instant::now();
    loop {
        let (code, out) = crate::proc::run_hidden_output(&sc, &["query".as_ref(), name.as_ref()])?;
        if code == ERROR_SERVICE_DOES_NOT_EXIST {
            return Ok(());
        }
        if parse_sc_state(&out).is_some_and(|s| s.contains("STOPPED")) {
            break;
        }
        if started.elapsed() >= SERVICE_STOP_TIMEOUT {
            return Err("service-still-running".to_string());
        }
        std::thread::sleep(SERVICE_POLL);
    }
    tool("sc.exe", &["delete".as_ref(), name.as_ref()])
}

fn live_delete_task(name: &str) -> Result<(), String> {
    let tn = task_path(name);
    tool("schtasks.exe", &["/delete".as_ref(), "/tn".as_ref(), tn.as_ref(), "/f".as_ref()])
}

fn live_create_task(name: &str, xml: &Path) -> Result<(), String> {
    let tn = task_path(name);
    tool("schtasks.exe", &["/create".as_ref(), "/xml".as_ref(), xml.as_os_str(), "/tn".as_ref(), tn.as_ref(), "/f".as_ref()])
}

impl QuarantineOps {
    pub fn live() -> QuarantineOps {
        QuarantineOps {
            mover: Arc::new(RealFileMover),
            regw: Arc::new(WinRegistryWriter),
            reg: Arc::new(WinRegistry),
            fs: Arc::new(RealFs),
            manifest_dir: crate::paths::quarantine_manifest_dir(),
            payload_root_for: Box::new(crate::paths::quarantine_payload_root),
            tasks_root: system32_tool("Tasks"),
            stop_service: Box::new(live_stop_service),
            delete_task: Box::new(live_delete_task),
            create_task: Box::new(live_create_task),
            trusted_dir: Box::new(owned_by_admins),
        }
    }
}

#[derive(Clone, Debug)]
pub enum QuarantineProgress {
    /// About to handle item `done` (0-based) of `total`.
    Item { done: usize, total: usize, what: String },
    Finished(Box<QuarantineManifest>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum RestoreOutcome {
    Restored,
    Partial(Vec<Failure>),
}

/// `HKLM\<path>`.
fn key_text(k: &RegKeyPath) -> String {
    let hive = match k.hive {
        Hive::LocalMachine => "HKLM",
        Hive::CurrentUser => "HKCU",
        Hive::Users => "HKU",
    };
    format!(r"{hive}\{}", k.path.trim_matches('\\'))
}

fn describe(kind: &LeftoverKind) -> String {
    match kind {
        LeftoverKind::Dir { path, .. } | LeftoverKind::File { path, .. } => path.display().to_string(),
        LeftoverKind::Shortcut { lnk, .. } => lnk.display().to_string(),
        LeftoverKind::RegKey { key } => key_text(key),
        LeftoverKind::RegValue { key, name } => format!(r"{}\{name}", key_text(key)),
        LeftoverKind::Service { name, .. } | LeftoverKind::ScheduledTask { name, .. } => name.clone(),
    }
}

/// `p` is `base` or below it (case-insensitive).
fn is_under(p: &Path, base: &Path) -> bool {
    let (p, base) = (norm(p), norm(base));
    p.strip_prefix(&base).is_some_and(|rest| rest.is_empty() || rest.starts_with('\\'))
}

/// The first existing reparse point among `path`'s ancestors from the payload root's parent down.
fn reparse_in_chain(mover: &dyn FileMover, root: &Path, path: &Path) -> Option<PathBuf> {
    let top = root.parent().unwrap_or(root);
    path.ancestors()
        .skip(1)
        .take_while(|a| is_under(a, top))
        .find(|a| mover.exists(a) && mover.is_reparse(a))
        .map(Path::to_path_buf)
}

/// Writes `text` as UTF-16LE with a BOM, the encoding Task Scheduler's XML declares.
fn write_utf16(file: &Path, text: &str) -> Result<(), String> {
    let bytes: Vec<u8> = [0xFF, 0xFE].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect();
    std::fs::write(file, bytes).map_err(|e| format!("write {}: {e}", file.display()))
}

/// Characters a file name cannot hold become `_`.
fn file_safe(name: &str) -> String {
    name.chars().map(|c| if r#"\/:*?"<>|"#.contains(c) || c.is_control() { '_' } else { c }).collect()
}

/// One `quarantine()` call in progress.
struct Run<'a> {
    ops: &'a QuarantineOps,
    m: QuarantineManifest,
    /// `<manifest dir>\<id>`.
    folder: PathBuf,
    done: usize,
    total: usize,
    progress: &'a dyn Fn(QuarantineProgress),
    /// Payload root per target drive, prepared once.
    roots: BTreeMap<char, Result<PathBuf, String>>,
    /// A save failed: nothing more is touched.
    broken: bool,
}

impl Run<'_> {
    fn step(&mut self, what: &str) {
        (self.progress)(QuarantineProgress::Item { done: self.done, total: self.total, what: what.to_string() });
        self.done += 1;
    }

    fn fail(&mut self, what: impl Into<String>, error: impl Into<String>) {
        self.m.failures.push(Failure { what: what.into(), error: error.into() });
    }

    /// Saves the manifest; on the first failure records `manifest-save-failed`, and from then on
    /// returns false without touching anything.
    fn save(&mut self) -> bool {
        if self.broken {
            return false;
        }
        match save_manifest(&self.ops.manifest_dir, &self.m) {
            Ok(()) => true,
            Err(e) => {
                self.broken = true;
                self.fail("manifest", format!("manifest-save-failed: {e}"));
                false
            }
        }
    }

    /// Records `kind` as pending and saves, before its destructive step runs; false (the step
    /// must not run) when that save fails.
    fn begin(&mut self, kind: PendingKind) -> bool {
        if self.broken {
            return false;
        }
        self.m.pending.push(PendingItem { kind, started: Utc::now() });
        if self.save() {
            true
        } else {
            self.m.pending.pop();
            false
        }
    }

    /// The step begun last is over: drops its pending record, lets `record` add the outcome, saves.
    fn end(&mut self, record: impl FnOnce(&mut QuarantineManifest)) {
        self.m.pending.pop();
        record(&mut self.m);
        self.save();
    }

    /// Export the service key, then stop and delete the service.
    fn services(&mut self, items: &[Leftover]) {
        for it in items {
            let LeftoverKind::Service { name, .. } = &it.kind else { continue };
            self.step(name);
            if name.is_empty() || name.contains(['\\', '/']) {
                self.fail(name, "invalid service name");
                continue;
            }
            let file = self.folder.join(format!("service-{}.reg", file_safe(name)));
            let k = service_key(name);
            if let Err(e) = self.ops.regw.export_key(&k, &file) {
                self.fail(name, e);
                continue;
            }
            if !self.begin(PendingKind::ServiceExport { name: name.clone(), reg_file: file.clone() }) {
                continue;
            }
            let stopped = (self.ops.stop_service)(name);
            self.end(|m| match stopped {
                Ok(()) => m.services.push(RemovedService { name: name.clone(), reg_file: file }),
                Err(error) => m.failures.push(Failure { what: name.clone(), error }),
            });
        }
    }

    /// Copy the task XML next to the manifest, then delete the task.
    fn tasks(&mut self, items: &[Leftover]) {
        let mut n = 0;
        for it in items {
            let LeftoverKind::ScheduledTask { name, xml, .. } = &it.kind else { continue };
            self.step(name);
            if !is_under(xml, &self.ops.tasks_root) {
                self.fail(name, "task file is outside the tasks folder");
                continue;
            }
            let Some(text) = self.ops.fs.read_text(xml) else {
                self.fail(name, "task definition could not be read");
                continue;
            };
            let backup = self.folder.join(format!("task-{n}.xml"));
            n += 1;
            if let Err(e) = write_utf16(&backup, &text) {
                self.fail(name, e);
                continue;
            }
            if !self.begin(PendingKind::TaskBackup { name: name.clone(), xml_backup: backup.clone() }) {
                continue;
            }
            let deleted = (self.ops.delete_task)(name);
            self.end(|m| match deleted {
                Ok(()) => m.tasks.push(RemovedTask { name: name.clone(), xml_backup: backup, original_xml: xml.clone() }),
                Err(error) => m.failures.push(Failure { what: name.clone(), error }),
            });
        }
    }

    /// Export each value's key once, then delete the values.
    fn values(&mut self, items: &[Leftover]) {
        let mut groups: Vec<(&RegKeyPath, Vec<&str>)> = Vec::new();
        for it in items {
            let LeftoverKind::RegValue { key, name } = &it.kind else { continue };
            let same = |k: &&RegKeyPath| k.hive == key.hive && k.path.eq_ignore_ascii_case(&key.path);
            match groups.iter_mut().find(|(k, _)| same(k)) {
                Some((_, names)) => names.push(name),
                None => groups.push((key, vec![name])),
            }
        }
        for (n, (k, names)) in groups.into_iter().enumerate() {
            let file = self.folder.join(format!("value-{n}.reg"));
            let mut exported = None;
            for name in names {
                let what = format!(r"{}\{name}", key_text(k));
                self.step(&what);
                if let Err(e) = exported.get_or_insert_with(|| self.ops.regw.export_key(k, &file)) {
                    let e = e.clone();
                    self.fail(what, e);
                    continue;
                }
                let pending = PendingKind::ValueExport { key: k.clone(), name: name.to_string(), reg_file: file.clone() };
                if !self.begin(pending) {
                    continue;
                }
                let deleted = self.ops.regw.delete_value(k, name);
                self.end(|m| match deleted {
                    Ok(()) => m.values.push(RemovedValue { key: k.clone(), name: name.to_string(), reg_file: file.clone() }),
                    Err(error) => m.failures.push(Failure { what, error }),
                });
            }
        }
    }

    /// Export each key, then delete it.
    fn keys(&mut self, items: &[Leftover]) {
        let mut n = 0;
        for it in items {
            let LeftoverKind::RegKey { key } = &it.kind else { continue };
            let what = key_text(key);
            self.step(&what);
            let file = self.folder.join(format!("key-{n}.reg"));
            n += 1;
            if let Err(e) = self.ops.regw.export_key(key, &file) {
                self.fail(what, e);
                continue;
            }
            if !self.begin(PendingKind::KeyExport { key: key.clone(), reg_file: file.clone() }) {
                continue;
            }
            let deleted = self.ops.regw.delete_key(key);
            self.end(|m| match deleted {
                Ok(()) => m.keys.push(ExportedKey { key: key.clone(), reg_file: file }),
                Err(error) => m.failures.push(Failure { what, error }),
            });
        }
    }

    fn shortcuts(&mut self, items: &[Leftover]) {
        for it in items {
            if let LeftoverKind::Shortcut { lnk, .. } = &it.kind {
                self.move_item(lnk, false);
            }
        }
    }

    fn files(&mut self, items: &[Leftover]) {
        for it in items {
            match &it.kind {
                LeftoverKind::Dir { path, .. } => self.move_item(path, true),
                LeftoverKind::File { path, .. } => self.move_item(path, false),
                _ => {}
            }
        }
    }

    /// The payload root for `drive`; on first use its parent and itself are created and locked
    /// down, and this quarantine's `<id>` folder is created locked below it.
    fn payload_root(&mut self, drive: char) -> Result<PathBuf, String> {
        let ops = self.ops;
        let id = self.m.id.0.clone();
        self.roots
            .entry(drive)
            .or_insert_with(|| {
                let root = (ops.payload_root_for)(drive);
                if let Some(parent) = root.parent() {
                    ops.mover.ensure_payload_root(parent)?;
                }
                ops.mover.ensure_payload_root(&root)?;
                ops.mover.create_locked_dir(&root.join(&id))?;
                Ok(root)
            })
            .clone()
    }

    /// Moves a file or folder into the payload root of its drive (C: when that drive cannot hold one).
    fn move_item(&mut self, original: &Path, is_dir: bool) {
        let what = original.display().to_string();
        self.step(&what);
        let Some(drive) = drive_letter(original) else {
            self.fail(what, "no drive letter");
            return;
        };
        let drive = if self.ops.mover.volume_ok(drive) { drive } else { 'C' };
        let root = match self.payload_root(drive) {
            Ok(root) => root,
            Err(e) => return self.fail(what, e),
        };
        let Some(stored) = stored_path_for(&root, &self.m.id, original) else {
            self.fail(what, "path cannot be stored in quarantine");
            return;
        };
        if let Some(p) = reparse_in_chain(self.ops.mover.as_ref(), &root, &stored) {
            self.fail(what, format!("reparse point in quarantine path: {}", p.display()));
            return;
        }
        let size = self.ops.mover.size_of(original);
        let original = original.to_path_buf();
        if !self.begin(PendingKind::Move { original: original.clone(), stored: stored.clone() }) {
            return;
        }
        let moved = self.ops.mover.move_path(&original, &stored);
        self.end(|m| match moved {
            Ok(()) => m.moved.push(MovedItem { original, stored, is_dir, size }),
            Err(MoveError::PartialMove { stored, detail }) => {
                m.moved.push(MovedItem { original, stored, is_dir, size });
                m.failures.push(Failure { what, error: format!("partial move: {detail}") });
            }
            Err(e) => m.failures.push(Failure { what, error: e.to_string() }),
        });
    }
}

/// For each item, whether it is a file, folder or shortcut strictly inside a listed folder (it then
/// goes along with that folder and is not handled on its own).
fn covered_by_listed_dir(items: &[Leftover]) -> Vec<bool> {
    let dirs: Vec<String> = items
        .iter()
        .filter_map(|i| match &i.kind {
            LeftoverKind::Dir { path, .. } => Some(norm(path)),
            _ => None,
        })
        .collect();
    items
        .iter()
        .map(|i| {
            let p = match &i.kind {
                LeftoverKind::Dir { path, .. } | LeftoverKind::File { path, .. } => path,
                LeftoverKind::Shortcut { lnk, .. } => lnk,
                _ => return false,
            };
            let p = norm(p);
            dirs.iter().any(|d| p.strip_prefix(d.as_str()).is_some_and(|rest| rest.starts_with('\\')))
        })
        .collect()
}

/// Where the files of a removal will go, worked out before it starts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DestinationPlan {
    /// Bytes stored on each target drive's payload root.
    pub per_drive: BTreeMap<char, u64>,
    /// Source drives that cannot hold a payload root (not NTFS, read-only, missing) and the bytes
    /// from them that are copied to C: instead.
    pub fallbacks: Vec<(char, u64)>,
    /// Target drives without enough free space for the bytes copied to them (a same-drive move is
    /// a rename and needs none).
    pub insufficient: Vec<char>,
}

/// Plans where `items` (files, folders and shortcuts; the rest takes no disk space) will be
/// stored: on their own drive when `volume_ok`, else on C:. Items inside a listed folder count
/// with that folder. `free_bytes` is the free space of a drive (`None`: unknown, not checked).
pub fn plan_destinations(
    items: &[Leftover],
    volume_ok: &dyn Fn(char) -> bool,
    free_bytes: &dyn Fn(char) -> Option<u64>,
) -> DestinationPlan {
    let mut plan = DestinationPlan::default();
    let mut copied: BTreeMap<char, u64> = BTreeMap::new();
    let mut fallbacks: BTreeMap<char, u64> = BTreeMap::new();
    for (it, covered) in items.iter().zip(covered_by_listed_dir(items)) {
        let path = match &it.kind {
            LeftoverKind::Dir { path, .. } | LeftoverKind::File { path, .. } => path,
            LeftoverKind::Shortcut { lnk, .. } => lnk,
            _ => continue,
        };
        let Some(source) = drive_letter(path) else { continue };
        if covered {
            continue;
        }
        let bytes = it.size().unwrap_or(0);
        let target = if volume_ok(source) { source } else { 'C' };
        *plan.per_drive.entry(target).or_insert(0) += bytes;
        if target != source {
            *fallbacks.entry(source).or_insert(0) += bytes;
            *copied.entry(target).or_insert(0) += bytes;
        }
    }
    plan.fallbacks = fallbacks.into_iter().collect();
    plan.insufficient =
        copied.into_iter().filter(|&(d, bytes)| free_bytes(d).is_some_and(|free| free < bytes)).map(|(d, _)| d).collect();
    plan
}

/// Free bytes available to this process on `<drive>:\`; `None` when it cannot be read.
pub fn live_free_bytes(drive: char) -> Option<u64> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    if !drive.is_ascii_alphabetic() {
        return None;
    }
    let root: Vec<u16> = format!(r"{}:\", drive.to_ascii_uppercase()).encode_utf16().chain(std::iter::once(0)).collect();
    let mut free = 0u64;
    // SAFETY: `root` is NUL-terminated and outlives the call; `free` is a valid out-pointer.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(root.as_ptr()), Some(&mut free), None, None) }.ok()?;
    Some(free)
}

/// Whether `<drive>:\` can hold a payload root (see [`FileMover::volume_ok`]).
pub fn live_volume_ok(drive: char) -> bool {
    RealFileMover.volume_ok(drive)
}

/// Locks the manifest folder down like a payload root (Administrators and SYSTEM only), so the
/// `.reg`/XML backups a restore imports cannot be planted by an unprivileged user.
fn secure_manifest_dir(ops: &QuarantineOps) -> Result<(), String> {
    ops.mover.ensure_payload_root(&ops.manifest_dir)
}

/// Refuses a `<manifest dir>\<id>` folder that is a reparse point or that `ops.trusted_dir` does
/// not vouch for.
fn check_trusted(ops: &QuarantineOps, id: &QuarantineId) -> Result<(), String> {
    let folder = ops.manifest_dir.join(&id.0);
    if !ops.mover.is_reparse(&folder) && (ops.trusted_dir)(&folder) == Some(true) {
        Ok(())
    } else {
        Err(format!("quarantine folder not owned by Administrators or SYSTEM: {}", folder.display()))
    }
}

/// Quarantines `items` in the order services, tasks, registry values, registry keys, shortcuts,
/// files/folders. Every failure is recorded in the manifest and the rest continues. The manifest
/// is saved before anything is touched, with a pending record before each destructive step and
/// with its outcome after it; a failed save stops everything after it.
/// Files, folders and shortcuts inside a listed folder go along with that folder.
pub fn quarantine(
    ops: &QuarantineOps,
    program_name: &str,
    items: &[Leftover],
    now: DateTime<Utc>,
    progress: &dyn Fn(QuarantineProgress),
) -> QuarantineManifest {
    let mut id = new_id(now);
    while ops.manifest_dir.join(&id.0).exists() {
        id = new_id(now);
    }
    let m = QuarantineManifest {
        id: id.clone(),
        program_name: program_name.to_string(),
        created: now,
        moved: vec![],
        keys: vec![],
        values: vec![],
        services: vec![],
        tasks: vec![],
        failures: vec![],
        notes: vec![],
        pending: vec![],
    };
    let mut run = Run {
        ops,
        m,
        folder: ops.manifest_dir.join(&id.0),
        done: 0,
        total: items.len(),
        progress,
        roots: BTreeMap::new(),
        broken: false,
    };
    let created = secure_manifest_dir(ops).and_then(|()| ops.mover.create_locked_dir(&run.folder));
    if let Err(e) = created.and_then(|()| save_manifest(&ops.manifest_dir, &run.m)) {
        // Without a manifest nothing could be restored, so nothing is touched.
        for it in items {
            run.fail(describe(&it.kind), format!("manifest could not be saved: {e}"));
        }
        progress(QuarantineProgress::Finished(Box::new(run.m.clone())));
        return run.m;
    }
    let covered = covered_by_listed_dir(items);
    let mut work = Vec::new();
    for (it, covered) in items.iter().zip(covered) {
        if covered {
            run.step(&describe(&it.kind));
        } else {
            work.push(it.clone());
        }
    }
    for group in 0..6 {
        match group {
            0 => run.services(&work),
            1 => run.tasks(&work),
            2 => run.values(&work),
            3 => run.keys(&work),
            4 => run.shortcuts(&work),
            _ => run.files(&work),
        }
        if !run.save() {
            break;
        }
    }
    progress(QuarantineProgress::Finished(Box::new(run.m.clone())));
    run.m
}

/// Every existing `<payload root of drive A..Z>\<id>`, whether or not the manifest lists items there.
fn payload_dirs(ops: &QuarantineOps, id: &QuarantineId) -> Vec<PathBuf> {
    ('A'..='Z').map(|d| (ops.payload_root_for)(d).join(&id.0)).filter(|p| ops.mover.exists(p)).collect()
}

/// A backup a restore may use: a non-link file named after `pattern` (`N` = digits, `*` = any
/// non-empty text) directly in `<manifest dir>\<id>`.
fn check_backup(ops: &QuarantineOps, id: &QuarantineId, file: &Path, pattern: &str) -> Result<(), String> {
    let outside = || format!("backup file outside the quarantine folder: {}", file.display());
    let name = file.file_name().and_then(|n| n.to_str()).ok_or_else(outside)?;
    let (prefix, suffix) = pattern.split_once(['N', '*']).ok_or_else(outside)?;
    let middle = name.strip_prefix(prefix).and_then(|r| r.strip_suffix(suffix)).ok_or_else(outside)?;
    if middle.is_empty() || (pattern.contains('N') && !middle.chars().all(|c| c.is_ascii_digit())) {
        return Err(outside());
    }
    let parent = file.parent().and_then(|p| std::fs::canonicalize(p).ok());
    let folder = std::fs::canonicalize(ops.manifest_dir.join(&id.0)).ok();
    if parent.is_none() || parent != folder {
        return Err(outside());
    }
    if ops.mover.is_reparse(file) {
        return Err(format!("backup file is a link: {}", file.display()));
    }
    Ok(())
}

/// Moves files back, last moved first; returns the items that stay in quarantine.
fn restore_moved(ops: &QuarantineOps, id: &QuarantineId, moved: &[MovedItem], failures: &mut Vec<Failure>) -> Vec<MovedItem> {
    let mut kept = Vec::new();
    for item in moved.iter().rev() {
        let what = item.original.display().to_string();
        let mut fail = |error: String| failures.push(Failure { what: what.clone(), error });
        if ops.mover.exists(&item.original) {
            fail(OCCUPIED.to_string());
            kept.push(item.clone());
            continue;
        }
        let root = drive_letter(&item.stored).map(|d| (ops.payload_root_for)(d));
        let Some(root) = root.filter(|r| is_under(&item.stored, &r.join(&id.0))) else {
            fail("stored outside the quarantine folder".to_string());
            kept.push(item.clone());
            continue;
        };
        if let Some(p) = reparse_in_chain(ops.mover.as_ref(), &root, &item.stored) {
            fail(format!("reparse point in quarantine path: {}", p.display()));
            kept.push(item.clone());
            continue;
        }
        if let Some(p) = item.original.ancestors().skip(1).find(|a| ops.mover.is_reparse(a)) {
            fail(format!("reparse-in-destination: {}", p.display()));
            kept.push(item.clone());
            continue;
        }
        match ops.mover.move_path(&item.stored, &item.original) {
            Ok(()) => {}
            // The original is complete again; the leftover copy goes with the payload folder.
            Err(MoveError::PartialMove { detail, .. }) => fail(format!("partial move: {detail}")),
            Err(e) => {
                fail(e.to_string());
                kept.push(item.clone());
            }
        }
    }
    kept.reverse();
    kept
}

/// `Failure::error` for something a restore would overwrite: the original path, key, value or
/// service exists again (e.g. the program was reinstalled). The item stays in quarantine.
const OCCUPIED: &str = "occupied";

/// `Err("occupied")` when `occupied`, so a reinstalled program's registry data is never
/// overwritten by an old backup.
fn not_occupied(occupied: bool) -> Result<(), String> {
    if occupied { Err(OCCUPIED.to_string()) } else { Ok(()) }
}

/// Imports each key backup unless the key exists again.
fn restore_keys(ops: &QuarantineOps, id: &QuarantineId, keys: &[ExportedKey], failures: &mut Vec<Failure>) -> Vec<ExportedKey> {
    let mut kept = Vec::new();
    for k in keys.iter().rev() {
        let imported = not_occupied(ops.reg.key_exists(&k.key))
            .and_then(|()| check_backup(ops, id, &k.reg_file, "key-N.reg"))
            .and_then(|()| ops.regw.import_file(&k.reg_file));
        if let Err(error) = imported {
            failures.push(Failure { what: key_text(&k.key), error });
            kept.push(k.clone());
        }
    }
    kept.reverse();
    kept
}

/// Imports each value export once (it re-creates the whole key as it was), unless one of its
/// removed values exists again.
fn restore_values(
    ops: &QuarantineOps,
    id: &QuarantineId,
    values: &[RemovedValue],
    failures: &mut Vec<Failure>,
) -> Vec<RemovedValue> {
    let mut files: Vec<&Path> = Vec::new();
    for v in values.iter().rev() {
        if !files.contains(&v.reg_file.as_path()) {
            files.push(&v.reg_file);
        }
    }
    let mut failed: Vec<&Path> = Vec::new();
    for file in files {
        let occupied = values.iter().any(|v| v.reg_file == file && value_exists(ops, &v.key, &v.name));
        let imported = not_occupied(occupied)
            .and_then(|()| check_backup(ops, id, file, "value-N.reg"))
            .and_then(|()| ops.regw.import_file(file));
        if let Err(error) = imported {
            for v in values.iter().filter(|v| v.reg_file == file) {
                failures.push(Failure { what: format!(r"{}\{}", key_text(&v.key), v.name), error: error.clone() });
            }
            failed.push(file);
        }
    }
    values.iter().filter(|v| failed.contains(&v.reg_file.as_path())).cloned().collect()
}

fn restore_tasks(ops: &QuarantineOps, id: &QuarantineId, tasks: &[RemovedTask], failures: &mut Vec<Failure>) -> Vec<RemovedTask> {
    let mut kept = Vec::new();
    for t in tasks.iter().rev() {
        let created =
            check_backup(ops, id, &t.xml_backup, "task-N.xml").and_then(|()| (ops.create_task)(&t.name, &t.xml_backup));
        if let Err(error) = created {
            failures.push(Failure { what: t.name.clone(), error });
            kept.push(t.clone());
        }
    }
    kept.reverse();
    kept
}

/// Re-imports service keys (unless the service exists again); a restored service only starts
/// after a reboot (noted in `notes`).
fn restore_services(
    ops: &QuarantineOps,
    id: &QuarantineId,
    services: &[RemovedService],
    failures: &mut Vec<Failure>,
    notes: &mut Vec<String>,
) -> Vec<RemovedService> {
    let mut kept = Vec::new();
    for s in services.iter().rev() {
        let imported = not_occupied(service_exists(ops, &s.name))
            .and_then(|()| check_backup(ops, id, &s.reg_file, "service-*.reg"))
            .and_then(|()| ops.regw.import_file(&s.reg_file));
        match imported {
            Ok(()) => notes.push(format!("service-needs-reboot:{}", s.name)),
            Err(error) => {
                failures.push(Failure { what: s.name.clone(), error });
                kept.push(s.clone());
            }
        }
    }
    kept.reverse();
    kept
}

/// Puts a quarantine back: files/shortcuts, then registry keys/values, tasks, services (pending
/// steps included, see [`adopt_pending`]). Items that cannot go back stay in the (re-saved)
/// manifest; once nothing is left the known payload paths, the payload folders left empty and the
/// manifest folder are deleted (a payload folder holding anything unknown keeps the manifest and
/// is reported as [`UNKNOWN_CONTENT_LEFT`]). A full restore drops the manifest, so callers that want to say
/// "restored services need a reboot" look at `m.services`. A quarantine folder not owned by
/// Administrators or SYSTEM (`ops.trusted_dir`) is refused untouched.
pub fn restore(ops: &QuarantineOps, m: &QuarantineManifest) -> RestoreOutcome {
    if !valid_id(&m.id) {
        return RestoreOutcome::Partial(vec![Failure { what: m.id.0.clone(), error: "invalid quarantine id".to_string() }]);
    }
    if let Err(error) = secure_manifest_dir(ops).and_then(|()| check_trusted(ops, &m.id)) {
        return RestoreOutcome::Partial(vec![Failure { what: "manifest".to_string(), error }]);
    }
    let m = &adopt_pending(ops, m);
    let mut failures = Vec::new();
    let mut left = m.clone();
    left.moved = restore_moved(ops, &m.id, &m.moved, &mut failures);
    left.keys = restore_keys(ops, &m.id, &m.keys, &mut failures);
    left.values = restore_values(ops, &m.id, &m.values, &mut failures);
    left.tasks = restore_tasks(ops, &m.id, &m.tasks, &mut failures);
    left.services = restore_services(ops, &m.id, &m.services, &mut failures, &mut left.notes);

    let nothing_left = left.moved.is_empty()
        && left.keys.is_empty()
        && left.values.is_empty()
        && left.tasks.is_empty()
        && left.services.is_empty();
    if nothing_left {
        // Copies left behind by a partial move back are known; anything else stays.
        let known: Vec<PathBuf> = m.moved.iter().map(|i| i.stored.clone()).collect();
        let left_over = clean_payload(ops, &m.id, &known);
        if left_over.is_empty() {
            if let Err(e) = remove_manifest_folder(ops, &m.id) {
                log::warn!("quarantine {}: {e}", m.id.0);
            }
            return if failures.is_empty() { RestoreOutcome::Restored } else { RestoreOutcome::Partial(failures) };
        }
        add_new(&mut left.failures, &left_over);
        failures.extend(left_over);
    }
    if let Err(error) = save_manifest(&ops.manifest_dir, &left) {
        failures.push(Failure { what: "manifest".to_string(), error });
    }
    if failures.is_empty() { RestoreOutcome::Restored } else { RestoreOutcome::Partial(failures) }
}

/// Deletes `<manifest dir>\<id>` (the manifest and its `.reg`/XML backups); missing is fine.
fn remove_manifest_folder(ops: &QuarantineOps, id: &QuarantineId) -> Result<(), String> {
    let folder = ops.manifest_dir.join(&id.0);
    match std::fs::remove_dir_all(&folder) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("remove {}: {e}", folder.display())),
        _ => Ok(()),
    }
}

/// Deletes the known stored paths (moved items and pending moves), the payload folders left empty,
/// and then the manifest folder for good. If anything cannot be deleted, or a payload folder holds
/// anything unknown ([`UNKNOWN_CONTENT_LEFT`], recorded in the manifest), the manifest is kept, so
/// nothing is orphaned and the purge can be retried. A quarantine folder not owned by
/// Administrators or SYSTEM (`ops.trusted_dir`) is refused untouched, and so is one with stored
/// items on a drive that is not connected (`Err("drive-missing: X:")`).
pub fn purge(ops: &QuarantineOps, m: &QuarantineManifest) -> Result<(), String> {
    secure_manifest_dir(ops)?;
    purge_secured(ops, m)
}

fn purge_secured(ops: &QuarantineOps, m: &QuarantineManifest) -> Result<(), String> {
    if !valid_id(&m.id) {
        return Err(format!("invalid quarantine id {:?}", m.id.0));
    }
    check_trusted(ops, &m.id)?;
    let mut kept = adopt_pending(ops, m);
    // An unplugged drive would make its items look purged: keep everything until it is back.
    let mut drives: Vec<char> = kept.moved.iter().filter_map(|i| drive_letter(&i.stored)).collect();
    drives.dedup();
    if let Some(d) = drives.into_iter().find(|d| !ops.mover.exists(Path::new(&format!(r"{d}:\")))) {
        return Err(format!("{DRIVE_MISSING}: {d}:"));
    }
    let known: Vec<PathBuf> = kept.moved.iter().map(|i| i.stored.clone()).collect();
    let left_over = clean_payload(ops, &m.id, &known);
    if !left_over.is_empty() {
        // What was deleted is no longer listed.
        kept.moved.retain(|i| ops.mover.exists(&i.stored));
        add_new(&mut kept.failures, &left_over);
        if let Err(e) = save_manifest(&ops.manifest_dir, &kept) {
            log::warn!("quarantine {}: {e}", m.id.0);
        }
        return Err(left_over.iter().map(|f| format!("{}: {}", f.error, f.what)).collect::<Vec<_>>().join("; "));
    }
    remove_manifest_folder(ops, &m.id)
}

/// Appends each of `new` that `list` does not hold yet.
fn add_new(list: &mut Vec<Failure>, new: &[Failure]) {
    for f in new {
        if !list.contains(f) {
            list.push(f.clone());
        }
    }
}

/// `m` with its pending steps turned into ordinary entries wherever the step may have happened:
/// a move whose stored path exists is a moved item; a key, value or service export whose backup
/// exists while the key/value/service is gone is an exported item; a task backup that exists is
/// a removed task. Pending steps that cannot have happened are dropped.
fn adopt_pending(ops: &QuarantineOps, m: &QuarantineManifest) -> QuarantineManifest {
    let mut out = m.clone();
    out.pending.clear();
    for p in &m.pending {
        match &p.kind {
            PendingKind::Move { original, stored } if ops.mover.exists(stored) => out.moved.push(MovedItem {
                original: original.clone(),
                stored: stored.clone(),
                is_dir: false,
                size: ops.mover.size_of(stored),
            }),
            PendingKind::KeyExport { key, reg_file } if reg_file.is_file() && !ops.reg.key_exists(key) => {
                out.keys.push(ExportedKey { key: key.clone(), reg_file: reg_file.clone() })
            }
            PendingKind::ValueExport { key, name, reg_file } if reg_file.is_file() && !value_exists(ops, key, name) => {
                out.values.push(RemovedValue { key: key.clone(), name: name.clone(), reg_file: reg_file.clone() })
            }
            PendingKind::ServiceExport { name, reg_file } if reg_file.is_file() && !service_exists(ops, name) => {
                out.services.push(RemovedService { name: name.clone(), reg_file: reg_file.clone() })
            }
            PendingKind::TaskBackup { name, xml_backup } if xml_backup.is_file() => out.tasks.push(RemovedTask {
                name: name.clone(),
                xml_backup: xml_backup.clone(),
                original_xml: PathBuf::new(),
            }),
            _ => {}
        }
    }
    out
}

fn value_exists(ops: &QuarantineOps, k: &RegKeyPath, name: &str) -> bool {
    ops.reg.values(k).is_ok_and(|vals| vals.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)))
}

fn service_key(name: &str) -> RegKeyPath {
    key(Hive::LocalMachine, &format!(r"{SERVICES_KEY}\{name}"))
}

fn service_exists(ops: &QuarantineOps, name: &str) -> bool {
    ops.reg.key_exists(&service_key(name))
}

/// Removes the `known` stored paths below each payload `<id>` folder (never through a link), then
/// the folders left empty bottom-up. Returns one [`UNKNOWN_CONTENT_LEFT`] failure per payload
/// folder that still holds anything (it is kept), and any removal error.
fn clean_payload(ops: &QuarantineOps, id: &QuarantineId, known: &[PathBuf]) -> Vec<Failure> {
    let mut failures = Vec::new();
    for dir in payload_dirs(ops, id) {
        let root = dir.parent().unwrap_or(&dir);
        for k in known.iter().filter(|k| is_under(k, &dir) && ops.mover.exists(k)) {
            if let Some(p) = reparse_in_chain(ops.mover.as_ref(), root, k) {
                log::warn!("quarantine {}: not removing {}: reparse point {}", id.0, k.display(), p.display());
                continue;
            }
            if let Err(error) = ops.mover.remove_all(k) {
                failures.push(Failure { what: k.display().to_string(), error });
            }
        }
        match ops.mover.remove_empty_dirs(&dir) {
            Ok(true) => {}
            Ok(false) => failures.push(Failure { what: dir.display().to_string(), error: UNKNOWN_CONTENT_LEFT.to_string() }),
            Err(error) => failures.push(Failure { what: dir.display().to_string(), error }),
        }
    }
    failures
}

/// Purges every quarantine created more than `days` days before `now`; `0` turns this off.
/// Quarantines whose folder `ops.trusted_dir` does not vouch for, or with items on a drive that
/// is not connected ([`DRIVE_MISSING`]), are skipped. Returns the purged ids.
pub fn auto_purge(ops: &QuarantineOps, days: u32, now: DateTime<Utc>) -> Vec<QuarantineId> {
    if days == 0 {
        return Vec::new();
    }
    let Some(cutoff) = now.checked_sub_signed(Duration::days(i64::from(days))) else { return Vec::new() };
    if let Err(e) = secure_manifest_dir(ops) {
        log::warn!("auto-purge: {e}");
        return Vec::new();
    }
    list_trusted_manifests(&ops.manifest_dir, &*ops.trusted_dir, &|p| ops.mover.is_reparse(p))
        .into_iter()
        .filter(|m| m.created < cutoff)
        .filter_map(|m| match purge_secured(ops, &m) {
            Ok(()) => Some(m.id),
            Err(e) => {
                log::warn!("auto-purge of quarantine {}: {e}", m.id.0);
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsys::{decode_text, FakeFs};
    use crate::leftovers::Confidence;
    use crate::model::Hive;
    use crate::quarantine::fsmove::FakeFileMover;
    use crate::quarantine::store::{list_manifests, new_id};
    use crate::quarantine::{PendingItem, PendingKind};
    use crate::quarantine::{ExportedKey, MovedItem, RemovedService, RemovedTask, RemovedValue};
    use crate::registry::{key, FakeRegistry};
    use crate::regwrite::FakeRegistryWriter;
    use chrono::{Duration, TimeZone};
    use std::cell::RefCell;
    use std::sync::Mutex;

    const TASKS: &str = r"C:\Windows\System32\Tasks";
    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    struct Env {
        ops: QuarantineOps,
        mover: Arc<FakeFileMover>,
        regw: Arc<FakeRegistryWriter>,
        /// stop/delete-task/create-task calls, each with what had happened just before it.
        calls: Arc<Mutex<Vec<String>>>,
        dir: tempfile::TempDir,
    }

    /// Decoded contents of every `task-*.xml` below `dir`.
    fn task_backups(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        for sub in std::fs::read_dir(dir).unwrap().flatten() {
            for f in std::fs::read_dir(sub.path()).into_iter().flatten().flatten() {
                if f.file_name().to_string_lossy().starts_with("task-") {
                    out.push(decode_text(&std::fs::read(f.path()).unwrap()));
                }
            }
        }
        out
    }

    fn env(fs: FakeFs) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let mover = Arc::new(FakeFileMover::new());
        let regw = Arc::new(FakeRegistryWriter::default());
        let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let last_reg = {
            let regw = regw.clone();
            move || regw.log.lock().unwrap().last().cloned().unwrap_or_default()
        };
        let stop_service = {
            let (calls, last_reg) = (calls.clone(), last_reg.clone());
            move |name: &str| -> Result<(), String> {
                calls.lock().unwrap().push(format!("stop {name} after {}", last_reg()));
                Ok(())
            }
        };
        let delete_task = {
            let (calls, d) = (calls.clone(), dir.path().to_path_buf());
            move |name: &str| -> Result<(), String> {
                calls.lock().unwrap().push(format!("delete-task {name} backup={}", task_backups(&d).join("|")));
                Ok(())
            }
        };
        let create_task = {
            let calls = calls.clone();
            move |name: &str, xml: &Path| -> Result<(), String> {
                let file = xml.file_name().unwrap().to_string_lossy().into_owned();
                calls.lock().unwrap().push(format!("create-task {name} from {file} after {}", last_reg()));
                Ok(())
            }
        };
        let ops = QuarantineOps {
            mover: mover.clone(),
            regw: regw.clone(),
            reg: Arc::new(FakeRegistry::new()),
            fs: Arc::new(fs),
            manifest_dir: dir.path().to_path_buf(),
            payload_root_for: Box::new(crate::paths::quarantine_payload_root),
            tasks_root: PathBuf::from(TASKS),
            stop_service: Box::new(stop_service),
            delete_task: Box::new(delete_task),
            create_task: Box::new(create_task),
            trusted_dir: Box::new(|_| Some(true)),
        };
        Env { ops, mover, regw, calls, dir }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 18, 12, 0, 0).unwrap()
    }

    fn item(kind: LeftoverKind) -> Leftover {
        Leftover { kind, confidence: Confidence::High, note: None }
    }

    fn dir_item(p: &str) -> Leftover {
        item(LeftoverKind::Dir { path: PathBuf::from(p), size: None })
    }

    fn file_item(p: &str) -> Leftover {
        item(LeftoverKind::File { path: PathBuf::from(p), size: None })
    }

    fn shortcut_item(p: &str) -> Leftover {
        item(LeftoverKind::Shortcut { lnk: PathBuf::from(p), target: PathBuf::from(r"C:\Apps\Foo\foo.exe") })
    }

    fn key_item(p: &str) -> Leftover {
        item(LeftoverKind::RegKey { key: key(Hive::LocalMachine, p) })
    }

    fn value_item(name: &str) -> Leftover {
        item(LeftoverKind::RegValue { key: key(Hive::CurrentUser, RUN), name: name.to_string() })
    }

    fn service_item(name: &str) -> Leftover {
        item(LeftoverKind::Service { name: name.to_string(), image: PathBuf::from(r"C:\Apps\Foo\svc.exe") })
    }

    fn task_item(name: &str) -> Leftover {
        item(LeftoverKind::ScheduledTask {
            name: name.to_string(),
            xml: Path::new(TASKS).join(name),
            command: PathBuf::from(r"C:\Apps\Foo\update.exe"),
        })
    }

    fn run(e: &Env, items: &[Leftover]) -> QuarantineManifest {
        quarantine(&e.ops, "Foo", items, now(), &|_| {})
    }

    /// `<drive>:\Stale Uninstaller\quarantine\<id>\<rest>`.
    fn stored(drive: char, m: &QuarantineManifest, rest: &str) -> PathBuf {
        PathBuf::from(format!(r"{drive}:\Stale Uninstaller\quarantine\{}\{rest}", m.id.0))
    }

    fn exists(e: &Env, p: &str) -> bool {
        e.mover.exists(Path::new(p))
    }

    fn fail(what: &str, error: &str) -> Failure {
        Failure { what: what.to_string(), error: error.to_string() }
    }

    /// The mover log line for locking down the manifest dir.
    fn ensure_manifest_dir(e: &Env) -> String {
        format!("ensure {}", e.dir.path().display())
    }

    #[test]
    fn parse_sc_state_reads_the_state_line() {
        let running = "SERVICE_NAME: Spooler \r\n        TYPE               : 110  WIN32_OWN_PROCESS  (interactive)\r\n        STATE              : 4  RUNNING \r\n                                (STOPPABLE, NOT_PAUSABLE, IGNORES_SHUTDOWN)\r\n        WIN32_EXIT_CODE    : 0  (0x0)\r\n";
        assert_eq!(parse_sc_state(running).as_deref(), Some("RUNNING"));
        assert_eq!(parse_sc_state("        STATE              : 3  STOP_PENDING\n").as_deref(), Some("STOP_PENDING"));
        assert_eq!(parse_sc_state("        STATE              : 1  STOPPED\n").as_deref(), Some("STOPPED"));
        assert_eq!(parse_sc_state("        STATE              : 1\n").as_deref(), Some("STOPPED"), "number only");
        assert_eq!(parse_sc_state("[SC] EnumQueryServicesStatus:OpenService FAILED 1060:"), None);
    }

    #[test]
    fn new_id_is_timestamp_and_four_hex_digits() {
        let id = new_id(now()).0;
        assert_eq!(&id[..16], "20261018-120000-", "{id}");
        assert_eq!(id.len(), 20, "{id}");
        assert!(id[16..].chars().all(|c| c.is_ascii_hexdigit()), "{id}");
    }

    #[test]
    fn quarantine_moves_files_exports_keys_and_saves_manifest() {
        let e = env(FakeFs::new());
        e.mover.add_file(r"C:\Apps\Foo\foo.exe", 10);
        e.mover.add_file(r"C:\Apps\Foo\sub\a.dll", 5);
        e.mover.add_file(r"C:\Users\A\AppData\Roaming\Foo\cfg.ini", 3);
        e.mover.add_file(r"C:\Users\A\Desktop\Foo.lnk", 1);
        let items = [
            dir_item(r"C:\Apps\Foo"),
            key_item(r"SOFTWARE\Foo"),
            value_item("Foo"),
            value_item("FooHelper"),
            file_item(r"C:\Users\A\AppData\Roaming\Foo\cfg.ini"),
            shortcut_item(r"C:\Users\A\Desktop\Foo.lnk"),
        ];
        let events = RefCell::new(Vec::new());
        let m = quarantine(&e.ops, "Foo", &items, now(), &|p| {
            events.borrow_mut().push(match p {
                QuarantineProgress::Item { done, total, .. } => format!("{done}/{total}"),
                QuarantineProgress::Finished(m) => format!("finished {}", m.id.0),
            })
        });
        let mdir = e.dir.path().join(&m.id.0);
        assert_eq!((m.program_name.as_str(), m.created), ("Foo", now()));
        assert!(m.failures.is_empty(), "{:?}", m.failures);
        assert_eq!(
            m.moved,
            vec![
                MovedItem {
                    original: PathBuf::from(r"C:\Users\A\Desktop\Foo.lnk"),
                    stored: stored('C', &m, r"C\Users\A\Desktop\Foo.lnk"),
                    is_dir: false,
                    size: 1,
                },
                MovedItem { original: PathBuf::from(r"C:\Apps\Foo"), stored: stored('C', &m, r"C\Apps\Foo"), is_dir: true, size: 15 },
                MovedItem {
                    original: PathBuf::from(r"C:\Users\A\AppData\Roaming\Foo\cfg.ini"),
                    stored: stored('C', &m, r"C\Users\A\AppData\Roaming\Foo\cfg.ini"),
                    is_dir: false,
                    size: 3,
                },
            ]
        );
        assert_eq!(m.keys, vec![ExportedKey { key: key(Hive::LocalMachine, r"SOFTWARE\Foo"), reg_file: mdir.join("key-0.reg") }]);
        let removed = |name: &str| RemovedValue { key: key(Hive::CurrentUser, RUN), name: name.to_string(), reg_file: mdir.join("value-0.reg") };
        assert_eq!(m.values, vec![removed("Foo"), removed("FooHelper")]);
        assert_eq!(
            *e.regw.log.lock().unwrap(),
            vec![
                format!(r"export HKCU\{RUN} -> {}", mdir.join("value-0.reg").display()),
                format!(r"delete-value HKCU\{RUN}\Foo"),
                format!(r"delete-value HKCU\{RUN}\FooHelper"),
                format!(r"export HKLM\SOFTWARE\Foo -> {}", mdir.join("key-0.reg").display()),
                r"delete HKLM\SOFTWARE\Foo".to_string(),
            ]
        );
        assert!(!exists(&e, r"C:\Apps\Foo"));
        assert!(e.mover.exists(&stored('C', &m, r"C\Apps\Foo\sub\a.dll")));
        assert!(mdir.join("manifest.json").is_file());
        assert_eq!(list_manifests(e.dir.path()), vec![m.clone()]);
        let expected: Vec<String> = (0..6).map(|i| format!("{i}/6")).chain([format!("finished {}", m.id.0)]).collect();
        assert_eq!(events.into_inner(), expected);
    }

    #[test]
    fn locked_file_is_recorded_and_rest_continues() {
        let e = env(FakeFs::new());
        e.mover.add_file(r"C:\Apps\Foo\foo.exe", 10);
        e.mover.add_file(r"C:\Apps\Foo\in-use.dll", 4);
        e.mover.lock(r"C:\Apps\Foo\in-use.dll");
        e.mover.add_file(r"C:\ProgramData\Foo\data.bin", 7);
        let m = run(&e, &[dir_item(r"C:\Apps\Foo"), dir_item(r"C:\ProgramData\Foo"), key_item(r"SOFTWARE\Foo")]);
        assert_eq!(m.failures, vec![fail(r"C:\Apps\Foo", r"locked: C:\Apps\Foo\in-use.dll")]);
        assert_eq!(m.moved.len(), 1);
        assert_eq!(m.moved[0].original, PathBuf::from(r"C:\ProgramData\Foo"));
        assert_eq!(m.keys.len(), 1);
        assert!(exists(&e, r"C:\Apps\Foo\foo.exe") && exists(&e, r"C:\Apps\Foo\in-use.dll"));
        assert!(!exists(&e, r"C:\ProgramData\Foo"));
        assert_eq!(list_manifests(e.dir.path()), vec![m.clone()]);

        // Undo still works for what did move.
        assert_eq!(restore(&e.ops, &m), RestoreOutcome::Restored);
        assert!(exists(&e, r"C:\ProgramData\Foo\data.bin"));
        assert!(e.regw.log.lock().unwrap().iter().any(|l| l.starts_with("import") && l.ends_with("key-0.reg")));
        assert!(list_manifests(e.dir.path()).is_empty());
    }

    #[test]
    fn payload_root_is_per_drive() {
        let e = env(FakeFs::new());
        e.mover.add_file(r"D:\Games\Foo\foo.exe", 10);
        e.mover.add_file(r"C:\Apps\Foo\x.dll", 1);
        e.mover.add_file(r"E:\Foo\y.dat", 2);
        e.mover.set_volume_ok('E', false);
        let m = run(&e, &[dir_item(r"D:\Games\Foo"), dir_item(r"C:\Apps\Foo"), dir_item(r"E:\Foo")]);
        assert!(m.failures.is_empty(), "{:?}", m.failures);
        let stores: Vec<PathBuf> = m.moved.iter().map(|i| i.stored.clone()).collect();
        let (d, c, e_on_c) = (stored('D', &m, r"D\Games\Foo"), stored('C', &m, r"C\Apps\Foo"), stored('C', &m, r"E\Foo"));
        assert_eq!(stores, vec![d.clone(), c.clone(), e_on_c.clone()]);
        assert_eq!(
            *e.mover.log.lock().unwrap(),
            vec![
                ensure_manifest_dir(&e),
                format!("create-locked {}", e.dir.path().join(&m.id.0).display()),
                r"ensure D:\Stale Uninstaller".to_string(),
                r"ensure D:\Stale Uninstaller\quarantine".to_string(),
                format!(r"create-locked D:\Stale Uninstaller\quarantine\{}", m.id.0),
                format!(r"move D:\Games\Foo -> {}", d.display()),
                r"ensure C:\Stale Uninstaller".to_string(),
                r"ensure C:\Stale Uninstaller\quarantine".to_string(),
                format!(r"create-locked C:\Stale Uninstaller\quarantine\{}", m.id.0),
                format!(r"move C:\Apps\Foo -> {}", c.display()),
                format!(r"move E:\Foo -> {}", e_on_c.display()),
            ]
        );
        assert_eq!(m.size_per_drive().into_iter().collect::<Vec<_>>(), vec![('C', 3), ('D', 10)]);
    }

    #[test]
    fn restore_puts_everything_back_and_removes_manifest() {
        let e = env(FakeFs::new().text(&format!(r"{TASKS}\Foo\Updater"), "<Task>u</Task>"));
        e.mover.add_file(r"C:\Apps\Foo\foo.exe", 10);
        e.mover.add_file(r"C:\Users\A\Desktop\Foo.lnk", 1);
        let items = [
            service_item("FooSvc"),
            task_item(r"Foo\Updater"),
            value_item("Foo"),
            key_item(r"SOFTWARE\Foo"),
            shortcut_item(r"C:\Users\A\Desktop\Foo.lnk"),
            dir_item(r"C:\Apps\Foo"),
        ];
        let m = run(&e, &items);
        assert!(m.failures.is_empty(), "{:?}", m.failures);
        let mdir = e.dir.path().join(&m.id.0);
        e.mover.log.lock().unwrap().clear();
        e.regw.log.lock().unwrap().clear();
        e.calls.lock().unwrap().clear();

        assert_eq!(restore(&e.ops, &m), RestoreOutcome::Restored);
        assert!(exists(&e, r"C:\Apps\Foo\foo.exe") && exists(&e, r"C:\Users\A\Desktop\Foo.lnk"));
        assert_eq!(
            *e.mover.log.lock().unwrap(),
            vec![
                ensure_manifest_dir(&e),
                format!(r"move {} -> C:\Apps\Foo", stored('C', &m, r"C\Apps\Foo").display()),
                format!(r"move {} -> C:\Users\A\Desktop\Foo.lnk", stored('C', &m, r"C\Users\A\Desktop\Foo.lnk").display()),
                format!(r"prune C:\Stale Uninstaller\quarantine\{}", m.id.0),
            ]
        );
        let import = |f: &str| format!("import {}", mdir.join(f).display());
        assert_eq!(*e.regw.log.lock().unwrap(), vec![import("key-0.reg"), import("value-0.reg"), import("service-FooSvc.reg")]);
        assert_eq!(*e.calls.lock().unwrap(), vec![format!(r"create-task Foo\Updater from task-0.xml after {}", import("value-0.reg"))]);
        assert!(!mdir.exists());
        assert!(list_manifests(e.dir.path()).is_empty());
    }

    #[test]
    fn restore_skips_occupied_original_and_keeps_manifest() {
        let e = env(FakeFs::new());
        e.mover.add_file(r"C:\Apps\Foo\foo.exe", 10);
        e.mover.add_file(r"C:\ProgramData\Foo\data.bin", 2);
        let m = run(&e, &[dir_item(r"C:\Apps\Foo"), dir_item(r"C:\ProgramData\Foo"), key_item(r"SOFTWARE\Foo")]);
        e.mover.add_file(r"C:\Apps\Foo\reinstalled.exe", 1);

        assert_eq!(restore(&e.ops, &m), RestoreOutcome::Partial(vec![fail(r"C:\Apps\Foo", "occupied")]));
        assert!(exists(&e, r"C:\ProgramData\Foo\data.bin"));
        assert!(e.mover.exists(&stored('C', &m, r"C\Apps\Foo\foo.exe")));
        assert!(!exists(&e, r"C:\Apps\Foo\foo.exe"));
        assert!(!e.mover.log.lock().unwrap().iter().any(|l| l.starts_with("remove")));
        let left = list_manifests(e.dir.path());
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].moved, vec![m.moved[0].clone()]);
        assert!(left[0].keys.is_empty());
    }

    #[test]
    fn purge_deletes_payload_and_manifest() {
        let e = env(FakeFs::new());
        e.mover.add_file(r"D:\Games\Foo\a.pak", 5);
        e.mover.add_file(r"C:\Apps\Foo\b.dll", 1);
        let m = run(&e, &[dir_item(r"D:\Games\Foo"), dir_item(r"C:\Apps\Foo"), key_item(r"SOFTWARE\Foo")]);
        e.mover.log.lock().unwrap().clear();

        purge(&e.ops, &m).unwrap();
        assert_eq!(
            *e.mover.log.lock().unwrap(),
            vec![
                ensure_manifest_dir(&e),
                format!(r"remove {}", stored('C', &m, r"C\Apps\Foo").display()),
                format!(r"prune C:\Stale Uninstaller\quarantine\{}", m.id.0),
                format!(r"remove {}", stored('D', &m, r"D\Games\Foo").display()),
                format!(r"prune D:\Stale Uninstaller\quarantine\{}", m.id.0),
            ]
        );
        assert!(m.moved.iter().all(|i| !e.mover.exists(&i.stored) && !e.mover.exists(&i.original)));
        assert!(!e.dir.path().join(&m.id.0).exists());
        assert!(list_manifests(e.dir.path()).is_empty());
        assert!(!e.regw.log.lock().unwrap().iter().any(|l| l.starts_with("import")));
    }

    #[test]
    fn auto_purge_respects_days_and_zero_means_off() {
        let e = env(FakeFs::new());
        e.mover.add_file(r"C:\Old\a", 1);
        e.mover.add_file(r"C:\New\b", 1);
        let old = quarantine(&e.ops, "Old", &[dir_item(r"C:\Old")], now() - Duration::days(40), &|_| {});
        let new = quarantine(&e.ops, "New", &[dir_item(r"C:\New")], now() - Duration::days(5), &|_| {});

        assert_eq!(auto_purge(&e.ops, 0, now()), vec![]);
        assert_eq!(auto_purge(&e.ops, u32::MAX, now()), vec![], "cutoff before chrono's range purges nothing");
        assert_eq!(list_manifests(e.dir.path()), vec![new.clone(), old.clone()], "newest first");
        assert_eq!(auto_purge(&e.ops, 30, now()), vec![old.id.clone()]);
        assert_eq!(list_manifests(e.dir.path()), vec![new.clone()]);
        assert!(!e.mover.exists(&old.moved[0].stored));
        assert!(e.mover.exists(&new.moved[0].stored));
    }

    #[test]
    fn service_and_task_are_exported_before_removal() {
        let e = env(FakeFs::new().text(&format!(r"{TASKS}\Foo\Updater"), "<Task>u</Task>"));
        let services = r"HKLM\SYSTEM\CurrentControlSet\Services";
        e.regw.fail_on.lock().unwrap().push(format!(r"export {services}\BarSvc"));
        let m = run(&e, &[service_item("FooSvc"), service_item("BarSvc"), task_item(r"Foo\Updater")]);
        let mdir = e.dir.path().join(&m.id.0);
        assert_eq!(
            *e.calls.lock().unwrap(),
            vec![
                format!(r"stop FooSvc after export {services}\FooSvc -> {}", mdir.join("service-FooSvc.reg").display()),
                r"delete-task Foo\Updater backup=<Task>u</Task>".to_string(),
            ]
        );
        assert_eq!(m.services, vec![RemovedService { name: "FooSvc".to_string(), reg_file: mdir.join("service-FooSvc.reg") }]);
        assert_eq!(
            m.tasks,
            vec![RemovedTask {
                name: r"Foo\Updater".to_string(),
                xml_backup: mdir.join("task-0.xml"),
                original_xml: PathBuf::from(format!(r"{TASKS}\Foo\Updater")),
            }]
        );
        assert_eq!(m.failures, vec![fail("BarSvc", "forced failure")]);
    }

    #[test]
    fn partial_move_is_recorded_as_moved_and_failure() {
        let e = env(FakeFs::new());
        e.mover.add_file(r"C:\Apps\Foo\a.dll", 4);
        e.mover.fail_partially(r"C:\Apps\Foo");
        let m = run(&e, &[dir_item(r"C:\Apps\Foo")]);
        let s = stored('C', &m, r"C\Apps\Foo");
        assert_eq!(m.moved, vec![MovedItem { original: PathBuf::from(r"C:\Apps\Foo"), stored: s.clone(), is_dir: true, size: 4 }]);
        assert_eq!(m.failures, vec![fail(r"C:\Apps\Foo", "partial move: forced partial move")]);
        assert!(e.mover.exists(&s.join("a.dll")));
        assert_eq!(list_manifests(e.dir.path()), vec![m]);
    }

    #[test]
    fn reparse_in_payload_path_is_refused() {
        let e = env(FakeFs::new());
        e.mover.add_file(r"C:\Apps\Foo\a.dll", 1);
        e.mover.set_reparse(r"C:\Stale Uninstaller\quarantine");
        let m = run(&e, &[dir_item(r"C:\Apps\Foo")]);
        assert!(m.moved.is_empty());
        assert_eq!(m.failures, vec![fail(r"C:\Apps\Foo", r"reparse point in quarantine path: C:\Stale Uninstaller\quarantine")]);
        assert!(exists(&e, r"C:\Apps\Foo\a.dll"));
        assert!(!e.mover.log.lock().unwrap().iter().any(|l| l.starts_with("move")));

        // A junction that appears inside the payload tree later blocks the restore move.
        let e = env(FakeFs::new());
        e.mover.add_file(r"C:\Apps\Foo\a.dll", 1);
        let m = run(&e, &[dir_item(r"C:\Apps\Foo")]);
        let parent = m.moved[0].stored.parent().unwrap().to_path_buf();
        e.mover.set_reparse(parent.to_str().unwrap());
        let err = format!("reparse point in quarantine path: {}", parent.display());
        assert_eq!(restore(&e.ops, &m), RestoreOutcome::Partial(vec![fail(r"C:\Apps\Foo", &err)]));
        assert!(e.mover.exists(&m.moved[0].stored.join("a.dll")));
        assert!(!exists(&e, r"C:\Apps\Foo"));
        assert_eq!(list_manifests(e.dir.path()).len(), 1);
    }

    #[test]
    fn nested_child_listed_before_parent_is_covered_by_parent() {
        let e = env(FakeFs::new());
        e.mover.add_file(r"C:\Apps\Foo\foo.exe", 10);
        e.mover.add_file(r"C:\Apps\Foo\Foo.lnk", 1);
        let events = RefCell::new(Vec::new());
        let items = [shortcut_item(r"C:\Apps\Foo\Foo.lnk"), dir_item(r"C:\Apps\Foo")];
        let m = quarantine(&e.ops, "Foo", &items, now(), &|p| {
            if let QuarantineProgress::Item { done, total, what } = p {
                events.borrow_mut().push(format!("{done}/{total} {what}"));
            }
        });
        assert!(m.failures.is_empty(), "{:?}", m.failures);
        assert_eq!(m.moved, vec![MovedItem { original: PathBuf::from(r"C:\Apps\Foo"), stored: stored('C', &m, r"C\Apps\Foo"), is_dir: true, size: 11 }]);
        assert!(e.mover.exists(&stored('C', &m, r"C\Apps\Foo\Foo.lnk")));
        assert_eq!(e.mover.log.lock().unwrap().iter().filter(|l| l.starts_with("move")).count(), 1);
        assert_eq!(events.into_inner(), vec![r"0/2 C:\Apps\Foo\Foo.lnk".to_string(), r"1/2 C:\Apps\Foo".to_string()]);
    }

    #[test]
    fn nested_child_listed_after_parent_is_not_a_failure() {
        let e = env(FakeFs::new());
        e.mover.add_file(r"C:\Apps\Foo\foo.exe", 10);
        e.mover.add_file(r"C:\Apps\Foo\cfg\settings.ini", 2);
        e.mover.add_file(r"C:\Apps\FooBar\other.exe", 3);
        let items = [
            dir_item(r"C:\Apps\Foo"),
            dir_item(r"c:\apps\foo\cfg"),
            file_item(r"C:\Apps\Foo\cfg\settings.ini"),
            dir_item(r"C:\Apps\FooBar"),
        ];
        let m = run(&e, &items);
        assert!(m.failures.is_empty(), "{:?}", m.failures);
        let originals: Vec<PathBuf> = m.moved.iter().map(|i| i.original.clone()).collect();
        assert_eq!(originals, vec![PathBuf::from(r"C:\Apps\Foo"), PathBuf::from(r"C:\Apps\FooBar")], "a name-prefix sibling is not nested");
        assert!(e.mover.exists(&stored('C', &m, r"C\Apps\Foo\cfg\settings.ini")));
    }

    #[test]
    fn restore_refuses_reg_file_outside_manifest_dir() {
        let e = env(FakeFs::new());
        let m = run(&e, &[key_item(r"SOFTWARE\Foo"), key_item(r"SOFTWARE\Bar")]);
        let mut tampered = m.clone();
        let planted = e.dir.path().join("key-0.reg");
        std::fs::write(&planted, "Windows Registry Editor Version 5.00").unwrap();
        tampered.keys[0].reg_file = planted.clone();
        tampered.keys[1].reg_file = e.dir.path().join(&m.id.0).join("evil.reg");
        e.regw.log.lock().unwrap().clear();

        let outside = |p: &Path| format!("backup file outside the quarantine folder: {}", p.display());
        assert_eq!(
            restore(&e.ops, &tampered),
            RestoreOutcome::Partial(vec![
                fail(r"HKLM\SOFTWARE\Bar", &outside(&tampered.keys[1].reg_file)),
                fail(r"HKLM\SOFTWARE\Foo", &outside(&planted)),
            ])
        );
        assert!(e.regw.log.lock().unwrap().is_empty(), "nothing imported");
        assert_eq!(list_manifests(e.dir.path())[0].keys, tampered.keys);
    }

    #[test]
    fn restore_refuses_task_backup_outside_manifest_dir() {
        let e = env(FakeFs::new().text(&format!(r"{TASKS}\Foo\Updater"), "<Task>u</Task>"));
        let m = run(&e, &[task_item(r"Foo\Updater")]);
        let mut tampered = m.clone();
        let other = tempfile::tempdir().unwrap();
        tampered.tasks[0].xml_backup = other.path().join(&m.id.0).join("task-0.xml");
        std::fs::create_dir_all(other.path().join(&m.id.0)).unwrap();
        e.calls.lock().unwrap().clear();

        let err = format!("backup file outside the quarantine folder: {}", tampered.tasks[0].xml_backup.display());
        assert_eq!(restore(&e.ops, &tampered), RestoreOutcome::Partial(vec![fail(r"Foo\Updater", &err)]));
        assert!(e.calls.lock().unwrap().is_empty(), "no task created");
    }

    #[test]
    fn restore_refuses_reparse_in_destination() {
        let e = env(FakeFs::new());
        e.mover.add_file(r"C:\Apps\Foo\a.dll", 1);
        let m = run(&e, &[dir_item(r"C:\Apps\Foo")]);
        e.mover.set_reparse(r"C:\Apps");
        assert_eq!(restore(&e.ops, &m), RestoreOutcome::Partial(vec![fail(r"C:\Apps\Foo", r"reparse-in-destination: C:\Apps")]));
        assert!(e.mover.exists(&m.moved[0].stored.join("a.dll")));
        assert!(!exists(&e, r"C:\Apps\Foo"));
    }

    #[test]
    fn failed_group_save_stops_further_groups() {
        let mut e = env(FakeFs::new().text(&format!(r"{TASKS}\Foo\Updater"), "<Task>u</Task>"));
        // Stopping the service replaces the manifest folder with a file, so the next save fails.
        let dir = e.dir.path().to_path_buf();
        e.ops.stop_service = Box::new(move |_| {
            for sub in std::fs::read_dir(&dir).unwrap().flatten() {
                std::fs::remove_dir_all(sub.path()).unwrap();
                std::fs::write(sub.path(), b"x").unwrap();
            }
            Ok(())
        });
        e.mover.add_file(r"C:\Apps\Foo\a.dll", 1);
        let m = run(&e, &[service_item("FooSvc"), task_item(r"Foo\Updater"), key_item(r"SOFTWARE\Foo"), dir_item(r"C:\Apps\Foo")]);
        assert_eq!(m.services.len(), 1);
        assert_eq!(m.failures.len(), 1, "{:?}", m.failures);
        assert_eq!(m.failures[0].what, "manifest");
        assert!(m.failures[0].error.starts_with("manifest-save-failed: "), "{:?}", m.failures);
        assert!(m.tasks.is_empty() && m.keys.is_empty() && m.moved.is_empty());
        assert!(e.calls.lock().unwrap().is_empty(), "task not deleted");
        assert_eq!(e.regw.log.lock().unwrap().len(), 1, "only the service export");
        assert!(exists(&e, r"C:\Apps\Foo\a.dll"));
    }

    #[test]
    fn restore_refuses_untrusted_manifest_dir() {
        let mut e = env(FakeFs::new());
        e.mover.add_file(r"C:\Apps\Foo\a.dll", 1);
        let m = run(&e, &[dir_item(r"C:\Apps\Foo"), key_item(r"SOFTWARE\Foo")]);
        let folder = e.dir.path().join(&m.id.0);
        e.ops.trusted_dir = Box::new(|_| Some(false));
        e.mover.log.lock().unwrap().clear();
        e.regw.log.lock().unwrap().clear();

        let untrusted = format!("quarantine folder not owned by Administrators or SYSTEM: {}", folder.display());
        assert_eq!(restore(&e.ops, &m), RestoreOutcome::Partial(vec![fail("manifest", &untrusted)]));
        assert!(!e.mover.log.lock().unwrap().iter().any(|l| l.starts_with("move")), "nothing moved back");
        assert!(e.regw.log.lock().unwrap().is_empty(), "nothing imported");
        assert!(e.mover.exists(&m.moved[0].stored.join("a.dll")));

        e.ops.trusted_dir = Box::new(|_| None);
        assert_eq!(restore(&e.ops, &m), RestoreOutcome::Partial(vec![fail("manifest", &untrusted)]), "unknown owner is refused too");
        assert_eq!(purge(&e.ops, &m), Err(untrusted.clone()));

        // A junction planted as `<id>` is refused even when its target looks trusted.
        e.ops.trusted_dir = Box::new(|_| Some(true));
        e.mover.set_reparse(&folder.display().to_string());
        assert_eq!(restore(&e.ops, &m), RestoreOutcome::Partial(vec![fail("manifest", &untrusted)]));
        assert_eq!(purge(&e.ops, &m), Err(untrusted));
        assert_eq!(auto_purge(&e.ops, 1, now() + Duration::days(30)), vec![]);
        assert!(e.mover.exists(&m.moved[0].stored.join("a.dll")), "purge refused");
        assert!(folder.exists());
    }

    #[test]
    fn auto_purge_skips_untrusted_dirs() {
        let mut e = env(FakeFs::new());
        e.mover.add_file(r"C:\Old\a", 1);
        e.mover.add_file(r"C:\Planted\b", 1);
        let old = quarantine(&e.ops, "Old", &[dir_item(r"C:\Old")], now() - Duration::days(40), &|_| {});
        let planted = quarantine(&e.ops, "Planted", &[dir_item(r"C:\Planted")], now() - Duration::days(41), &|_| {});
        let trusted_folder = e.dir.path().join(&old.id.0);
        e.ops.trusted_dir = Box::new(move |p| Some(p == trusted_folder));

        assert_eq!(auto_purge(&e.ops, 30, now()), vec![old.id.clone()]);
        assert!(!e.mover.exists(&old.moved[0].stored));
        assert!(e.mover.exists(&planted.moved[0].stored), "untrusted quarantine is left alone");
        assert_eq!(list_manifests(e.dir.path()), vec![planted]);
    }

    #[test]
    fn crash_between_move_and_record_is_recoverable() {
        let e = env(FakeFs::new());
        e.mover.add_file(r"C:\Apps\Foo\a.dll", 4);
        let done = run(&e, &[dir_item(r"C:\Apps\Foo")]);
        // As if the process died after the move but before its outcome was saved.
        let mut crashed = done.clone();
        let item = crashed.moved.pop().unwrap();
        crashed.pending.push(PendingItem {
            kind: PendingKind::Move { original: item.original.clone(), stored: item.stored.clone() },
            started: now(),
        });
        // A pending move that never happened (nothing stored) is simply dropped.
        crashed.pending.push(PendingItem {
            kind: PendingKind::Move { original: PathBuf::from(r"C:\Apps\Bar"), stored: stored('C', &done, r"C\Apps\Bar") },
            started: now(),
        });
        save_manifest(e.dir.path(), &crashed).unwrap();
        assert_eq!(list_manifests(e.dir.path())[0].pending.len(), 2, "pending survives a reload");

        assert_eq!(restore(&e.ops, &crashed), RestoreOutcome::Restored);
        assert!(exists(&e, r"C:\Apps\Foo\a.dll"));
        assert!(!e.mover.exists(&item.stored));
        assert!(list_manifests(e.dir.path()).is_empty());
    }

    #[test]
    fn pending_record_is_saved_before_each_destructive_step() {
        let mut e = env(FakeFs::new());
        let dir = e.dir.path().to_path_buf();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        e.ops.stop_service = Box::new(move |_| {
            seen2.lock().unwrap().extend(list_manifests(&dir).into_iter().flat_map(|m| m.pending));
            Ok(())
        });
        let m = run(&e, &[service_item("FooSvc")]);
        let reg_file = e.dir.path().join(&m.id.0).join("service-FooSvc.reg");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].kind, PendingKind::ServiceExport { name: "FooSvc".to_string(), reg_file });
        assert!(m.pending.is_empty());
        assert!(list_manifests(e.dir.path())[0].pending.is_empty(), "the outcome replaces the pending record");
        assert_eq!(m.services.len(), 1);
    }

    #[test]
    fn purge_keeps_unknown_payload() {
        let e = env(FakeFs::new());
        e.mover.add_file(r"C:\Apps\Foo\a.dll", 4);
        let m = run(&e, &[dir_item(r"C:\Apps\Foo")]);
        let dir = PathBuf::from(format!(r"C:\Stale Uninstaller\quarantine\{}", m.id.0));
        e.mover.add_file(dir.join("stray.bin").to_str().unwrap(), 1);

        let err = purge(&e.ops, &m).unwrap_err();
        assert_eq!(err, format!("{UNKNOWN_CONTENT_LEFT}: {}", dir.display()));
        assert!(!e.mover.exists(&m.moved[0].stored), "the known payload is gone");
        assert!(e.mover.exists(&dir.join("stray.bin")), "the unknown file is untouched");
        assert!(!e.mover.log.lock().unwrap().iter().any(|l| *l == format!("remove {}", dir.display())), "never removed wholesale");
        let kept = list_manifests(e.dir.path());
        assert_eq!(kept.len(), 1, "manifest kept");
        assert_eq!(kept[0].failures, vec![fail(&dir.display().to_string(), UNKNOWN_CONTENT_LEFT)]);

        // A retry reports it again without piling up failures; a restore keeps it too.
        assert!(purge(&e.ops, &kept[0]).is_err());
        assert_eq!(list_manifests(e.dir.path())[0].failures.len(), 1);
        assert_eq!(
            restore(&e.ops, &kept[0]),
            RestoreOutcome::Partial(vec![fail(&dir.display().to_string(), UNKNOWN_CONTENT_LEFT)])
        );
        assert!(e.mover.exists(&dir.join("stray.bin")));
        assert_eq!(list_manifests(e.dir.path()).len(), 1);
    }

    #[test]
    fn failed_export_skips_delete() {
        let e = env(FakeFs::new());
        e.regw.fail_on.lock().unwrap().push(r"export HKLM\SOFTWARE\Foo".to_string());
        e.regw.fail_on.lock().unwrap().push(format!(r"export HKCU\{RUN}"));
        let m = run(&e, &[key_item(r"SOFTWARE\Foo"), value_item("Foo")]);
        assert!(!e.regw.log.lock().unwrap().iter().any(|l| l.starts_with("delete")), "{:?}", e.regw.log.lock().unwrap());
        assert!(m.keys.is_empty() && m.values.is_empty() && m.pending.is_empty());
        assert_eq!(m.failures.len(), 2, "{:?}", m.failures);
    }

    #[test]
    fn restore_skips_existing_key_and_keeps_it() {
        let mut e = env(FakeFs::new());
        let m = run(&e, &[key_item(r"SOFTWARE\Foo"), key_item(r"SOFTWARE\Bar"), value_item("Foo"), service_item("FooSvc")]);
        assert!(m.failures.is_empty(), "{:?}", m.failures);
        // The program was reinstalled: its key, Run value and service exist again.
        let mut reg = FakeRegistry::new();
        reg.add_key(key(Hive::LocalMachine, r"SOFTWARE\Foo"));
        reg.set(key(Hive::CurrentUser, RUN), "Foo", crate::registry::RegValue::Str("foo.exe".into()));
        reg.add_key(key(Hive::LocalMachine, r"SYSTEM\CurrentControlSet\Services\FooSvc"));
        e.ops.reg = Arc::new(reg);
        e.regw.log.lock().unwrap().clear();

        assert_eq!(
            restore(&e.ops, &m),
            RestoreOutcome::Partial(vec![
                fail(r"HKLM\SOFTWARE\Foo", "occupied"),
                fail(&format!(r"HKCU\{RUN}\Foo"), "occupied"),
                fail("FooSvc", "occupied"),
            ])
        );
        let mdir = e.dir.path().join(&m.id.0);
        assert_eq!(*e.regw.log.lock().unwrap(), vec![format!("import {}", mdir.join("key-1.reg").display())], "only Bar");
        let left = list_manifests(e.dir.path());
        assert_eq!(left.len(), 1, "manifest kept");
        assert_eq!(left[0].keys, vec![m.keys[0].clone()]);
        assert_eq!(left[0].values, m.values);
        assert_eq!(left[0].services, m.services);
    }

    #[test]
    fn purge_with_missing_drive_keeps_manifest() {
        let e = env(FakeFs::new());
        e.mover.add_file(r"D:\Games\Foo\a.pak", 5);
        e.mover.add_file(r"C:\Apps\Foo\b.dll", 1);
        let m = quarantine(&e.ops, "Foo", &[dir_item(r"D:\Games\Foo"), dir_item(r"C:\Apps\Foo")], now() - Duration::days(40), &|_| {});
        assert!(m.failures.is_empty(), "{:?}", m.failures);
        // D: is unplugged.
        e.mover.remove_all(Path::new(r"D:\")).unwrap();
        e.mover.log.lock().unwrap().clear();

        assert_eq!(purge(&e.ops, &m), Err("drive-missing: D:".to_string()));
        assert_eq!(auto_purge(&e.ops, 30, now()), vec![], "auto-purge skips it");
        assert!(e.mover.exists(&m.moved[1].stored), "nothing on C: was purged either");
        assert!(!e.mover.log.lock().unwrap().iter().any(|l| l.starts_with("remove") || l.starts_with("prune")));
        assert_eq!(list_manifests(e.dir.path()), vec![m]);
    }

    fn sized_dir(p: &str, size: u64) -> Leftover {
        item(LeftoverKind::Dir { path: PathBuf::from(p), size: Some(size) })
    }

    #[test]
    fn plan_destinations_reports_fallbacks_and_free_space() {
        let items = [
            sized_dir(r"D:\Games\Foo", 100),
            sized_dir(r"D:\Games\Foo\sub", 40),
            sized_dir(r"E:\Foo", 30),
            item(LeftoverKind::File { path: PathBuf::from(r"F:\foo.cfg"), size: Some(5) }),
            sized_dir(r"C:\Apps\Foo", 7),
            key_item(r"SOFTWARE\Foo"),
        ];
        let not_ntfs = |d: char| !matches!(d, 'E' | 'F');
        let plenty = |_: char| Some(1_000);
        let plan = plan_destinations(&items, &not_ntfs, &plenty);
        assert_eq!(plan.per_drive.into_iter().collect::<Vec<_>>(), vec![('C', 42), ('D', 100)], "sub counted with its parent");
        assert_eq!(plan.fallbacks, vec![('E', 30), ('F', 5)]);
        assert!(plan.insufficient.is_empty());

        // C: has room for its own 7 bytes (a rename) but not for the 35 copied there.
        let tight = |d: char| if d == 'C' { Some(20) } else { Some(0) };
        assert_eq!(plan_destinations(&items, &not_ntfs, &tight).insufficient, vec!['C']);
        // Unknown free space is not a reason to block.
        assert!(plan_destinations(&items, &not_ntfs, &|_| None).insufficient.is_empty());
        let all_ok = plan_destinations(&items, &|_| true, &|_| Some(0));
        assert!(all_ok.fallbacks.is_empty() && all_ok.insufficient.is_empty(), "same-drive moves need no space");
    }

    #[test]
    #[ignore]
    fn live_free_bytes_of_system_drive() {
        let free = live_free_bytes('C');
        println!("C: free {free:?}");
        assert!(free.is_some_and(|f| f > 0));
        assert_eq!(live_free_bytes('1'), None);
    }
}
