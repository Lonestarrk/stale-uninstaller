//! Headless end-to-end test of the uninstall flow against a real per-user (HKCU) program:
//! `cargo run -p stale-core --features e2e-overrides --example e2e -- <name substring> [--vendor-silent] [--plan-only]`.
//!
//! Steps: pick the program, scan leftovers, run the vendor uninstaller silently (NSIS `/S`, only
//! with `--vendor-silent`; else Forced mode), scan again, quarantine the default-checked items,
//! verify, restore, verify, quarantine again, purge, verify. Prints PASS/FAIL per step and exits
//! with 1 on any FAIL. Everything destructive goes through the core APIs.
//!
//! THIS REMOVES THE PROGRAM. Unelevated, set the test-only overrides `STALE_E2E_DATA_DIR=<folder>`
//! and `STALE_E2E_TRUST_ALL=1` (manifests are otherwise kept in the locked
//! `%ProgramData%\Stale Uninstaller`, which only an elevated process can use). They exist only
//! with the `e2e-overrides` feature, which this example requires.

use std::path::PathBuf;
use std::process::ExitCode;

use chrono::Utc;
use stale_core::fsys::RealFs;
use stale_core::leftovers::{scan_leftovers, Confidence, Leftover, LeftoverKind, LeftoverReport, LeftoverSources, ProgramHint};
use stale_core::model::{Hive, RegKeyPath, Scope};
use stale_core::pathutil::{drive_letter, is_shared_root};
use stale_core::programs::enumerate_programs;
use stale_core::quarantine::{purge, quarantine, restore, QuarantineManifest, QuarantineOps, QuarantineProgress, RestoreOutcome};
use stale_core::registry::{RegistryReader, WinRegistry};
use stale_core::uninstall::{still_installed, uninstall_command, JobObjectRunner, ProcessRunner};

/// PASS/FAIL per step, printed at the end.
struct Steps(Vec<(String, bool, String)>);

impl Steps {
    fn record(&mut self, step: &str, ok: bool, detail: impl Into<String>) {
        let detail = detail.into();
        println!("  => {step}: {} {detail}", if ok { "PASS" } else { "FAIL" });
        self.0.push((step.to_string(), ok, detail));
    }
}

fn hive(h: Hive) -> &'static str {
    match h {
        Hive::LocalMachine => "HKLM",
        Hive::CurrentUser => "HKCU",
        Hive::Users => "HKU",
    }
}

fn key_text(k: &RegKeyPath) -> String {
    format!(r"{}\{}", hive(k.hive), k.path)
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

fn describe(kind: &LeftoverKind) -> (&'static str, String) {
    match kind {
        LeftoverKind::Dir { path, .. } => ("Dir", path.display().to_string()),
        LeftoverKind::File { path, .. } => ("File", path.display().to_string()),
        LeftoverKind::RegKey { key } => ("RegKey", key_text(key)),
        LeftoverKind::RegValue { key, name } => ("RegValue", format!(r"{}\[{name}]", key_text(key))),
        LeftoverKind::Shortcut { lnk, target } => ("Shortcut", format!("{} -> {}", lnk.display(), target.display())),
        LeftoverKind::Service { name, image } => ("Service", format!("{name} ({})", image.display())),
        LeftoverKind::ScheduledTask { name, command, .. } => ("Task", format!("{name} ({})", command.display())),
    }
}

fn print_report(report: &LeftoverReport) {
    let mut counts = [0usize; 3];
    for item in &report.items {
        let label = match item.confidence {
            Confidence::High => "High",
            Confidence::Medium => "Medium",
            Confidence::Low => "Low",
        };
        counts[item.confidence as usize] += 1;
        let (kind, what) = describe(&item.kind);
        let size = item.size().map_or("-".to_string(), mb);
        let checked = if item.default_checked() { "[x]" } else { "[ ]" };
        println!("  {checked} {label:<6} {kind:<8} {what} | {size} | {}", item.note.as_deref().unwrap_or(""));
    }
    println!("  --- {} High, {} Medium, {} Low", counts[0], counts[1], counts[2]);
}

fn value_exists(reg: &dyn RegistryReader, key: &RegKeyPath, name: &str) -> bool {
    reg.values(key).is_ok_and(|vals| vals.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)))
}

