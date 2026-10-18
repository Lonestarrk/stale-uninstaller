//! Turns [`Evidence`] into [`Ghost`]s: grouping by program name, assessment, and conversion to
//! the [`Leftover`] items the quarantine flow consumes.

use super::folders::per_profile_kind;
use super::{is_container_folder, is_skipped_folder, parse_location, Evidence, InstalledIndex, RefKind, RootKind};
use crate::keep::KeepList;
use crate::leftovers::fs_scan::FsScanRoots;
use crate::leftovers::names::{is_matchable, name_matches_exact, normalize_name, publisher_matches};
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
    /// Why the ghost is [`Assessment::Uncertain`]: [`REASON_FIRST_WORD`], [`REASON_WORD_OF_NAME`],
    /// [`REASON_SIBLING_OWNED`], [`REASON_OTHER_PROFILE`] or [`REASON_APPDATA_ONLY`]; empty otherwise.
    #[serde(default)]
    pub reasons: Vec<String>,
    /// Language-free key such as [`NOTE_OTHER_PROFILE`]; the GUI maps it to text.
    #[serde(default)]
    pub note: Option<String>,
}

/// The name is only the first word of an installed name (`Google` for `Google Chrome`).
pub const REASON_FIRST_WORD: &str = "first-word-match";
/// The name is a run of whole words of an installed name (`Code` in `Microsoft Visual Studio Code`).
pub const REASON_WORD_OF_NAME: &str = "word-of-installed-name";
/// A folder sits in a publisher folder that also holds an installed program.
pub const REASON_SIBLING_OWNED: &str = "sibling-owned";
/// A folder lies in another user's profile, whose per-user installs are invisible to us.
pub const REASON_OTHER_PROFILE: &str = "other-profile";
/// The name is an installed program's publisher (`Binary Fortress Software` for DisplayFusion).
pub const REASON_PUBLISHER_OF_INSTALLED: &str = "publisher-of-installed";
/// The only evidence is AppData folders.
pub const REASON_APPDATA_ONLY: &str = "appdata-only";
/// The only evidence is ProgramData and AppData folders (data, not a program).
pub const REASON_DATA_ONLY: &str = "data-only";
/// `Ghost::note` of a ghost with a folder in another user's profile.
pub const NOTE_OTHER_PROFILE: &str = "other-profile";

/// The program roots names are taken below, and the installed publishers whose folders hold one
/// folder per product.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NameRoots {
    /// [`norm`]ed: Program Files (both), ProgramData and each profile's `AppData\Local\Programs`,
    /// `AppData\Local` and `AppData\Roaming`.
    pub roots: Vec<String>,
    /// Publishers of the installed programs.
    pub publishers: Vec<String>,
    /// Folder names of the user profiles (`Jobb` for `C:\Users\Jobb`): a folder named like one is
    /// another user's deploy root, never a ghost.
    pub profile_names: Vec<String>,
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
        let profile_names = profiles.iter().filter_map(|p| file_name(p)).collect();
        NameRoots { roots, publishers, profile_names }
    }

    fn is_profile_name(&self, name: &str) -> bool {
        self.profile_names.iter().any(|p| p.eq_ignore_ascii_case(name))
    }

    /// True when `name` is (a form of) an installed publisher.
    fn is_installed_publisher(&self, name: &str) -> bool {
        self.publishers.iter().any(|p| publisher_matches(name, p))
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
        is_container_folder(name) || self.is_installed_publisher(name)
    }
}

