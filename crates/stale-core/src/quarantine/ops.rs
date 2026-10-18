//! Quarantine operations: moving chosen leftovers aside, restoring them, purging them for good.
//!
//! A quarantine `<id>` keeps its manifest, `.reg` exports and task XML copies in
//! `<manifest dir>\<id>\`, and the moved files in `<drive>:\Stale Uninstaller\quarantine\<id>\`.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

use super::fsmove::{stored_path_for, FileMover, MoveError, RealFileMover};
use super::store::{list_manifests, new_id, save_manifest, valid_id};
use super::{
    ExportedKey, Failure, MovedItem, QuarantineId, QuarantineManifest, RemovedService, RemovedTask, RemovedValue,
};
use crate::fsys::{FsReader, RealFs};
use crate::leftovers::{Leftover, LeftoverKind};
use crate::model::{Hive, RegKeyPath};
use crate::pathutil::{drive_letter, norm};
use crate::proc::{run_hidden, system32_tool};
use crate::registry::{key, RegistryReader, WinRegistry};
use crate::regwrite::{RegistryWriter, WinRegistryWriter};

const SERVICES_KEY: &str = r"SYSTEM\CurrentControlSet\Services";

/// A system command run on a service or task name.
pub type NameCommand = Box<dyn Fn(&str) -> Result<(), String> + Send + Sync>;
/// Re-creates a task `name` from an XML file.
pub type CreateTaskCommand = Box<dyn Fn(&str, &Path) -> Result<(), String> + Send + Sync>;

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
}

fn tool(name: &str, args: &[&OsStr]) -> Result<(), String> {
    run_hidden(&system32_tool(name), args)
}

/// Task Scheduler path of a task named relative to the Tasks folder.
fn task_path(name: &str) -> String {
    if name.starts_with('\\') { name.to_string() } else { format!(r"\{name}") }
}

/// `sc stop` (a service that is not running, error 1062, is fine) then `sc delete`.
fn live_stop_service(name: &str) -> Result<(), String> {
    match tool("sc.exe", &["stop".as_ref(), name.as_ref()]) {
        Err(e) if !e.contains("1062") => return Err(e),
        _ => {}
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
        }
    }
}

#[derive(Clone, Debug)]
pub enum QuarantineProgress {
    /// About to handle item `done` (0-based) of `total`.
    Item { done: usize, total: usize, what: String },
    Finished(QuarantineManifest),
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
}

impl Run<'_> {
    fn step(&mut self, what: &str) {
        (self.progress)(QuarantineProgress::Item { done: self.done, total: self.total, what: what.to_string() });
        self.done += 1;
    }

    fn fail(&mut self, what: impl Into<String>, error: impl Into<String>) {
        self.m.failures.push(Failure { what: what.into(), error: error.into() });
    }

    fn save(&mut self) {
        if let Err(e) = save_manifest(&self.ops.manifest_dir, &self.m) {
            self.fail("manifest", e);
        }
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
            let k = key(Hive::LocalMachine, &format!(r"{SERVICES_KEY}\{name}"));
            if let Err(e) = self.ops.regw.export_key(&k, &file) {
                self.fail(name, e);
                continue;
            }
            match (self.ops.stop_service)(name) {
                Ok(()) => self.m.services.push(RemovedService { name: name.clone(), reg_file: file }),
                Err(e) => self.fail(name, e),
            }
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
            match (self.ops.delete_task)(name) {
                Ok(()) => self.m.tasks.push(RemovedTask { name: name.clone(), xml_backup: backup, original_xml: xml.clone() }),
                Err(e) => self.fail(name, e),
            }
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
                match self.ops.regw.delete_value(k, name) {
                    Ok(()) => self.m.values.push(RemovedValue { key: k.clone(), name: name.to_string(), reg_file: file.clone() }),
                    Err(e) => self.fail(what, e),
                }
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
            match self.ops.regw.delete_key(key) {
                Ok(()) => self.m.keys.push(ExportedKey { key: key.clone(), reg_file: file }),
                Err(e) => self.fail(what, e),
            }
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

    /// The payload root for `drive`; on first use its parent and itself are created and locked down.
    fn payload_root(&mut self, drive: char) -> Result<PathBuf, String> {
        let ops = self.ops;
        self.roots
            .entry(drive)
            .or_insert_with(|| {
                let root = (ops.payload_root_for)(drive);
                if let Some(parent) = root.parent() {
                    ops.mover.ensure_payload_root(parent)?;
                }
                ops.mover.ensure_payload_root(&root)?;
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
        match self.ops.mover.move_path(&original, &stored) {
            Ok(()) => self.m.moved.push(MovedItem { original, stored, is_dir, size }),
            Err(MoveError::PartialMove { stored, detail }) => {
                self.m.moved.push(MovedItem { original, stored, is_dir, size });
                self.fail(what, format!("partial move: {detail}"));
            }
            Err(e) => self.fail(what, e.to_string()),
        }
    }
}

/// Quarantines `items` in the order services, tasks, registry values, registry keys, shortcuts,
/// files/folders. Every failure is recorded in the manifest and the rest continues; the manifest
/// is saved before anything is touched and after each group.
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
    };
    let mut run =
        Run { ops, m, folder: ops.manifest_dir.join(&id.0), done: 0, total: items.len(), progress, roots: BTreeMap::new() };
    if let Err(e) = save_manifest(&ops.manifest_dir, &run.m) {
        // Without a manifest nothing could be restored, so nothing is touched.
        for it in items {
            run.fail(describe(&it.kind), format!("manifest could not be saved: {e}"));
        }
        progress(QuarantineProgress::Finished(run.m.clone()));
        return run.m;
    }
    run.services(items);
    run.save();
    run.tasks(items);
    run.save();
    run.values(items);
    run.save();
    run.keys(items);
    run.save();
    run.shortcuts(items);
    run.save();
    run.files(items);
    run.save();
    progress(QuarantineProgress::Finished(run.m.clone()));
    run.m
}