/// Whether the thing a leftover names is still there (services and tasks are assumed to be).
fn still_exists(item: &Leftover) -> bool {
    match &item.kind {
        LeftoverKind::Dir { path, .. } | LeftoverKind::File { path, .. } => path.symlink_metadata().is_ok(),
        LeftoverKind::Shortcut { lnk, .. } => lnk.symlink_metadata().is_ok(),
        LeftoverKind::RegKey { key } => WinRegistry.key_exists(key),
        LeftoverKind::RegValue { key, name } => value_exists(&WinRegistry, key, name),
        LeftoverKind::Service { .. } | LeftoverKind::ScheduledTask { .. } => true,
    }
}

/// Why this test refuses to touch `item`: only per-user files and HKCU keys are in scope.
fn out_of_scope(item: &Leftover) -> Option<String> {
    match &item.kind {
        LeftoverKind::RegKey { key } | LeftoverKind::RegValue { key, .. } if key.hive != Hive::CurrentUser => {
            Some("not under HKCU".to_string())
        }
        LeftoverKind::Service { .. } | LeftoverKind::ScheduledTask { .. } => Some("service/task".to_string()),
        LeftoverKind::Dir { path, .. } | LeftoverKind::File { path, .. } if is_shared_root(path) => {
            Some("shared root".to_string())
        }
        _ => None,
    }
}

fn print_manifest(m: &QuarantineManifest, ops: &QuarantineOps) {
    println!("  manifest {} for {:?}", m.id.0, m.program_name);
    println!("  manifest path: {}", ops.manifest_dir.join(&m.id.0).join("manifest.json").display());
    println!("  moved ({}):", m.moved.len());
    for i in &m.moved {
        println!("    {} -> {} ({}, {})", i.original.display(), i.stored.display(), if i.is_dir { "dir" } else { "file" }, mb(i.size));
    }
    println!("  keys ({}):", m.keys.len());
    for k in &m.keys {
        println!("    {} -> {}", key_text(&k.key), k.reg_file.display());
    }
    println!("  values ({}):", m.values.len());
    for v in &m.values {
        println!("    {}\\[{}] -> {}", key_text(&v.key), v.name, v.reg_file.display());
    }
    println!("  services: {}, tasks: {}, pending: {}", m.services.len(), m.tasks.len(), m.pending.len());
    println!("  failures ({}):", m.failures.len());
    for f in &m.failures {
        println!("    {}: {}", f.what, f.error);
    }
    for (d, bytes) in m.size_per_drive() {
        println!("  size on {d}: {}", mb(bytes));
    }
}

fn progress(p: QuarantineProgress) {
    match p {
        QuarantineProgress::Item { done, total, what } => println!("  [{}/{total}] {what}", done + 1),
        QuarantineProgress::Finished(m) => println!("  finished: {} moved, {} failures", m.moved.len(), m.failures.len()),
    }
}

/// Checks a fresh quarantine: originals gone, stored copies present, keys and values gone.
fn verify_quarantined(m: &QuarantineManifest) -> Vec<String> {
    let mut problems = Vec::new();
    for i in &m.moved {
        if i.original.symlink_metadata().is_ok() {
            problems.push(format!("original still exists: {}", i.original.display()));
        }
        if i.stored.symlink_metadata().is_err() {
            problems.push(format!("stored copy missing: {}", i.stored.display()));
        }
    }
    for k in &m.keys {
        if WinRegistry.key_exists(&k.key) {
            problems.push(format!("key still exists: {}", key_text(&k.key)));
        }
    }
    for v in &m.values {
        if value_exists(&WinRegistry, &v.key, &v.name) {
            problems.push(format!("value still exists: {}\\[{}]", key_text(&v.key), v.name));
        }
    }
    problems
}