fn file_name(p: &Path) -> Option<String> {
    p.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// A GUID-like `{…-…-…}` group, or a run of 8+ hex digits with at least one decimal digit
/// (`1de4eec8`): an install id, not a program name.
fn has_id_token(name: &str) -> bool {
    let guid = name.find('{').is_some_and(|open| {
        name[open..].find('}').is_some_and(|close| name[open..open + close].matches('-').count() >= 2)
    });
    let mut run = 0;
    let mut digit = false;
    for c in name.chars() {
        if c.is_ascii_hexdigit() {
            run += 1;
            digit |= c.is_ascii_digit();
            if run >= 8 && digit {
                return true;
            }
        } else {
            run = 0;
            digit = false;
        }
    }
    guid
}

/// A name that can name a ghost: [`is_matchable`] and free of install ids.
fn usable(name: &str) -> bool {
    is_matchable(name) && !has_id_token(name)
}

/// The program folder of `file` and its name: the first folder below a known root, or the second
/// when the first is a publisher folder (`root[\publisher]\name`). `None` when that folder is
/// skipped or not matchable.
fn program_folder(file: &Path, roots: &NameRoots) -> Option<(PathBuf, String)> {
    let dir = file.parent()?;
    drive_letter(dir)?;
    let root = roots.root_of(&norm(dir))?;
    let depth = root.split('\\').filter(|c| !c.is_empty()).count();
    let dir = dir.to_string_lossy();
    let parts: Vec<&str> = dir.split(['\\', '/']).filter(|c| !c.is_empty()).collect();
    let first = *parts.get(depth)?;
    if is_skipped_folder(first) {
        return None;
    }
    let at = if roots.is_publisher_folder(first) { depth + 1 } else { depth };
    let name = *parts.get(at)?;
    (usable(name) && !is_skipped_folder(name)).then(|| (PathBuf::from(parts[..=at].join(r"\")), name.to_string()))
}

/// The key leaf, or its parent's when the leaf is a version or a generic name.
fn key_name(k: &RegKeyPath) -> Option<String> {
    let mut parts = k.path.rsplit('\\').filter(|c| !c.is_empty());
    let leaf = parts.next()?;
    let name = if usable(leaf) { leaf } else { parts.next().filter(|p| usable(p))? };
    Some(name.to_string())
}

/// The program name a piece of evidence points at; `None` when nothing matchable names it.
pub fn ghost_name_of(e: &Evidence, roots: &NameRoots) -> Option<String> {
    match e {
        Evidence::Folder { path, .. } => file_name(path).filter(|n| usable(n) && !roots.is_profile_name(n)),
        Evidence::RegKeyMissingPath { key, .. } | Evidence::RegKeyOrphan { key } => key_name(key),
        Evidence::DanglingRef { target, .. } => program_folder(target, roots).map(|(_, name)| name),
        Evidence::UsageTrace { exe, .. } => program_folder(exe, roots).map(|(_, name)| name),
    }
}

/// A dangling reference into a folder an installed program still occupies: that program's own
/// broken registration (an old version's file handlers), not a leftover.
fn is_installed_programs_own(e: &Evidence, installed: &InstalledIndex, roots: &NameRoots) -> bool {
    let Evidence::DanglingRef { target, .. } = e else { return false };
    program_folder(target, roots).is_some_and(|(folder, _)| installed.covers_path(&folder) || installed.holds_location(&folder))
}

/// The publisher folder a [`Evidence::Folder`] sits in: its parent, when that is not a root.
fn container_of(e: &Evidence, roots: &NameRoots) -> Option<PathBuf> {
    let Evidence::Folder { path, .. } = e else { return None };
    path.parent().filter(|p| !roots.is_root(&norm(p))).map(Path::to_path_buf)
}

/// True for a per-profile folder (AppData, `AppData\Local\Programs`) outside `current_profile`.
fn in_other_profile(e: &Evidence, current_profile: Option<&Path>) -> bool {
    let (Evidence::Folder { path, root_kind: RootKind::AppData | RootKind::LocalPrograms, .. }, Some(current)) =
        (e, current_profile)
    else {
        return false;
    };
    !normed_under(&norm(path), &norm(current))
}

/// The assessment and, for [`Assessment::Uncertain`], the reasons. The name and folder reasons
/// override strong evidence; [`REASON_APPDATA_ONLY`] and [`REASON_DATA_ONLY`] (folders only)
/// can only hold without it.
fn assess(
    name: &str,
    evidence: &[Evidence],
    installed: &InstalledIndex,
    roots: &NameRoots,
    current_profile: Option<&Path>,
) -> (Assessment, Vec<String>) {
    let mut reasons: Vec<&str> = Vec::new();
    if installed.matches_first_word(name) {
        reasons.push(REASON_FIRST_WORD);
    } else if installed.is_word_of_name(name) {
        reasons.push(REASON_WORD_OF_NAME);
    }
    if roots.is_installed_publisher(name) {
        reasons.push(REASON_PUBLISHER_OF_INSTALLED);
    }
    if evidence.iter().filter_map(|e| container_of(e, roots)).any(|c| installed.owns_sibling_in(&c)) {
        reasons.push(REASON_SIBLING_OWNED);
    }
    if evidence.iter().any(|e| in_other_profile(e, current_profile)) {
        reasons.push(REASON_OTHER_PROFILE);
    }
    let strong = evidence.iter().any(|e| {
        matches!(e, Evidence::DanglingRef { .. } | Evidence::UsageTrace { .. } | Evidence::RegKeyMissingPath { .. })
    });
    let folder_in = |kinds: &[RootKind]| {
        evidence.iter().all(|e| matches!(e, Evidence::Folder { root_kind, .. } if kinds.contains(root_kind)))
    };
    if folder_in(&[RootKind::AppData]) {
        reasons.push(REASON_APPDATA_ONLY);
    } else if folder_in(&[RootKind::AppData, RootKind::ProgramData]) {
        reasons.push(REASON_DATA_ONLY);
    }
    let assessment = if !reasons.is_empty() {
        Assessment::Uncertain
    } else if strong {
        Assessment::ProbablyUninstalled
    } else {
        Assessment::LeftoversOnly
    };
    (assessment, reasons.into_iter().map(str::to_string).collect())
}

fn assessment_rank(a: Assessment) -> u8 {
    match a {
        Assessment::ProbablyUninstalled => 0,
        Assessment::LeftoversOnly => 1,
        Assessment::Uncertain => 2,
    }
}

/// Groups `evidence` by the [`normalize_name`] of [`ghost_name_of`] (spaces ignored, so
/// `OldTool` joins `Old Tool`); evidence without a name, and dangling references into a folder
/// an installed program still occupies, are dropped. Groups whose name is kept, or
/// is exactly an installed program's name, are dropped. Sorted by assessment
/// (ProbablyUninstalled, LeftoversOnly, Uncertain), then by size, largest first.
/// `current_profile` is the running user's profile folder; per-profile folders outside it make a
/// ghost Uncertain with [`NOTE_OTHER_PROFILE`].
pub fn group(
    evidence: Vec<Evidence>,
    installed: &InstalledIndex,
    keep: &KeepList,
    roots: &NameRoots,
    current_profile: Option<&Path>,
) -> Vec<Ghost> {
    let mut groups: BTreeMap<String, (Vec<String>, Vec<Evidence>)> = BTreeMap::new();
    for e in evidence {
        if is_installed_programs_own(&e, installed, roots) {
            continue;
        }
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
            let (assessment, reasons) = assess(&name, &evidence, installed, roots, current_profile);
            let note = reasons.iter().any(|r| r == REASON_OTHER_PROFILE).then(|| NOTE_OTHER_PROFILE.to_string());
            Some(Ghost {
                key: normalize_name(&name),
                assessment,
                name,
                publisher,
                evidence,
                total_size,
                last_used,
                reasons,
                note,
            })
        })
        .collect();
    ghosts.sort_by(|a, b| {
        (assessment_rank(a.assessment), std::cmp::Reverse(a.total_size), &a.key)
            .cmp(&(assessment_rank(b.assessment), std::cmp::Reverse(b.total_size), &b.key))
    });
    ghosts
}

/// The key a dangling shell command removes: the whole ProgID key when its name is the ghost's
/// (`GoogleDriveFS.gdoc` for `GoogleDriveFS`), else only its `shell\open\command` (`http`).
fn shell_command_key(location: &str, ghost_name: &str) -> Option<RegKeyPath> {
    let mut key = parse_location(location)?;
    let leaf = key.path.rsplit('\\').next().unwrap_or_default();
    let stem = leaf.split('.').next().unwrap_or_default();
    if !(name_matches_exact(leaf, ghost_name) || name_matches_exact(stem, ghost_name)) {
        key.path.push_str(r"\shell\open\command");
    }
    Some(key)
}

fn leftover_of(e: &Evidence, g: &Ghost, tasks_root: &Path) -> Option<Leftover> {
    let item = |kind, confidence| Leftover { kind, confidence, note: None };
    // Folders and vendor keys are only as certain as the ghost; Low with its first reason.
    let owned = |kind| {
        if g.assessment == Assessment::Uncertain {
            Leftover { kind, confidence: Confidence::Low, note: g.reasons.first().cloned() }
        } else {
            item(kind, Confidence::Medium)
        }
    };
    Some(match e {
        Evidence::Folder { path, size, .. } => owned(LeftoverKind::Dir { path: path.clone(), size: *size }),
        Evidence::RegKeyMissingPath { key, .. } | Evidence::RegKeyOrphan { key } => {
            owned(LeftoverKind::RegKey { key: key.clone() })
        }
        Evidence::DanglingRef { kind, location, target, value_name } => {
            let kind = match kind {
                RefKind::Run => LeftoverKind::RegValue { key: parse_location(location)?, name: value_name.clone()? },
                RefKind::AppPaths | RefKind::ClassesServer => LeftoverKind::RegKey { key: parse_location(location)? },
                RefKind::ShellCommand => LeftoverKind::RegKey { key: shell_command_key(location, &g.name)? },
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

/// What removing `g` would remove, as leftover items: folders and vendor keys Medium (Low with
/// the first reason as note when the ghost is uncertain), dangling references High (their
/// targets are verifiably gone); execution traces are evidence only. Duplicates are merged (higher confidence wins) and
/// the items sorted like [`scan_leftovers`](crate::leftovers::scan_leftovers).
pub fn to_leftovers(g: &Ghost, tasks_root: &Path) -> LeftoverReport {
    let mut items: Vec<Leftover> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for item in g.evidence.iter().filter_map(|e| leftover_of(e, g, tasks_root)) {
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
            account: None,
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
        roots_for(&[r"C:\Users\A"], programs)
    }

    fn roots_for(profiles: &[&str], programs: &[Program]) -> NameRoots {
        let fs = FakeFs::new()
            .dir(r"C:\Program Files", 0)
            .dir(r"C:\Program Files (x86)", 0)
            .env("ProgramFiles", r"C:\Program Files")
            .env("ProgramFiles(x86)", r"C:\Program Files (x86)")
            .env("ProgramData", r"C:\ProgramData");
        let profiles: Vec<PathBuf> = profiles.iter().map(PathBuf::from).collect();
        NameRoots::new(&FsScanRoots::from_env(&fs), &profiles, programs)
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

    fn reasons(g: &Ghost) -> Vec<&str> {
        g.reasons.iter().map(String::as_str).collect()
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
        // Install ids name nothing; a GUID key leaf falls back to its parent.
        assert_eq!(name(folder(r"C:\ProgramData\Mozilla-1de4eec8-1241-4177-a864-e594e8d1fb38", 1, RootKind::ProgramData)), None);
        assert_eq!(name(folder(r"C:\ProgramData\Cache 0123abcd99", 1, RootKind::ProgramData)), None);
        assert_eq!(name(trace(r"C:\Program Files\{ab-cd-ef} Tool\t.exe", 1)), None);
        assert_eq!(
            name(orphan_key(r"SOFTWARE\Vendorly\{0F1E2D3C-1111-2222-3333-444455556666}")).as_deref(),
            Some("Vendorly")
        );
        assert_eq!(name(pf(r"C:\Program Files\Deadbeef Facade", 1)).as_deref(), Some("Deadbeef Facade"));
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
        let ghosts = group(ev, &installed(&[]), &KeepList::default(), &r, None);
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
        let ghosts = group(ev, &installed(&[]), &KeepList::default(), &r, None);
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
        let ghosts = group(ev, &installed(&programs), &KeepList::default(), &r, None);
        assert_eq!(
            summary(&ghosts),
            vec![
                ("Both Places".to_string(), Assessment::LeftoversOnly),
                ("Acme".to_string(), Assessment::Uncertain),
                ("Settings Only".to_string(), Assessment::Uncertain),
            ]
        );
        assert_eq!(reasons(&ghosts[1]), vec![REASON_FIRST_WORD]);
        assert_eq!(reasons(&ghosts[2]), vec![REASON_APPDATA_ONLY]);
        assert!(ghosts[0].reasons.is_empty() && ghosts.iter().all(|g| g.note.is_none()));
    }

    #[test]
    fn uncertain_when_name_is_word_of_installed_display_name() {
        let programs = [program("Microsoft Visual Studio Code", None, None)];
        let r = roots(&programs);
        let ev = vec![pf(r"C:\Program Files\Code", 10), trace(r"C:\Program Files\Code\code.exe", 1)];
        let ghosts = group(ev, &installed(&programs), &KeepList::default(), &r, None);
        assert_eq!(summary(&ghosts), vec![("Code".to_string(), Assessment::Uncertain)]);
        assert_eq!(reasons(&ghosts[0]), vec![REASON_WORD_OF_NAME]);
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
        let ghosts = group(ev, &installed(&programs), &KeepList::default(), &r, None);
        assert_eq!(
            summary(&ghosts),
            vec![
                ("Old Gizmo".to_string(), Assessment::ProbablyUninstalled),
                ("Old Widget".to_string(), Assessment::Uncertain),
            ]
        );
        assert_eq!(ghosts[0].publisher.as_deref(), Some("Lonely Corp"));
        assert_eq!(ghosts[1].publisher.as_deref(), Some("Vendorz"));
        assert_eq!(reasons(&ghosts[1]), vec![REASON_SIBLING_OWNED]);
    }

    #[test]
    fn folder_in_other_profile_is_uncertain_with_note() {
        let r = roots(&[]);
        let ev = vec![
            folder(r"C:\Users\B\AppData\Local\Programs\Their App", 50, RootKind::LocalPrograms),
            trace(r"C:\Program Files\Their App\t.exe", 1),
            folder(r"C:\Users\A\AppData\Local\Programs\My App", 10, RootKind::LocalPrograms),
        ];
        let ghosts = group(ev, &installed(&[]), &KeepList::default(), &r, Some(Path::new(r"C:\Users\A")));
        assert_eq!(
            summary(&ghosts),
            vec![("My App".to_string(), Assessment::LeftoversOnly), ("Their App".to_string(), Assessment::Uncertain)]
        );
        assert_eq!(reasons(&ghosts[1]), vec![REASON_OTHER_PROFILE]);
        assert_eq!(ghosts[1].note.as_deref(), Some(NOTE_OTHER_PROFILE));
        assert_eq!(ghosts[0].note, None);
    }

    #[test]
    fn installed_name_group_is_dropped() {
        let programs = [program("Old Tool", None, None)];
        let r = roots(&programs);
        let ev = vec![pf(r"C:\Program Files\OldTool", 1), orphan_key(r"SOFTWARE\Old Tool"), pf(r"C:\Program Files\Other Thing", 1)];
        let ghosts = group(ev, &installed(&programs), &KeepList::default(), &r, None);
        assert_eq!(summary(&ghosts), vec![("Other Thing".to_string(), Assessment::LeftoversOnly)]);
    }

    #[test]
    fn kept_name_is_dropped() {
        let r = roots(&[]);
        let keep = KeepList { names: vec!["old tool".into()], ..KeepList::default() };
        let ev = vec![pf(r"C:\Program Files\Old Tool", 1), trace(r"C:\Program Files\Old Tool\o.exe", 1), pf(r"C:\Program Files\Other Thing", 1)];
        let ghosts = group(ev, &installed(&[]), &keep, &r, None);
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
            reasons: Vec::new(),
            note: None,
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
                (LeftoverKind::RegKey { key: hklm(r"SOFTWARE\Classes\oldfile\shell\open\command") }, high, None),
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
        g.reasons = vec![REASON_SIBLING_OWNED.into(), REASON_OTHER_PROFILE.into()];
        let uncertain = to_leftovers(&g, tasks).items;
        let dir = uncertain.iter().find(|l| matches!(l.kind, LeftoverKind::Dir { .. })).unwrap();
        assert_eq!((dir.confidence, dir.note.as_deref()), (Confidence::Low, Some(REASON_SIBLING_OWNED)));
        // Folder and vendor keys drop to Low; the seven dangling references stay High.
        let count = |c: Confidence| uncertain.iter().filter(|l| l.confidence == c).count();
        assert_eq!((count(Confidence::High), count(Confidence::Medium), count(Confidence::Low)), (7, 0, 3));
    }

    #[test]
    fn uncertain_ghost_registry_keys_are_low() {
        let vendor = key(Hive::CurrentUser, r"Software\Binary Fortress Software");
        let mut g = ghost_with(
            "Binary Fortress Software",
            vec![
                Evidence::RegKeyOrphan { key: vendor.clone() },
                Evidence::RegKeyMissingPath { key: vendor.clone(), value_name: "InstallDir".into(), path: r"C:\Gone".into() },
                run(r"C:\Program Files\Binary Fortress Software\gone.exe", "BFS"),
            ],
        );
        g.assessment = Assessment::Uncertain;
        g.reasons = vec![REASON_PUBLISHER_OF_INSTALLED.into()];
        let items: Vec<(LeftoverKind, Confidence, Option<String>)> =
            to_leftovers(&g, Path::new(r"C:\T")).items.into_iter().map(|l| (l.kind, l.confidence, l.note)).collect();
        assert_eq!(
            items,
            vec![
                (LeftoverKind::RegValue { key: key(Hive::CurrentUser, RUN), name: "BFS".into() }, Confidence::High, None),
                (LeftoverKind::RegKey { key: vendor }, Confidence::Low, Some(REASON_PUBLISHER_OF_INSTALLED.into())),
            ]
        );
    }

    fn ghost_with(name: &str, evidence: Vec<Evidence>) -> Ghost {
        Ghost {
            name: name.into(),
            key: normalize_name(name),
            publisher: None,
            evidence,
            assessment: Assessment::ProbablyUninstalled,
            total_size: 0,
            last_used: None,
            reasons: Vec::new(),
            note: None,
        }
    }

    #[test]
    fn shell_command_removes_command_key_unless_progid_is_the_ghosts() {
        let target = r"C:\Program Files\Google\Drive File Stream\117.0.0.0\GoogleDriveFS.exe";
        let g = ghost_with(
            "GoogleDriveFS",
            vec![
                dangling(RefKind::ShellCommand, r"CurrentUser\Software\Classes\http", target),
                dangling(RefKind::ShellCommand, r"CurrentUser\Software\Classes\GoogleDriveFS.gdoc", target),
            ],
        );
        let keys: Vec<LeftoverKind> = to_leftovers(&g, Path::new(r"C:\T")).items.into_iter().map(|l| l.kind).collect();
        let hkcu = |p: &str| LeftoverKind::RegKey { key: key(Hive::CurrentUser, p) };
        assert_eq!(
            keys,
            vec![hkcu(r"Software\Classes\http\shell\open\command"), hkcu(r"Software\Classes\GoogleDriveFS.gdoc")]
        );
    }

    #[test]
    fn programdata_only_folder_is_uncertain_data_only() {
        let r = roots(&[]);
        let ev = vec![
            folder(r"C:\ProgramData\Vendor Cache", 10, RootKind::ProgramData),
            folder(r"C:\Users\A\AppData\Local\Vendor Cache", 10, RootKind::AppData),
        ];
        let ghosts = group(ev, &installed(&[]), &KeepList::default(), &r, None);
        assert_eq!(summary(&ghosts), vec![("Vendor Cache".to_string(), Assessment::Uncertain)]);
        assert_eq!(reasons(&ghosts[0]), vec![REASON_DATA_ONLY]);
    }

    // Regressions from a real machine.

    #[test]
    fn publisher_of_installed_program_is_uncertain() {
        let programs = [program("DisplayFusion", Some("Binary Fortress Software"), Some(r"C:\Program Files\DisplayFusion"))];
        let r = roots(&programs);
        let ev = vec![
            folder(r"C:\Users\A\AppData\Local\Binary Fortress Software", 598, RootKind::AppData),
            folder(r"C:\ProgramData\Binary Fortress Software", 4, RootKind::ProgramData),
        ];
        let ghosts = group(ev, &installed(&programs), &KeepList::default(), &r, Some(Path::new(r"C:\Users\A")));
        assert_eq!(summary(&ghosts), vec![("Binary Fortress Software".to_string(), Assessment::Uncertain)]);
        assert_eq!(reasons(&ghosts[0]), vec![REASON_PUBLISHER_OF_INSTALLED, REASON_DATA_ONLY]);
    }

    #[test]
    fn folder_named_like_a_profile_is_dropped() {
        let r = roots_for(&[r"C:\Users\A", r"C:\Users\Jobb"], &[]);
        let ev = vec![folder(r"C:\ProgramData\Jobb", 505, RootKind::ProgramData), pf(r"C:\Program Files\Old Tool", 1)];
        let ghosts = group(ev, &installed(&[]), &KeepList::default(), &r, None);
        assert_eq!(summary(&ghosts), vec![("Old Tool".to_string(), Assessment::LeftoversOnly)]);
    }

    #[test]
    fn installed_programs_own_dangling_refs_are_dropped() {
        let mut drive = program("Google Drive", Some("Google LLC"), None);
        drive.display_icon = Some(PathBuf::from(r"C:\Program Files\Google\Drive File Stream\132.0.0.0\GoogleDriveFS.exe"));
        let programs = [drive];
        let r = roots(&programs);
        let old = r"C:\Program Files\Google\Drive File Stream\117.0.0.0\GoogleDriveFS.exe";
        let ev = vec![
            dangling(RefKind::ShellCommand, r"CurrentUser\Software\Classes\GoogleDriveFS.gdoc", old),
            dangling(RefKind::ClassesServer, r"CurrentUser\Software\Classes\CLSID\{1}", old),
            dangling(RefKind::ShellCommand, r"CurrentUser\Software\Classes\gonefile", r"C:\Program Files\Gone App\g.exe"),
        ];
        let ghosts = group(ev, &installed(&programs), &KeepList::default(), &r, None);
        assert_eq!(summary(&ghosts), vec![("Gone App".to_string(), Assessment::ProbablyUninstalled)]);
    }
}