/// `<payload root of each drive holding moved items>\<id>`.
fn payload_dirs(ops: &QuarantineOps, m: &QuarantineManifest) -> BTreeSet<PathBuf> {
    m.moved.iter().filter_map(|i| drive_letter(&i.stored)).map(|d| (ops.payload_root_for)(d).join(&m.id.0)).collect()
}

/// Moves files back, last moved first; returns the items that stay in quarantine.
fn restore_moved(ops: &QuarantineOps, id: &QuarantineId, moved: &[MovedItem], failures: &mut Vec<Failure>) -> Vec<MovedItem> {
    let mut kept = Vec::new();
    for item in moved.iter().rev() {
        let what = item.original.display().to_string();
        let mut fail = |error: String| failures.push(Failure { what: what.clone(), error });
        if ops.mover.exists(&item.original) {
            fail("occupied".to_string());
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

fn restore_keys(ops: &QuarantineOps, keys: &[ExportedKey], failures: &mut Vec<Failure>) -> Vec<ExportedKey> {
    let mut kept = Vec::new();
    for k in keys.iter().rev() {
        if let Err(error) = ops.regw.import_file(&k.reg_file) {
            failures.push(Failure { what: key_text(&k.key), error });
            kept.push(k.clone());
        }
    }
    kept.reverse();
    kept
}

/// Imports each value export once (it re-creates the whole key as it was).
fn restore_values(ops: &QuarantineOps, values: &[RemovedValue], failures: &mut Vec<Failure>) -> Vec<RemovedValue> {
    let mut files: Vec<&Path> = Vec::new();
    for v in values.iter().rev() {
        if !files.contains(&v.reg_file.as_path()) {
            files.push(&v.reg_file);
        }
    }
    let mut failed: Vec<&Path> = Vec::new();
    for file in files {
        if let Err(error) = ops.regw.import_file(file) {
            for v in values.iter().filter(|v| v.reg_file == file) {
                failures.push(Failure { what: format!(r"{}\{}", key_text(&v.key), v.name), error: error.clone() });
            }
            failed.push(file);
        }
    }
    values.iter().filter(|v| failed.contains(&v.reg_file.as_path())).cloned().collect()
}

fn restore_tasks(ops: &QuarantineOps, tasks: &[RemovedTask], failures: &mut Vec<Failure>) -> Vec<RemovedTask> {
    let mut kept = Vec::new();
    for t in tasks.iter().rev() {
        if let Err(error) = (ops.create_task)(&t.name, &t.xml_backup) {
            failures.push(Failure { what: t.name.clone(), error });
            kept.push(t.clone());
        }
    }
    kept.reverse();
    kept
}

/// Re-imports service keys; a restored service only starts after a reboot (noted in `notes`).
fn restore_services(
    ops: &QuarantineOps,
    services: &[RemovedService],
    failures: &mut Vec<Failure>,
    notes: &mut Vec<String>,
) -> Vec<RemovedService> {
    let mut kept = Vec::new();
    for s in services.iter().rev() {
        match ops.regw.import_file(&s.reg_file) {
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

/// Puts a quarantine back: files/shortcuts, then registry keys/values, tasks, services. Items that
/// cannot go back stay in the (re-saved) manifest; once nothing is left the manifest folder and the
/// payload folders are deleted. A full restore drops the manifest, so callers that want to say
/// "restored services need a reboot" look at `m.services`.
pub fn restore(ops: &QuarantineOps, m: &QuarantineManifest) -> RestoreOutcome {
    if !valid_id(&m.id) {
        return RestoreOutcome::Partial(vec![Failure { what: m.id.0.clone(), error: "invalid quarantine id".to_string() }]);
    }
    let mut failures = Vec::new();
    let mut left = m.clone();
    left.moved = restore_moved(ops, &m.id, &m.moved, &mut failures);
    left.keys = restore_keys(ops, &m.keys, &mut failures);
    left.values = restore_values(ops, &m.values, &mut failures);
    left.tasks = restore_tasks(ops, &m.tasks, &mut failures);
    left.services = restore_services(ops, &m.services, &mut failures, &mut left.notes);

    let nothing_left = left.moved.is_empty()
        && left.keys.is_empty()
        && left.values.is_empty()
        && left.tasks.is_empty()
        && left.services.is_empty();
    if nothing_left {
        for dir in payload_dirs(ops, m) {
            if let Err(e) = ops.mover.remove_all(&dir) {
                log::warn!("quarantine {}: removing {}: {e}", m.id.0, dir.display());
            }
        }
        if let Err(e) = remove_manifest_folder(ops, &m.id) {
            log::warn!("quarantine {}: {e}", m.id.0);
        }
    } else if let Err(error) = save_manifest(&ops.manifest_dir, &left) {
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

/// Deletes the payload folders and then the manifest folder for good. If a payload folder cannot
/// be deleted the manifest is kept, so nothing is orphaned and the purge can be retried.
pub fn purge(ops: &QuarantineOps, m: &QuarantineManifest) -> Result<(), String> {
    if !valid_id(&m.id) {
        return Err(format!("invalid quarantine id {:?}", m.id.0));
    }
    let errors: Vec<String> = payload_dirs(ops, m).iter().filter_map(|d| ops.mover.remove_all(d).err()).collect();
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    remove_manifest_folder(ops, &m.id)
}

/// Purges every quarantine created more than `days` days before `now`; `0` turns this off.
/// Returns the purged ids.
pub fn auto_purge(ops: &QuarantineOps, days: u32, now: DateTime<Utc>) -> Vec<QuarantineId> {
    if days == 0 {
        return Vec::new();
    }
    let cutoff = now - Duration::days(i64::from(days));
    list_manifests(&ops.manifest_dir)
        .into_iter()
        .filter(|m| m.created < cutoff)
        .filter_map(|m| match purge(ops, &m) {
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
                r"ensure D:\Stale Uninstaller".to_string(),
                r"ensure D:\Stale Uninstaller\quarantine".to_string(),
                format!(r"move D:\Games\Foo -> {}", d.display()),
                r"ensure C:\Stale Uninstaller".to_string(),
                r"ensure C:\Stale Uninstaller\quarantine".to_string(),
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
                format!(r"move {} -> C:\Apps\Foo", stored('C', &m, r"C\Apps\Foo").display()),
                format!(r"move {} -> C:\Users\A\Desktop\Foo.lnk", stored('C', &m, r"C\Users\A\Desktop\Foo.lnk").display()),
                format!(r"remove C:\Stale Uninstaller\quarantine\{}", m.id.0),
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
                format!(r"remove C:\Stale Uninstaller\quarantine\{}", m.id.0),
                format!(r"remove D:\Stale Uninstaller\quarantine\{}", m.id.0),
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
}