/// Every `<payload root>\<id>` folder the manifest's stored items live in.
fn payload_dirs(ops: &QuarantineOps, m: &QuarantineManifest) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> =
        m.moved.iter().filter_map(|i| drive_letter(&i.stored)).map(|d| (ops.payload_root_for)(d).join(&m.id.0)).collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let vendor_silent = args.iter().any(|a| a == "--vendor-silent");
    let plan_only = args.iter().any(|a| a == "--plan-only");
    let Some(needle) = args.iter().find(|a| !a.starts_with("--")).map(|a| a.to_lowercase()) else {
        eprintln!("usage: e2e <program-name-substring> [--vendor-silent] [--plan-only]");
        return ExitCode::from(2);
    };
    let mut steps = Steps(Vec::new());
    // `paths::quarantine_manifest_dir()` creates a missing folder locked to Administrators and
    // SYSTEM, which an unelevated run cannot write to: with the test data dir, create it plainly first.
    if let Some(dir) = std::env::var_os(stale_core::paths::E2E_DATA_DIR_ENV).filter(|d| !d.is_empty()) {
        let _ = std::fs::create_dir_all(PathBuf::from(dir).join("quarantine"));
    }
    let ops = QuarantineOps::live();
    println!("data dir: {}", stale_core::paths::data_dir().display());
    println!("manifest dir: {}", ops.manifest_dir.display());
    println!("payload root C: {}", (ops.payload_root_for)('C').display());

    // 1. Pick the program.
    println!("\n== 1. enumerate programs");
    let all = enumerate_programs(&WinRegistry);
    let Some(p) = all.iter().find(|p| !p.hidden && p.name.to_lowercase().contains(&needle)).cloned() else {
        steps.record("1 enumerate", false, format!("no visible program matches {needle:?}"));
        return finish(&steps);
    };
    println!("  name: {}", p.name);
    println!("  key: {} (scope {:?})", key_text(&p.key), p.scope);
    println!("  install_location: {:?}", p.install_location);
    println!("  uninstall_string: {:?}", p.uninstall_string);
    if p.scope != Scope::CurrentUser || p.key.hive != Hive::CurrentUser {
        steps.record("1 enumerate", false, "refused: not a per-user (HKCU) program");
        return finish(&steps);
    }
    if p.install_location.as_deref().is_some_and(is_shared_root) {
        steps.record("1 enumerate", false, "refused: install_location is a shared root");
        return finish(&steps);
    }
    steps.record("1 enumerate", true, &p.name);

    // 2. Leftovers before uninstalling.
    println!("\n== 2. leftover scan (before uninstall)");
    let hint = ProgramHint::from_program(&p).with_other_programs(&all, &p.id);
    let before = scan_leftovers(&LeftoverSources::live(), &hint);
    print_report(&before);
    steps.record("2 scan before", !before.items.is_empty(), format!("{} items", before.items.len()));

    let command = uninstall_command(&p, &|x| x.exists()).map(|mut c| {
        c.push("/S".to_string());
        c
    });
    println!("\n== plan");
    if vendor_silent {
        println!("  3. run vendor uninstaller: {command:?}");
    } else {
        println!("  3. Forced mode: vendor uninstaller skipped");
    }
    println!("  4. scan again; 5. quarantine the default-checked items; 6. verify;");
    println!("  7. restore + verify; 8. quarantine again + purge + verify");
    let refused: Vec<String> = before
        .items
        .iter()
        .filter(|i| i.default_checked())
        .filter_map(|i| out_of_scope(i).map(|why| format!("{} ({why})", describe(&i.kind).1)))
        .collect();
    if !refused.is_empty() {
        for r in &refused {
            println!("  out of scope: {r}");
        }
        steps.record("plan", false, "refused: default-checked items outside per-user scope");
        return finish(&steps);
    }
    if plan_only {
        println!("  --plan-only: stopping before anything is touched");
        return finish(&steps);
    }
    println!("  starting in 3 s ...");
    std::thread::sleep(std::time::Duration::from_secs(3));

    // 3. Vendor uninstaller.
    println!("\n== 3. vendor uninstaller");
    if vendor_silent {
        match &command {
            None => steps.record("3 vendor", false, "no uninstall command"),
            Some(cmd) => {
                let result = JobObjectRunner::default().run_and_wait_tree(cmd);
                let gone = !still_installed(&WinRegistry, &RealFs, &p);
                let verdict = if gone { "Uninstalled" } else { "NotUninstalled" };
                println!("  exit: {result:?}; {verdict}");
                steps.record("3 vendor", result.is_ok() && gone, format!("{verdict}, exit {result:?}"));
            }
        }
    } else {
        println!("  Forced mode: skipped");
        steps.record("3 vendor", true, "skipped (Forced)");
    }

    // 4. Leftovers after.
    println!("\n== 4. leftover scan (after uninstall)");
    let after = scan_leftovers(&LeftoverSources::live(), &hint);
    print_report(&after);
    let checked: Vec<Leftover> = after.items.iter().filter(|i| i.default_checked()).cloned().collect();
    if let Some(bad) = checked.iter().find_map(|i| out_of_scope(i).map(|why| format!("{} ({why})", describe(&i.kind).1))) {
        steps.record("4 scan after", false, format!("refused: {bad}"));
        return finish(&steps);
    }
    steps.record("4 scan after", !checked.is_empty(), format!("{} items, {} default-checked", after.items.len(), checked.len()));
    if checked.is_empty() {
        return finish(&steps);
    }

    // 5. Quarantine.
    println!("\n== 5. quarantine");
    let m = quarantine(&ops, &p.name, &checked, Utc::now(), &progress);
    print_manifest(&m, &ops);
    let quarantined = m.failures.is_empty() && m.pending.is_empty() && !(m.moved.is_empty() && m.keys.is_empty());
    steps.record("5 quarantine", quarantined, format!("{} moved, {} keys, {} values, {} failures", m.moved.len(), m.keys.len(), m.values.len(), m.failures.len()));
    if m.moved.is_empty() && m.keys.is_empty() && m.values.is_empty() {
        return finish(&steps);
    }

    // 6. Verify on disk.
    println!("\n== 6. verify quarantine");
    let problems = verify_quarantined(&m);
    for pr in &problems {
        println!("  {pr}");
    }
    steps.record("6 verify quarantine", problems.is_empty(), format!("{} problems", problems.len()));

    // 7. Restore.
    println!("\n== 7. restore");
    let outcome = restore(&ops, &m);
    println!("  outcome: {outcome:?}");
    let mut problems = Vec::new();
    for i in &m.moved {
        if i.original.symlink_metadata().is_err() {
            problems.push(format!("not restored: {}", i.original.display()));
        }
    }
    for k in &m.keys {
        if !WinRegistry.key_exists(&k.key) {
            problems.push(format!("key not restored: {}", key_text(&k.key)));
        }
    }
    for v in &m.values {
        if !value_exists(&WinRegistry, &v.key, &v.name) {
            problems.push(format!("value not restored: {}\\[{}]", key_text(&v.key), v.name));
        }
    }
    let folder = ops.manifest_dir.join(&m.id.0);
    if folder.exists() {
        problems.push(format!("manifest folder still there: {}", folder.display()));
    }
    for d in payload_dirs(&ops, &m) {
        if d.exists() {
            problems.push(format!("payload folder still there: {}", d.display()));
        }
    }
    for pr in &problems {
        println!("  {pr}");
    }
    steps.record("7 restore", outcome == RestoreOutcome::Restored && problems.is_empty(), format!("{} problems", problems.len()));

    // 8. Quarantine again, then purge.
    println!("\n== 8. quarantine again + purge");
    let again: Vec<Leftover> = checked.iter().filter(|i| still_exists(i)).cloned().collect();
    println!("  {} of {} items still exist", again.len(), checked.len());
    let m2 = quarantine(&ops, &p.name, &again, Utc::now(), &progress);
    print_manifest(&m2, &ops);
    let mut problems = verify_quarantined(&m2);
    problems.extend(m2.failures.iter().map(|f| format!("quarantine failure: {}: {}", f.what, f.error)));
    let payload = payload_dirs(&ops, &m2);
    let purged = purge(&ops, &m2);
    println!("  purge: {purged:?}");
    let folder = ops.manifest_dir.join(&m2.id.0);
    if folder.exists() {
        problems.push(format!("manifest folder still there: {}", folder.display()));
    }
    for d in &payload {
        if d.exists() {
            problems.push(format!("payload folder still there: {}", d.display()));
        }
    }
    for i in &m2.moved {
        if i.original.symlink_metadata().is_ok() {
            problems.push(format!("original back after purge: {}", i.original.display()));
        }
    }
    for pr in &problems {
        println!("  {pr}");
    }
    steps.record("8 quarantine+purge", purged.is_ok() && problems.is_empty(), format!("{} problems", problems.len()));
    finish(&steps)
}

fn finish(steps: &Steps) -> ExitCode {
    println!("\n== result");
    for (step, ok, detail) in &steps.0 {
        println!("{} {step}: {detail}", if *ok { "PASS" } else { "FAIL" });
    }
    if steps.0.iter().all(|(_, ok, _)| *ok) { ExitCode::SUCCESS } else { ExitCode::from(1) }
}
