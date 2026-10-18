//! Manual check of the whole core: `cargo run -p stale-core --example dump` (elevated for BAM/Prefetch).
//! Prints `drive | name | last used (source) | size`, never-seen first, then oldest first.
//! `dump provenance <name>` prints the signature, version info, install date and source.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;

use stale_core::ghosts::{scan_ghosts, Evidence, GhostSources};
use stale_core::leftovers::{scan_leftovers, Confidence, LeftoverKind, LeftoverSources, ProgramHint};
use stale_core::programs::enumerate_programs;
use stale_core::registry::WinRegistry;
use stale_core::pathutil::exe_under;
use stale_core::scan::{gather_usage, run_scan, ScanEvent, ScanSources};
use stale_core::provenance::{provenance, WinProbe};
use stale_core::usage::matching::{explain_match, match_usage, program_main_exes, usage_install_dir};

/// `dump leftovers <name>`: lists what is left of the first visible program whose name contains
/// `<name>`. Nothing is deleted.
fn dump_leftovers(name: &str) {
    let needle = name.to_lowercase();
    let programs = enumerate_programs(&WinRegistry);
    let Some(program) = programs.iter().find(|p| !p.hidden && p.name.to_lowercase().contains(&needle)) else {
        println!("no visible program matches {name:?}");
        return;
    };
    println!("program: {} ({:?})", program.name, program.install_location);
    let hint = ProgramHint::from_program(program).with_other_programs(&programs, &program.id);
    let report = scan_leftovers(&LeftoverSources::live(), &hint);
    let mut counts = [0usize; 3];
    for item in &report.items {
        let (idx, label) = match item.confidence {
            Confidence::High => (0, "High"),
            Confidence::Medium => (1, "Medium"),
            Confidence::Low => (2, "Low"),
        };
        counts[idx] += 1;
        let (kind, what) = match &item.kind {
            LeftoverKind::Dir { path, .. } => ("Dir", path.display().to_string()),
            LeftoverKind::File { path, .. } => ("File", path.display().to_string()),
            LeftoverKind::RegKey { key } => ("RegKey", format!(r"{:?}\{}", key.hive, key.path)),
            LeftoverKind::RegValue { key, name } => ("RegValue", format!(r"{:?}\{} [{name}]", key.hive, key.path)),
            LeftoverKind::Shortcut { lnk, .. } => ("Shortcut", lnk.display().to_string()),
            LeftoverKind::Service { name, .. } => ("Service", name.clone()),
            LeftoverKind::ScheduledTask { name, .. } => ("ScheduledTask", name.clone()),
        };
        let size = item.size().map_or("-".to_string(), |b| format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)));
        println!("{label} | {kind} | {what} | {size} | {}", item.note.as_deref().unwrap_or(""));
    }
    println!("--- {} High, {} Medium, {} Low", counts[0], counts[1], counts[2]);
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

/// `dump ghosts`: lists programs that are gone from the uninstall list but left traces, with
/// their evidence. Nothing is deleted.
fn dump_ghosts() {
    let (tx, rx) = channel();
    let scanner = std::thread::spawn(move || run_scan(ScanSources::live(), tx));
    let (mut programs, mut unmatched) = (Vec::new(), Vec::new());
    for event in rx {
        match event {
            ScanEvent::Programs(p) => programs = p,
            ScanEvent::UnmatchedHits(h) => unmatched = h,
            _ => {}
        }
    }
    scanner.join().expect("scan thread panicked");
    let ghosts = scan_ghosts(&GhostSources::live(programs, unmatched));
    for g in &ghosts {
        let last = g.last_used.map_or("never".to_string(), |w| w.format("%Y-%m-%d").to_string());
        let publisher = g.publisher.as_deref().map_or(String::new(), |p| format!(" [{p}]"));
        let reasons = if g.reasons.is_empty() { String::new() } else { format!(" ({})", g.reasons.join(", ")) };
        println!("{}{publisher} | {:?}{reasons} | {} | {last}", g.name, g.assessment, mb(g.total_size));
        for e in &g.evidence {
            let line = match e {
                Evidence::Folder { path, size, root_kind, .. } => {
                    format!("Folder {:?} {} ({})", root_kind, path.display(), size.map_or("-".to_string(), mb))
                }
                Evidence::RegKeyMissingPath { key, value_name, path, .. } => {
                    format!(r"RegKeyMissingPath {:?}\{} [{value_name}] -> {}", key.hive, key.path, path.display())
                }
                Evidence::RegKeyOrphan { key, .. } => format!(r"RegKeyOrphan {:?}\{}", key.hive, key.path),
                Evidence::DanglingRef { kind, location, target, value_name, .. } => {
                    let value = value_name.as_deref().map_or(String::new(), |v| format!(" [{v}]"));
                    format!("{kind:?} {location}{value} -> {}", target.display())
                }
                Evidence::UsageTrace { exe, last_run, source } => {
                    format!("UsageTrace {source:?} {} at {}", exe.display(), last_run.format("%Y-%m-%d"))
                }
            };
            println!("    {line}");
        }
    }
    println!("--- {} ghosts", ghosts.len());
}

