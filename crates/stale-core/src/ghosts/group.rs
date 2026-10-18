//! Turns [`Evidence`] into [`Ghost`]s: grouping by program name, assessment, and conversion to
//! the [`Leftover`] items the quarantine flow consumes.

use super::folders::per_profile_kind;
use super::{is_container_folder, is_skipped_folder, parse_location, Evidence, InstalledIndex, RefKind, RootKind};
use crate::keep::KeepList;
use crate::leftovers::fs_scan::FsScanRoots;
use crate::leftovers::names::{is_matchable, normalize_name, publisher_matches};
use crate::leftovers::scan::{dedupe_key, kind_group, rank};
use crate::leftovers::{Confidence, Leftover, LeftoverKind, LeftoverReport};
use crate::model::{Program, RegKeyPath};
use crate::pathutil::{drive_letter, norm, normed_under};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Assessment { ProbablyUninstalled, LeftoversOnly, Uncertain }

/// A program that is gone from the uninstall list, with everything it left behind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ghost {
    /// The most descriptive (longest) original name among its evidence.
    pub name: String,
    /// [`normalize_name`] of `name`.
    pub key: String,
    /// The publisher folder its folder sat in, if any.
    pub publisher: Option<String>,
    pub evidence: Vec<Evidence>,
    pub assessment: Assessment,
    /// Sum of the folder sizes.
    pub total_size: u64,
    /// Newest execution trace.
    pub last_used: Option<DateTime<Utc>>,
}

/// The program roots names are taken below, and the installed publishers whose folders hold one
/// folder per product.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NameRoots {
    /// [`norm`]ed: Program Files (both), ProgramData and each profile's `AppData\Local\Programs`,
    /// `AppData\Local` and `AppData\Roaming`.
    pub roots: Vec<String>,
    /// Publishers of the installed programs.
    pub publishers: Vec<String>,
}

impl NameRoots {
    /// The roots [`orphan_folders`](super::folders::orphan_folders) scans, for every profile in
    /// `profiles`, plus the publishers of `programs`.
    pub fn new(fs_roots: &FsScanRoots, profiles: &[PathBuf], programs: &[Program]) -> NameRoots {
        let mut roots: Vec<String> = fs_roots.program_files.iter().map(|p| norm(p)).collect();
        if !fs_roots.program_data.as_os_str().is_empty() {
            roots.push(norm(&fs_roots.program_data));
        }
        for profile in profiles {
            for rel in fs_roots.per_profile.iter().filter(|rel| per_profile_kind(rel).is_some()) {
                roots.push(norm(&profile.join(rel)));
            }
        }
        let mut publishers: Vec<String> = Vec::new();
        for p in programs.iter().filter_map(|p| p.publisher.as_deref()).filter(|p| !p.trim().is_empty()) {
            if !publishers.iter().any(|q| q == p) {
                publishers.push(p.to_string());
            }
        }
        NameRoots { roots, publishers }
    }

    /// True when `n` ([`norm`]ed) is one of the roots.
    fn is_root(&self, n: &str) -> bool {
        self.roots.iter().any(|r| r == n)
    }

    /// The longest root that `n` ([`norm`]ed) lies strictly under.
    fn root_of(&self, n: &str) -> Option<&str> {
        self.roots.iter().filter(|r| normed_under(n, r)).max_by_key(|r| r.len()).map(String::as_str)
    }

    /// A publisher folder: in [`CONTAINER_FOLDERS`](super::CONTAINER_FOLDERS) or named after an
    /// installed publisher.
    fn is_publisher_folder(&self, name: &str) -> bool {
        is_container_folder(name) || self.publishers.iter().any(|p| publisher_matches(name, p))
    }
}

