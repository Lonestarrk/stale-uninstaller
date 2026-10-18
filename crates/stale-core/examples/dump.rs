//! Manual check of the whole core: `cargo run -p stale-core --example dump` (elevated for BAM/Prefetch).
//! Prints `drive | name | last used (source) | size`, never-seen first, then oldest first.

use std::collections::HashMap;
use std::sync::mpsc::channel;

use stale_core::leftovers::{scan_leftovers, Confidence, LeftoverKind, LeftoverSources, ProgramHint};
use stale_core::programs::enumerate_programs;
use stale_core::registry::WinRegistry;
use stale_core::scan::{run_scan, ScanEvent, ScanSources};

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
    let report = scan_leftovers(&LeftoverSources::live(), &ProgramHint::from_program(program));
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
        println!("{drive} | {} | {when} ({source}) | {size}", p.name);
    }
    println!("--- {unmatched} unmatched usage hits");
    for w in &warnings {
        println!("warning: {w}");
    }
}