/// Upper-cased names of the `.exe` files in `dir` and up to three levels below it.
fn exe_names_under(dir: &Path) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let path = entry.path();
            if ft.is_dir() {
                if depth < 3 {
                    stack.push((path, depth + 1));
                }
            } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) {
                names.insert(entry.file_name().to_string_lossy().to_uppercase());
            }
        }
    }
    names
}

/// `dump usage <name>`: for every program whose name contains `<name>`, its install folders,
/// main exes, and every usage hit related to it with what matching did with it.
fn dump_usage(name: &str) {
    let needle = name.to_lowercase();
    let src = ScanSources::live();
    let programs = enumerate_programs(&*src.reg);
    let inputs = gather_usage(&src, &programs);
    for w in &inputs.warnings {
        println!("warning: {w}");
    }
    let mut per_source: HashMap<String, usize> = HashMap::new();
    for h in &inputs.hits {
        *per_source.entry(format!("{:?}", h.source)).or_default() += 1;
    }
    let mut counts: Vec<_> = per_source.into_iter().collect();
    counts.sort();
    println!("hits per source: {counts:?}");
    let input = inputs.match_input(&programs, &src.shortcut_targets);
    for p in programs.iter().filter(|p| p.name.to_lowercase().contains(&needle)) {
        println!();
        println!("=== {} [{}]{}", p.name, p.id.0, if p.hidden { " (hidden)" } else { "" });
        println!("install_location: {:?}", p.install_location);
        println!("display_icon:     {:?}", p.display_icon);
        println!("uninstall_string: {:?}", p.uninstall_string);
        let effective: Option<PathBuf> = usage_install_dir(p, &src.shortcut_targets);
        println!("effective dir:    {effective:?}");
        let roots = inputs.root_exes.get(&p.id).map(Vec::as_slice).unwrap_or_default();
        let args = inputs.arg_exes.get(&p.id).map(Vec::as_slice).unwrap_or_default();
        let (mains, rule) = program_main_exes(p, &src.shortcut_targets, roots, args, &inputs.service_exes);
        println!("main exes ({rule:?}): {mains:?}");
        let dirs: Vec<&Path> = p.install_location.iter().chain(effective.iter()).map(PathBuf::as_path).collect();
        let names: HashSet<String> = dirs.iter().flat_map(|d| exe_names_under(d)).collect();
        println!("exe names under install dirs: {}", names.len());
        let mut shown = 0;
        for e in explain_match(&input, &p.id) {
            let file = e.hit.exe_path.file_name().map(|n| n.to_string_lossy().to_uppercase());
            let related = dirs.iter().any(|d| exe_under(&e.hit.exe_path, d))
                || file.is_some_and(|f| names.contains(&f))
                || !(e.outcome == "no-candidate" || e.outcome == "drive-less-unmatched" || e.outcome.starts_with("assigned-to:"));
            if !related {
                continue;
            }
            shown += 1;
            let runs = e.hit.run_count.map_or(String::new(), |c| format!(" x{c}"));
            println!(
                "  {:?} {} {}{runs} -> {}",
                e.hit.source,
                e.hit.last_run.format("%Y-%m-%d %H:%M"),
                e.hit.exe_path.display(),
                e.outcome
            );
        }
        println!("--- {shown} related hits");
    }
}