fn file_name(p: &Path) -> Option<String> {
    p.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// The program folder of `file`: the first folder below a known root, or the second when the
/// first is a publisher folder. `None` when that folder is skipped or not matchable.
fn name_below_root(file: &Path, roots: &NameRoots) -> Option<String> {
    let dir = file.parent()?;
    drive_letter(dir)?;
    let root = roots.root_of(&norm(dir))?;
    let depth = root.split('\\').filter(|c| !c.is_empty()).count();
    let below: Vec<String> = dir
        .to_string_lossy()
        .split(['\\', '/'])
        .filter(|c| !c.is_empty())
        .skip(depth)
        .map(str::to_string)
        .collect();
    let first = below.first()?;
    if is_skipped_folder(first) {
        return None;
    }
    let name = if roots.is_publisher_folder(first) { below.get(1)? } else { first };
    (is_matchable(name) && !is_skipped_folder(name)).then(|| name.clone())
}

/// The key leaf, or its parent's when the leaf is a version or a generic name.
fn key_name(k: &RegKeyPath) -> Option<String> {
    let mut parts = k.path.rsplit('\\').filter(|c| !c.is_empty());
    let leaf = parts.next()?;
    let name = if is_matchable(leaf) { leaf } else { parts.next().filter(|p| is_matchable(p))? };
    Some(name.to_string())
}

/// The program name a piece of evidence points at; `None` when nothing matchable names it.
pub fn ghost_name_of(e: &Evidence, roots: &NameRoots) -> Option<String> {
    match e {
        Evidence::Folder { path, .. } => file_name(path).filter(|n| is_matchable(n)),
        Evidence::RegKeyMissingPath { key, .. } | Evidence::RegKeyOrphan { key } => key_name(key),
        Evidence::DanglingRef { target, .. } => name_below_root(target, roots),
        Evidence::UsageTrace { exe, .. } => name_below_root(exe, roots),
    }
}

/// The publisher folder a [`Evidence::Folder`] sits in: its parent, when that is not a root.
fn container_of(e: &Evidence, roots: &NameRoots) -> Option<PathBuf> {
    let Evidence::Folder { path, .. } = e else { return None };
    path.parent().filter(|p| !roots.is_root(&norm(p))).map(Path::to_path_buf)
}

fn assess(name: &str, evidence: &[Evidence], installed: &InstalledIndex, roots: &NameRoots) -> Assessment {
    let shares_name = installed.matches_first_word(name) || installed.is_word_of_name(name);
    let shares_container =
        evidence.iter().filter_map(|e| container_of(e, roots)).any(|c| installed.owns_sibling_in(&c));
    let strong = evidence.iter().any(|e| {
        matches!(e, Evidence::DanglingRef { .. } | Evidence::UsageTrace { .. } | Evidence::RegKeyMissingPath { .. })
    });
    let appdata_only = evidence.iter().all(|e| matches!(e, Evidence::Folder { root_kind: RootKind::AppData, .. }));
    if shares_name || shares_container {
        Assessment::Uncertain
    } else if strong {
        Assessment::ProbablyUninstalled
    } else if appdata_only {
        Assessment::Uncertain
    } else {
        Assessment::LeftoversOnly
    }
}

fn assessment_rank(a: Assessment) -> u8 {
    match a {
        Assessment::ProbablyUninstalled => 0,
        Assessment::LeftoversOnly => 1,
        Assessment::Uncertain => 2,
    }
}

/// Groups `evidence` by the [`normalize_name`] of [`ghost_name_of`] (spaces ignored, so
/// `OldTool` joins `Old Tool`); evidence without a name is dropped. Groups whose name is kept, or
/// is exactly an installed program's name, are dropped. Sorted by assessment
/// (ProbablyUninstalled, LeftoversOnly, Uncertain), then by size, largest first.
pub fn group(evidence: Vec<Evidence>, installed: &InstalledIndex, keep: &KeepList, roots: &NameRoots) -> Vec<Ghost> {
    let mut groups: BTreeMap<String, (Vec<String>, Vec<Evidence>)> = BTreeMap::new();
    for e in evidence {
        let Some(name) = ghost_name_of(&e, roots) else { continue };
        let slot = groups.entry(normalize_name(&name).replace(' ', "")).or_default();
        slot.0.push(name);
        slot.1.push(e);
    }
    let mut ghosts: Vec<Ghost> = groups
        .into_values()
        .filter_map(|(names, evidence)| {
            if names.iter().any(|n| keep.is_name_kept(n) || installed.matches_exact(n)) {
                return None;
            }
            let name = names.iter().fold(&names[0], |best, n| if n.len() > best.len() { n } else { best }).clone();
            let publisher = evidence.iter().filter_map(|e| container_of(e, roots)).find_map(|c| file_name(&c));
            let total_size = evidence
                .iter()
                .map(|e| match e {
                    Evidence::Folder { size, .. } => size.unwrap_or(0),
                    _ => 0,
                })
                .sum();
            let last_used = evidence
                .iter()
                .filter_map(|e| match e {
                    Evidence::UsageTrace { last_run, .. } => Some(*last_run),
                    _ => None,
                })
                .max();
            Some(Ghost {
                key: normalize_name(&name),
                assessment: assess(&name, &evidence, installed, roots),
                name,
                publisher,
                evidence,
                total_size,
                last_used,
            })
        })
        .collect();
    ghosts.sort_by(|a, b| {
        (assessment_rank(a.assessment), std::cmp::Reverse(a.total_size), &a.key)
            .cmp(&(assessment_rank(b.assessment), std::cmp::Reverse(b.total_size), &b.key))
    });
    ghosts
}

/// `Leftover::note` of a ghost folder whose ghost is [`Assessment::Uncertain`].
pub const NOTE_APPDATA_ONLY: &str = "appdata-only";

fn leftover_of(e: &Evidence, assessment: Assessment, tasks_root: &Path) -> Option<Leftover> {
    let item = |kind, confidence| Leftover { kind, confidence, note: None };
    Some(match e {
        Evidence::Folder { path, size, .. } => {
            let kind = LeftoverKind::Dir { path: path.clone(), size: *size };
            if assessment == Assessment::Uncertain {
                Leftover { kind, confidence: Confidence::Low, note: Some(NOTE_APPDATA_ONLY.to_string()) }
            } else {
                item(kind, Confidence::Medium)
            }
        }
        Evidence::RegKeyMissingPath { key, .. } | Evidence::RegKeyOrphan { key } => {
            item(LeftoverKind::RegKey { key: key.clone() }, Confidence::Medium)
        }
        Evidence::DanglingRef { kind, location, target, value_name } => {
            let kind = match kind {
                RefKind::Run => LeftoverKind::RegValue { key: parse_location(location)?, name: value_name.clone()? },
                RefKind::AppPaths | RefKind::ClassesServer | RefKind::ShellCommand => {
                    LeftoverKind::RegKey { key: parse_location(location)? }
                }
                RefKind::Service => LeftoverKind::Service { name: location.clone(), image: target.clone() },
                RefKind::ScheduledTask => LeftoverKind::ScheduledTask {
                    name: location.clone(),
                    xml: tasks_root.join(location),
                    command: target.clone(),
                },
                RefKind::Shortcut => LeftoverKind::Shortcut { lnk: PathBuf::from(location), target: target.clone() },
            };
            item(kind, Confidence::High)
        }
        Evidence::UsageTrace { .. } => return None,
    })
}

/// What removing `g` would remove, as leftover items: folders Medium (Low with
/// [`NOTE_APPDATA_ONLY`] when the ghost is uncertain), vendor keys Medium, dangling references
/// High; execution traces are evidence only. Duplicates are merged (higher confidence wins) and
/// the items sorted like [`scan_leftovers`](crate::leftovers::scan_leftovers).
pub fn to_leftovers(g: &Ghost, tasks_root: &Path) -> LeftoverReport {
    let mut items: Vec<Leftover> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for item in g.evidence.iter().filter_map(|e| leftover_of(e, g.assessment, tasks_root)) {
        match index.get(&dedupe_key(&item.kind)) {
            Some(&i) => {
                if rank(item.confidence) < rank(items[i].confidence) {
                    items[i] = item;
                }
            }
            None => {
                index.insert(dedupe_key(&item.kind), items.len());
                items.push(item);
            }
        }
    }
    items.sort_by_key(|l| (rank(l.confidence), kind_group(&l.kind)));
    LeftoverReport { items }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsys::FakeFs;
    use crate::model::{Hive, ProgramId, Scope};
    use crate::registry::key;
    use crate::usage::UsageSource;
    use chrono::TimeZone;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn program(name: &str, publisher: Option<&str>, install: Option<&str>) -> Program {
        Program {
            id: ProgramId(name.into()),
            key: RegKeyPath { hive: Hive::LocalMachine, path: format!(r"Software\{name}") },
            name: name.into(),
            publisher: publisher.map(str::to_string),
            version: None,
            install_location: install.map(PathBuf::from),
            install_date: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            display_icon: None,
            is_msi: false,
            estimated_size_bytes: None,
            drive: None,
            hidden: false,
            scope: Scope::Machine,
        }
    }

    fn roots(programs: &[Program]) -> NameRoots {
        let fs = FakeFs::new()
            .dir(r"C:\Program Files", 0)
            .dir(r"C:\Program Files (x86)", 0)
            .env("ProgramFiles", r"C:\Program Files")
            .env("ProgramFiles(x86)", r"C:\Program Files (x86)")
            .env("ProgramData", r"C:\ProgramData");
        NameRoots::new(&FsScanRoots::from_env(&fs), &[PathBuf::from(r"C:\Users\A")], programs)
    }

    fn folder(path: &str, size: u64, root_kind: RootKind) -> Evidence {
        Evidence::Folder { path: path.into(), size: Some(size), modified: None, root_kind }
    }

    fn pf(path: &str, size: u64) -> Evidence {
        folder(path, size, RootKind::ProgramFiles)
    }

    fn run(target: &str, value: &str) -> Evidence {
        Evidence::DanglingRef {
            kind: RefKind::Run,
            location: format!(r"CurrentUser\{RUN}"),
            target: target.into(),
            value_name: Some(value.into()),
        }
    }

    fn trace(exe: &str, day: u32) -> Evidence {
        Evidence::UsageTrace { exe: exe.into(), last_run: Utc.with_ymd_and_hms(2026, 1, day, 0, 0, 0).unwrap(), source: UsageSource::Bam }
    }

    fn orphan_key(path: &str) -> Evidence {
        Evidence::RegKeyOrphan { key: key(Hive::LocalMachine, path) }
    }

    fn missing_path(path: &str) -> Evidence {
        Evidence::RegKeyMissingPath { key: key(Hive::LocalMachine, path), value_name: "InstallDir".into(), path: r"C:\Gone".into() }
    }

    fn installed(programs: &[Program]) -> InstalledIndex {
        InstalledIndex::from_programs(programs)
    }

    /// `(name, assessment)` per ghost, in order.
    fn summary(ghosts: &[Ghost]) -> Vec<(String, Assessment)> {
        ghosts.iter().map(|g| (g.name.clone(), g.assessment)).collect()
    }

    #[test]
    fn ghost_name_of_picks_matchable_component() {
        let programs = [program("Widget Suite", Some("Acme Inc."), None)];
        let r = roots(&programs);
        let name = |e: Evidence| ghost_name_of(&e, &r);
        assert_eq!(name(pf(r"C:\Program Files\Old Tool", 1)).as_deref(), Some("Old Tool"));
        assert_eq!(name(run(r"C:\Program Files\Foobar\bin\x.exe", "x")).as_deref(), Some("Foobar"));
        assert_eq!(name(trace(r"C:\Users\A\AppData\Local\Programs\Foobar\f.exe", 1)).as_deref(), Some("Foobar"));
        assert_eq!(name(trace(r"C:\Users\A\AppData\Local\Barley\b.exe", 1)).as_deref(), Some("Barley"));
        assert_eq!(name(trace(r"C:\Users\A\AppData\Roaming\Barley\x\b.exe", 1)).as_deref(), Some("Barley"));
        // Publisher folders: a container entry or an installed publisher; the product is next.
        assert_eq!(name(run(r"C:\Program Files (x86)\Google\Earth Pro\e.exe", "e")).as_deref(), Some("Earth Pro"));
        assert_eq!(name(trace(r"C:\Program Files\Acme\Gizmo Pro\g.exe", 1)).as_deref(), Some("Gizmo Pro"));
        // Skipped, generic, root-level or outside every root: no name.
        assert_eq!(name(trace(r"C:\Program Files\Common Files\Thing\t.exe", 1)), None);
        assert_eq!(name(trace(r"C:\Program Files\Tools\t.exe", 1)), None);
        assert_eq!(name(trace(r"C:\Program Files\gone.exe", 1)), None);
        assert_eq!(name(trace(r"D:\Elsewhere\Thing\t.exe", 1)), None);
        // Registry: the key leaf, or its parent when the leaf is a version or generic.
        assert_eq!(name(missing_path(r"SOFTWARE\Old Tool")).as_deref(), Some("Old Tool"));
        assert_eq!(name(orphan_key(r"SOFTWARE\Vendorly\Product\2.1")).as_deref(), Some("Product"));
        assert_eq!(name(orphan_key(r"SOFTWARE\Gadgetry\Tools")).as_deref(), Some("Gadgetry"));
        assert_eq!(name(orphan_key(r"SOFTWARE\Tools")), None);
    }

    #[test]
    fn group_merges_folder_key_and_run_value_by_name() {
        let r = roots(&[]);
        let ev = vec![
            pf(r"C:\Program Files\Old Tool", 100),
            missing_path(r"SOFTWARE\OldTool"),
            run(r"C:\Program Files\Old Tool\old.exe", "OldTool"),
            pf(r"C:\Program Files\Other Thing", 5),
            trace(r"C:\Program Files\Unnamed.exe", 1),
        ];
        let ghosts = group(ev, &installed(&[]), &KeepList::default(), &r);
        assert_eq!(ghosts.len(), 2, "{ghosts:?}");
        let old = ghosts.iter().find(|g| g.key == "old tool").expect("old tool");
        assert_eq!(old.name, "Old Tool");
        assert_eq!(old.evidence.len(), 3);
        assert_eq!(old.total_size, 100);
        assert_eq!(old.publisher, None);
        assert!(ghosts.iter().any(|g| g.name == "Other Thing" && g.evidence.len() == 1));
    }

    #[test]
    fn assessment_probably_uninstalled_needs_dangling_or_trace() {
        let r = roots(&[]);
        let ev = vec![
            pf(r"C:\Program Files\Folder Only", 10),
            pf(r"C:\Program Files\Folder Key", 50),
            orphan_key(r"SOFTWARE\Folder Key"),
            pf(r"C:\Program Files\Was Used", 1),
            trace(r"C:\Program Files\Was Used\w.exe", 3),
            trace(r"C:\Program Files\Was Used\bin\w2.exe", 7),
            missing_path(r"SOFTWARE\Key Gone"),
        ];
        let ghosts = group(ev, &installed(&[]), &KeepList::default(), &r);
        assert_eq!(
            summary(&ghosts),
            vec![
                ("Was Used".to_string(), Assessment::ProbablyUninstalled),
                ("Key Gone".to_string(), Assessment::ProbablyUninstalled),
                ("Folder Key".to_string(), Assessment::LeftoversOnly),
                ("Folder Only".to_string(), Assessment::LeftoversOnly),
            ]
        );
        assert_eq!(ghosts[0].last_used, Some(Utc.with_ymd_and_hms(2026, 1, 7, 0, 0, 0).unwrap()));
        assert_eq!(ghosts[2].last_used, None);
    }

    #[test]
    fn assessment_uncertain_on_first_word_match_or_appdata_only() {
        let programs = [program("Acme Studio", None, None)];
        let r = roots(&programs);
        let ev = vec![
            // "Acme" is only the first word of an installed name: maybe its vendor folder.
            pf(r"C:\Program Files\Acme", 900),
            run(r"C:\Program Files\Acme\a.exe", "Acme"),
            folder(r"C:\Users\A\AppData\Roaming\Settings Only", 5, RootKind::AppData),
            folder(r"C:\Users\A\AppData\Local\Both Places", 5, RootKind::AppData),
            pf(r"C:\Program Files\Both Places", 1),
        ];
        let ghosts = group(ev, &installed(&programs), &KeepList::default(), &r);
        assert_eq!(
            summary(&ghosts),
            vec![
                ("Both Places".to_string(), Assessment::LeftoversOnly),
                ("Acme".to_string(), Assessment::Uncertain),
                ("Settings Only".to_string(), Assessment::Uncertain),
            ]
        );
    }

    #[test]
    fn uncertain_when_name_is_word_of_installed_display_name() {
        let programs = [program("Microsoft Visual Studio Code", None, None)];
        let r = roots(&programs);
        let ev = vec![pf(r"C:\Program Files\Code", 10), trace(r"C:\Program Files\Code\code.exe", 1)];
        let ghosts = group(ev, &installed(&programs), &KeepList::default(), &r);
        assert_eq!(summary(&ghosts), vec![("Code".to_string(), Assessment::Uncertain)]);
    }

    #[test]
    fn uncertain_when_sibling_owned_by_installed() {
        let programs = [program("Gadget Pro", None, Some(r"C:\Program Files\Vendorz\Gadget Pro"))];
        let r = roots(&programs);
        let ev = vec![
            pf(r"C:\Program Files\Vendorz\Old Widget", 10),
            run(r"C:\Program Files\Old Widget\w.exe", "Widget"),
            pf(r"C:\Program Files\Lonely Corp\Old Gizmo", 10),
            run(r"C:\Program Files\Old Gizmo\g.exe", "Gizmo"),
        ];
        let ghosts = group(ev, &installed(&programs), &KeepList::default(), &r);
        assert_eq!(
            summary(&ghosts),
            vec![
                ("Old Gizmo".to_string(), Assessment::ProbablyUninstalled),
                ("Old Widget".to_string(), Assessment::Uncertain),
            ]
        );
        assert_eq!(ghosts[0].publisher.as_deref(), Some("Lonely Corp"));
        assert_eq!(ghosts[1].publisher.as_deref(), Some("Vendorz"));
    }

    #[test]
    fn installed_name_group_is_dropped() {
        let programs = [program("Old Tool", None, None)];
        let r = roots(&programs);
        let ev = vec![pf(r"C:\Program Files\OldTool", 1), orphan_key(r"SOFTWARE\Old Tool"), pf(r"C:\Program Files\Other Thing", 1)];
        let ghosts = group(ev, &installed(&programs), &KeepList::default(), &r);
        assert_eq!(summary(&ghosts), vec![("Other Thing".to_string(), Assessment::LeftoversOnly)]);
    }

    #[test]
    fn kept_name_is_dropped() {
        let r = roots(&[]);
        let keep = KeepList { names: vec!["old tool".into()], ..KeepList::default() };
        let ev = vec![pf(r"C:\Program Files\Old Tool", 1), trace(r"C:\Program Files\Old Tool\o.exe", 1), pf(r"C:\Program Files\Other Thing", 1)];
        let ghosts = group(ev, &installed(&[]), &keep, &r);
        assert_eq!(summary(&ghosts), vec![("Other Thing".to_string(), Assessment::LeftoversOnly)]);
    }

    fn dangling(kind: RefKind, location: &str, target: &str) -> Evidence {
        Evidence::DanglingRef { kind, location: location.into(), target: target.into(), value_name: None }
    }

    #[test]
    fn to_leftovers_confidences() {
        let tasks = Path::new(r"C:\Windows\System32\Tasks");
        let hklm = |p: &str| key(Hive::LocalMachine, p);
        let mut g = Ghost {
            name: "Old Tool".into(),
            key: "old tool".into(),
            publisher: None,
            evidence: vec![
                trace(r"C:\Program Files\Old Tool\o.exe", 1),
                dangling(RefKind::Shortcut, r"C:\Menu\Old Tool.lnk", r"C:\Program Files\Old Tool\o.exe"),
                pf(r"C:\Program Files\Old Tool", 7),
                missing_path(r"SOFTWARE\Old Tool"),
                Evidence::RegKeyMissingPath { key: hklm(r"SOFTWARE\Old Tool"), value_name: "Path".into(), path: r"C:\Gone".into() },
                orphan_key(r"SOFTWARE\Old Tool\Sub"),
                run(r"C:\Program Files\Old Tool\o.exe", "OldTool"),
                dangling(RefKind::AppPaths, r"LocalMachine\SOFTWARE\App Paths\o.exe", r"C:\Program Files\Old Tool\o.exe"),
                dangling(RefKind::ClassesServer, r"LocalMachine\SOFTWARE\Classes\CLSID\{1}", r"C:\Program Files\Old Tool\o.dll"),
                dangling(RefKind::ShellCommand, r"LocalMachine\SOFTWARE\Classes\oldfile", r"C:\Program Files\Old Tool\o.exe"),
                dangling(RefKind::Service, "OldSvc", r"C:\Program Files\Old Tool\svc.exe"),
                dangling(RefKind::ScheduledTask, r"Old\Update", r"C:\Program Files\Old Tool\up.exe"),
            ],
            assessment: Assessment::ProbablyUninstalled,
            total_size: 7,
            last_used: None,
        };
        let items: Vec<(LeftoverKind, Confidence, Option<String>)> =
            to_leftovers(&g, tasks).items.into_iter().map(|l| (l.kind, l.confidence, l.note)).collect();
        let exe = PathBuf::from(r"C:\Program Files\Old Tool\o.exe");
        let high = Confidence::High;
        let medium = Confidence::Medium;
        assert_eq!(
            items,
            vec![
                (LeftoverKind::RegValue { key: key(Hive::CurrentUser, RUN), name: "OldTool".into() }, high, None),
                (LeftoverKind::RegKey { key: hklm(r"SOFTWARE\App Paths\o.exe") }, high, None),
                (LeftoverKind::RegKey { key: hklm(r"SOFTWARE\Classes\CLSID\{1}") }, high, None),
                (LeftoverKind::RegKey { key: hklm(r"SOFTWARE\Classes\oldfile") }, high, None),
                (LeftoverKind::Shortcut { lnk: r"C:\Menu\Old Tool.lnk".into(), target: exe.clone() }, high, None),
                (LeftoverKind::Service { name: "OldSvc".into(), image: r"C:\Program Files\Old Tool\svc.exe".into() }, high, None),
                (
                    LeftoverKind::ScheduledTask {
                        name: r"Old\Update".into(),
                        xml: tasks.join(r"Old\Update"),
                        command: r"C:\Program Files\Old Tool\up.exe".into()
                    },
                    high,
                    None
                ),
                (LeftoverKind::Dir { path: r"C:\Program Files\Old Tool".into(), size: Some(7) }, medium, None),
                (LeftoverKind::RegKey { key: hklm(r"SOFTWARE\Old Tool") }, medium, None),
                (LeftoverKind::RegKey { key: hklm(r"SOFTWARE\Old Tool\Sub") }, medium, None),
            ]
        );
        g.assessment = Assessment::Uncertain;
        let dir = to_leftovers(&g, tasks).items.into_iter().find(|l| matches!(l.kind, LeftoverKind::Dir { .. })).unwrap();
        assert_eq!((dir.confidence, dir.note.as_deref()), (Confidence::Low, Some("appdata-only")));
    }
}
