//! Manual check of the whole core: `cargo run -p stale-core --example dump` (elevated for BAM/Prefetch).
//! Prints `drive | name | last used (source) | size`, never-seen first, then oldest first.

use std::collections::HashMap;
use std::sync::mpsc::channel;

use stale_core::scan::{run_scan, ScanEvent, ScanSources};

fn main() {
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