/// `dump provenance <name>`: for every program whose name contains `<name>`, its main exe and
/// what the details panel shows about where it came from.
fn dump_provenance(name: &str) {
    let needle = name.to_lowercase();
    let src = ScanSources::live();
    let programs = enumerate_programs(&*src.reg);
    let inputs = gather_usage(&src, &programs);
    let result = match_usage(&inputs.match_input(&programs, &src.shortcut_targets));
    for p in programs.iter().filter(|p| p.name.to_lowercase().contains(&needle)) {
        let main_exe = result.by_program.get(&p.id).and_then(|u| u.main_exe.clone());
        let started = std::time::Instant::now();
        let prov = provenance(p, main_exe.as_deref(), &WinProbe, &WinRegistry);
        println!();
        println!("=== {} [{}]{}", p.name, p.id.0, if p.hidden { " (hidden)" } else { "" });
        println!("publisher:   {:?}  msi: {}", p.publisher, p.is_msi);
        println!("main exe:    {main_exe:?}");
        println!("signature:   {:?}", prov.signature);
        println!("description: {:?}", prov.description);
        println!("company:     {:?}", prov.company);
        println!("installed:   {:?}", prov.install_date);
        println!("source:      {:?}", prov.source);
        println!("took:        {} ms", started.elapsed().as_millis());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("provenance") {
        match args.get(1) {
            Some(name) => dump_provenance(name),
            None => println!("usage: dump provenance <program name>"),
        }
        return;
    }
    if args.first().map(String::as_str) == Some("deps") {
        for p in enumerate_programs(&WinRegistry) {
            if let Some(key) = stale_core::dependencies::is_shared_dependency(&p) {
                println!("{key:9} | {}", p.name);
            }
        }
        return;
    }
    if args.first().map(String::as_str) == Some("ghosts") {
        dump_ghosts();
        return;
    }
    if args.first().map(String::as_str) == Some("usage") {
        match args.get(1) {
            Some(name) => dump_usage(name),
            None => println!("usage: dump usage <program name>"),
        }
        return;
    }
    if args.first().map(String::as_str) == Some("leftovers") {
        match args.get(1) {
            Some(name) => dump_leftovers(name),
            None => println!("usage: dump leftovers <program name>"),
        }
        return;
    }
    let (tx, rx) = channel();
    let scanner = std::thread::spawn(move || run_scan(ScanSources::live(), tx));

    let (mut programs, mut usage, mut sizes) = (Vec::new(), HashMap::new(), HashMap::new());
    let (mut warnings, mut unmatched) = (Vec::new(), 0);
    for event in rx {
        match event {
            ScanEvent::Programs(p) => programs = p,
            ScanEvent::Usage(u) => usage = u,
            ScanEvent::UnmatchedHits(h) => unmatched = h.len(),
            ScanEvent::Sizes(s) => sizes.extend(s),
            ScanEvent::Warning(w) => warnings.push(w),
            ScanEvent::Done => {}
        }
    }
    scanner.join().expect("scan thread panicked");

    let mut visible: Vec<_> = programs.iter().filter(|p| !p.hidden).collect();
    // `None` sorts before `Some`, so never-seen programs come first.
    visible.sort_by_key(|p| usage.get(&p.id).and_then(|u| u.last_used.when));
    for p in visible {
        let last = usage.get(&p.id).map(|u| &u.last_used);
        let when = last.and_then(|l| l.when).map_or("never".to_string(), |w| w.format("%Y-%m-%d").to_string());
        let source = last.and_then(|l| l.source).map_or("-".to_string(), |s| format!("{s:?}"));
        let size = sizes
            .get(&p.id)
            .copied()
            .map_or("-".to_string(), |b| format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)));
        let drive = p.drive.map_or("-".to_string(), |d| format!("{d}:"));
        let via = last.and_then(|l| l.via.as_deref()).map_or(String::new(), |v| format!(" via {v}"));
        let auto = if last.is_some_and(|l| l.autostart) { " autostart" } else { "" };
        println!("{drive} | {} | {when} ({source}){auto}{via} | {size}", p.name);
    }
    println!("--- {unmatched} unmatched usage hits");
    for w in &warnings {
        println!("warning: {w}");
    }
}
