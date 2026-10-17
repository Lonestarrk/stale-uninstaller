//! Decides each program's main exe and turns raw usage hits into a per-program "last used".

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{UsageHit, UsageSource};
use crate::model::{Program, ProgramId};
use crate::pathutil::{drive_letter, exe_under, first_path_in_command, is_shared_root, norm, normed_under};
use crate::shortcuts::ShortcutTarget;

/// Lowercased file-stem fragments that mark an exe as background machinery, not the program itself.
/// `unins` covers `unins000` and `uninstall`: running the uninstaller is not using the program.
pub const EXCLUDED_NAME_PARTS: [&str; 9] = ["update", "updater", "service", "helper", "crash", "report", "daemon", "tray", "unins"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExeUsage {
    pub exe_path: PathBuf,
    pub last_run: DateTime<Utc>,
    pub source: UsageSource,
    pub run_count: Option<u32>,
    pub is_main: bool,
    pub excluded: Option<ExcludeReason>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExcludeReason {
    NamePattern,
    Service,
    NotMain,
}

/// `confident` is false only when the date comes from file timestamps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LastUsed {
    pub when: Option<DateTime<Utc>>,
    pub source: Option<UsageSource>,
    pub confident: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProgramUsage {
    pub last_used: LastUsed,
    /// Sorted by `last_run`, newest first.
    pub exes: Vec<ExeUsage>,
}

pub struct MatchInput<'a> {
    pub programs: &'a [Program],
    pub hits: &'a [UsageHit],
    pub shortcut_targets: &'a [ShortcutTarget],
    pub root_exes: &'a HashMap<ProgramId, Vec<(PathBuf, u64)>>,
    /// Per program: exes under its install location named in the arguments of its
    /// name-excluded shortcut targets (see `main_exes`, tier 2).
    pub arg_exes: &'a HashMap<ProgramId, Vec<PathBuf>>,
    pub service_exes: &'a HashSet<String>,
}

pub struct MatchResult {
    pub by_program: HashMap<ProgramId, ProgramUsage>,
    /// Hits that belong to no program (input for the "ghost programs" view).
    pub unmatched: Vec<UsageHit>,
}

fn is_exe(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
}

/// Uppercased file name, used for name-only matching.
fn file_name_upper(p: &Path) -> Option<String> {
    p.file_name().map(|n| n.to_string_lossy().to_uppercase())
}

/// `"C:\x\app.exe",0` -> `C:\x\app.exe`; paths without an icon index are only unquoted.
fn strip_icon_index(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    let s = match s.rsplit_once(',') {
        Some((head, idx)) if idx.trim().trim_start_matches('-').parse::<u32>().is_ok() => head,
        _ => &s,
    };
    PathBuf::from(s.trim().trim_matches('"'))
}

/// Keeps the first of each case-insensitively equal path.
fn dedupe(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths.into_iter().filter(|p| seen.insert(norm(p))).collect()
}

/// The exe(s) that represent the program itself, first non-empty tier wins:
/// 1. shortcut targets under the install location that are not name-excluded;
/// 2. for name-excluded shortcut targets under it (Squirrel's `Update.exe --processStart
///    App.exe`), the exes in `arg_exes` (found under the install location by the caller)
///    named in their arguments (see `arg_exe_names`);
/// 3. `display_icon` if it is a non-uninstaller exe under the install location;
/// 4. the largest non-name-excluded exe in the install root.
///
/// Empty without a (non-shared) install location.
pub fn main_exes(
    p: &Program,
    shortcut_targets: &[ShortcutTarget],
    root_exes: &[(PathBuf, u64)],
    arg_exes: &[PathBuf],
) -> Vec<PathBuf> {
    let Some(install) = p.install_location.as_deref().filter(|i| !is_shared_root(i)) else {
        return vec![];
    };
    let (excluded, ok): (Vec<&ShortcutTarget>, Vec<&ShortcutTarget>) =
        shortcut_targets.iter().filter(|t| exe_under(&t.path, install)).partition(|t| is_name_excluded(&t.path));
    let from_shortcuts = dedupe(ok.iter().map(|t| t.path.clone()));
    if !from_shortcuts.is_empty() {
        return from_shortcuts;
    }
    let wanted: HashSet<String> = excluded.iter().flat_map(|t| arg_exe_names(&t.arguments)).collect();
    let from_args = dedupe(
        arg_exes
            .iter()
            .filter(|e| exe_under(e, install) && file_name_upper(e).is_some_and(|n| wanted.contains(&n)))
            .filter(|e| !is_name_excluded(e))
            .cloned(),
    );
    if !from_args.is_empty() {
        return from_args;
    }
    let icon = p.display_icon.as_deref().map(strip_icon_index);
    if let Some(icon) = icon.filter(|i| is_exe(i) && exe_under(i, install) && !is_uninstaller(i)) {
        return vec![icon];
    }
    root_exes
        .iter()
        .filter(|(exe, _)| !is_name_excluded(exe))
        .max_by_key(|(_, size)| *size)
        .map(|(exe, _)| vec![exe.clone()])
        .unwrap_or_default()
}

/// True when the exe's file stem contains one of `EXCLUDED_NAME_PARTS`.
pub fn is_name_excluded(exe: &Path) -> bool {
    let stem = exe.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    EXCLUDED_NAME_PARTS.iter().any(|part| stem.contains(part))
}

/// Uppercased file names of the `.exe`s named in shortcut arguments, e.g. `DISCORD.EXE` from
/// `--processStart Discord.exe` or `--processStart="C:\x\Discord.exe"`.
pub fn arg_exe_names(arguments: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in arguments.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => tokens.push(std::mem::take(&mut current)),
            c => current.push(c),
        }
    }
    tokens.push(current);
    tokens
        .iter()
        .map(|t| t.rsplit('=').next().unwrap_or(t))
        .map(Path::new)
        .filter(|p| is_exe(p))
        .filter_map(file_name_upper)
        .collect()
}

/// `unins000.exe`, `Uninstall Foo.exe`, …
fn is_uninstaller(exe: &Path) -> bool {
    exe.file_stem().is_some_and(|s| s.to_string_lossy().to_lowercase().contains("unins"))
}

pub fn exclude_reason(exe: &Path, is_main: bool, service_exes: &HashSet<String>) -> Option<ExcludeReason> {
    if is_name_excluded(exe) {
        Some(ExcludeReason::NamePattern)
    } else if service_exes.contains(&norm(exe)) {
        Some(ExcludeReason::Service)
    } else if !is_main {
        Some(ExcludeReason::NotMain)
    } else {
        None
    }
}

/// A program prepared for matching, with everything normalized once.
struct Candidate<'a> {
    program: &'a Program,
    /// `norm()`ed install location; `None` when missing or a shared root.
    install: Option<String>,
    /// `norm()`ed main exes.
    main: HashSet<String>,
    /// Without an install location: uppercased display-icon exe name (counts as main).
    icon_name: Option<String>,
    /// Without an install location: uppercased uninstaller exe name (never MsiExec).
    uninstaller_name: Option<String>,
}

impl<'a> Candidate<'a> {
    fn new(program: &'a Program, input: &MatchInput) -> Self {
        let roots = input.root_exes.get(&program.id).map(Vec::as_slice).unwrap_or_default();
        let args = input.arg_exes.get(&program.id).map(Vec::as_slice).unwrap_or_default();
        let main = main_exes(program, input.shortcut_targets, roots, args).iter().map(|e| norm(e)).collect();
        let install = program.install_location.as_deref().filter(|i| !is_shared_root(i)).map(norm);
        let (mut icon_name, mut uninstaller_name) = (None, None);
        if program.install_location.is_none() {
            icon_name = program.display_icon.as_deref().map(strip_icon_index).filter(|i| is_exe(i)).and_then(|i| file_name_upper(&i));
            uninstaller_name = program
                .uninstall_string
                .as_deref()
                .and_then(first_path_in_command)
                .filter(|u| is_exe(u))
                .and_then(|u| file_name_upper(&u))
                .filter(|n| n != "MSIEXEC.EXE");
        }
        Candidate { program, install, main, icon_name, uninstaller_name }
    }

    fn is_main(&self, exe: &Path) -> bool {
        self.main.contains(&norm(exe)) || (self.icon_name.is_some() && file_name_upper(exe) == self.icon_name)
    }

    fn claims_by_name(&self, name: &Option<String>) -> bool {
        name.is_some() && (*name == self.icon_name || *name == self.uninstaller_name)
    }
}

/// Picks the program(s) a hit belongs to and the path to record for each. By location, the
/// longest install location wins and every program tied for it gets the hit (duplicate
/// entries, or `C:` and `D:` installs for a drive-less Prefetch path), so none of them looks
/// unused. Otherwise, by file name, the first program without an install location that claims it.
fn assign_hit(hit: &UsageHit, candidates: &[Candidate]) -> Vec<(usize, PathBuf)> {
    let exe = norm(&hit.exe_path);
    let under: Vec<(usize, usize)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.install.as_deref().filter(|inst| normed_under(&exe, inst)).map(|inst| (i, inst.len())))
        .collect();
    if let Some(longest) = under.iter().map(|&(_, len)| len).max() {
        return under
            .into_iter()
            .filter(|&(_, len)| len == longest)
            .map(|(i, _)| (i, with_install_drive(&hit.exe_path, candidates[i].program)))
            .collect();
    }
    let name = file_name_upper(&hit.exe_path);
    candidates.iter().position(|c| c.claims_by_name(&name)).map(|i| (i, hit.exe_path.clone())).into_iter().collect()
}

/// Gives a drive-less (volume-stripped Prefetch) path the install location's drive.
fn with_install_drive(exe: &Path, program: &Program) -> PathBuf {
    let drive = program.install_location.as_deref().and_then(drive_letter);
    match drive {
        Some(d) if exe.to_string_lossy().starts_with('\\') => PathBuf::from(format!("{d}:{}", exe.display())),
        _ => exe.to_path_buf(),
    }
}

/// One `ExeUsage` per distinct exe (by `norm()`), newest first. Callers never mix FileTime
/// hits with other sources in one call (see `summarize`).
fn group_hits(hits: Vec<UsageHit>, is_main: impl Fn(&Path) -> bool, service_exes: &HashSet<String>) -> Vec<ExeUsage> {
    let mut groups: BTreeMap<String, Vec<UsageHit>> = BTreeMap::new();
    for hit in hits {
        groups.entry(norm(&hit.exe_path)).or_default().push(hit);
    }
    let mut exes: Vec<ExeUsage> = groups
        .into_values()
        .filter_map(|group| {
            let latest = group.iter().max_by_key(|h| h.last_run)?;
            let main = is_main(&latest.exe_path);
            // UserAssist records launches by the user, so such an exe counts even when it is not
            // a main exe or has a background-looking name; running an uninstaller never does.
            let user_launched =
                group.iter().any(|h| h.source == UsageSource::UserAssist) && !is_uninstaller(&latest.exe_path);
            Some(ExeUsage {
                exe_path: latest.exe_path.clone(),
                last_run: latest.last_run,
                source: latest.source,
                run_count: group.iter().filter_map(|h| h.run_count).max(),
                is_main: main,
                excluded: if user_launched { None } else { exclude_reason(&latest.exe_path, main, service_exes) },
            })
        })
        .collect();
    exes.sort_by_key(|e| std::cmp::Reverse(e.last_run));
    exes
}

fn last_used<'a>(exes: impl IntoIterator<Item = &'a ExeUsage>) -> LastUsed {
    exes.into_iter()
        .filter(|e| e.excluded.is_none())
        .max_by_key(|e| e.last_run)
        .map(|e| LastUsed { when: Some(e.last_run), source: Some(e.source), confident: e.source != UsageSource::FileTime })
        .unwrap_or(LastUsed { when: None, source: None, confident: true })
}

/// Summarizes one program's hits (spec §5). File timestamps are a pure fallback: they never
/// merge into an exe that has a real source, and they date the program only when it has no
/// other hits at all (excluded ones included).
fn summarize(hits: Vec<UsageHit>, is_main: impl Fn(&Path) -> bool, service_exes: &HashSet<String>) -> ProgramUsage {
    let (file_times, real): (Vec<UsageHit>, Vec<UsageHit>) = hits.into_iter().partition(|h| h.source == UsageSource::FileTime);
    let real = group_hits(real, &is_main, service_exes);
    let real_exes: HashSet<String> = real.iter().map(|e| norm(&e.exe_path)).collect();
    let file_times: Vec<ExeUsage> = group_hits(file_times, &is_main, service_exes)
        .into_iter()
        .filter(|e| !real_exes.contains(&norm(&e.exe_path)))
        .collect();
    let last_used = if real.is_empty() { last_used(&file_times) } else { last_used(&real) };
    let mut exes = real;
    exes.extend(file_times);
    exes.sort_by_key(|e| std::cmp::Reverse(e.last_run));
    ProgramUsage { last_used, exes }
}

/// Assigns hits to programs (see `assign_hit`) and summarizes each program's usage.
/// Every program gets an entry; hits no program claims are returned in `unmatched`.
pub fn match_usage(input: &MatchInput) -> MatchResult {
    let candidates: Vec<Candidate> = input.programs.iter().map(|p| Candidate::new(p, input)).collect();
    let mut assigned: Vec<Vec<UsageHit>> = vec![Vec::new(); candidates.len()];
    let mut unmatched = Vec::new();
    for hit in input.hits {
        let owners = assign_hit(hit, &candidates);
        if owners.is_empty() {
            unmatched.push(hit.clone());
        }
        for (i, exe_path) in owners {
            assigned[i].push(UsageHit { exe_path, ..hit.clone() });
        }
    }
    let by_program = candidates
        .iter()
        .zip(assigned)
        .map(|(c, hits)| (c.program.id.clone(), summarize(hits, |e| c.is_main(e), input.service_exes)))
        .collect();
    MatchResult { by_program, unmatched }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};

    use super::*;
    use crate::model::{Hive, Scope};
    use crate::registry::key;

    fn prog(id: &str, install: Option<&str>) -> Program {
        Program {
            id: ProgramId(id.to_string()),
            key: key(Hive::LocalMachine, id),
            name: id.to_string(),
            publisher: None,
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

    fn days_ago(d: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap() - Duration::days(d)
    }

    fn hit(path: &str, days: i64, source: UsageSource) -> UsageHit {
        UsageHit { exe_path: PathBuf::from(path), last_run: days_ago(days), source, run_count: None }
    }

    fn paths(ps: &[&str]) -> Vec<PathBuf> {
        ps.iter().map(PathBuf::from).collect()
    }

    fn targets(ps: &[&str]) -> Vec<ShortcutTarget> {
        ps.iter().map(ShortcutTarget::new).collect()
    }

    fn env_path(var: &str) -> PathBuf {
        PathBuf::from(std::env::var(var).unwrap())
    }

    /// Runs matching with Foo's shortcut `C:\Apps\Foo\foo.exe` and no root exes.
    fn run(programs: &[Program], hits: &[UsageHit], services: &HashSet<String>) -> MatchResult {
        let shortcuts = targets(&[r"C:\Apps\Foo\foo.exe"]);
        let roots = HashMap::new();
        match_usage(&MatchInput { programs, hits, shortcut_targets: &shortcuts, root_exes: &roots, arg_exes: &HashMap::new(), service_exes: services })
    }

    fn usage<'a>(r: &'a MatchResult, id: &str) -> &'a ProgramUsage {
        &r.by_program[&ProgramId(id.to_string())]
    }

    #[test]
    fn main_exe_prefers_shortcut_then_icon_then_largest() {
        let mut a = prog("A", Some(r"C:\Apps\A"));
        a.display_icon = Some(PathBuf::from(r"C:\Apps\A\icon.exe"));
        let shortcuts = targets(&[r"C:\Apps\A\a.exe", r"c:\apps\a\A.EXE", r"C:\Apps\A\bin\tool.exe", r"C:\Other\z.exe"]);
        let roots = vec![(PathBuf::from(r"C:\Apps\A\big.exe"), 100)];
        assert_eq!(main_exes(&a, &shortcuts, &roots, &[]), paths(&[r"C:\Apps\A\a.exe", r"C:\Apps\A\bin\tool.exe"]));

        let mut b = prog("B", Some(r"C:\Apps\B"));
        b.display_icon = Some(PathBuf::from(r"C:\Apps\B\b.EXE,0"));
        assert_eq!(main_exes(&b, &shortcuts, &roots, &[]), paths(&[r"C:\Apps\B\b.EXE"]));
        b.display_icon = Some(PathBuf::from(r"C:\Apps\B\b.exe"));
        assert_eq!(main_exes(&b, &shortcuts, &roots, &[]), paths(&[r"C:\Apps\B\b.exe"]));

        let mut c = prog("C", Some(r"C:\Apps\C"));
        c.display_icon = Some(PathBuf::from(r"C:\Apps\C\c.ico"));
        let roots = vec![(PathBuf::from(r"C:\Apps\C\small.exe"), 10), (PathBuf::from(r"C:\Apps\C\big.exe"), 99)];
        assert_eq!(main_exes(&c, &shortcuts, &roots, &[]), paths(&[r"C:\Apps\C\big.exe"]));
        assert_eq!(main_exes(&c, &shortcuts, &[], &[]), Vec::<PathBuf>::new());
    }

    #[test]
    fn main_exe_ignores_display_icon_outside_install_location() {
        // MSI programs often point DisplayIcon at an icon-only exe under C:\Windows\Installer.
        let mut p = prog("P", Some(r"C:\Apps\P"));
        p.display_icon = Some(PathBuf::from(r"C:\Windows\Installer\{1234}\ARPPRODUCTICON.exe"));
        let roots = vec![(PathBuf::from(r"C:\Apps\P\p.exe"), 5)];
        assert_eq!(main_exes(&p, &[], &roots, &[]), paths(&[r"C:\Apps\P\p.exe"]));
    }

    #[test]
    fn main_exe_empty_for_shared_root_install_location() {
        let pf = env_path("ProgramFiles");
        let mut p = prog("P", Some(pf.to_str().unwrap()));
        p.display_icon = Some(pf.join("Foo").join("foo.exe"));
        let shortcuts = vec![ShortcutTarget::new(pf.join("Foo").join("foo.exe"))];
        let roots = vec![(pf.join("x.exe"), 5)];
        assert!(main_exes(&p, &shortcuts, &roots, &[]).is_empty());
        let mut none = prog("N", None);
        none.display_icon = Some(pf.join("Foo").join("foo.exe"));
        assert!(main_exes(&none, &shortcuts, &roots, &[]).is_empty());
    }

    #[test]
    fn exclude_reason_checks_name_then_service_then_main() {
        let exe = Path::new(r"C:\Apps\Foo\FooUpdater.exe");
        let services = HashSet::from([norm(exe), norm(Path::new(r"C:\Apps\Foo\agent.exe"))]);
        assert_eq!(exclude_reason(exe, true, &services), Some(ExcludeReason::NamePattern));
        assert_eq!(exclude_reason(Path::new(r"C:\Apps\Foo\CrashReporter.exe"), true, &HashSet::new()), Some(ExcludeReason::NamePattern));
        assert_eq!(exclude_reason(Path::new(r"C:\Apps\Foo\AGENT.EXE"), true, &services), Some(ExcludeReason::Service));
        assert_eq!(exclude_reason(Path::new(r"C:\Apps\Foo\other.exe"), false, &services), Some(ExcludeReason::NotMain));
        assert_eq!(exclude_reason(Path::new(r"C:\Apps\Foo\foo.exe"), true, &services), None);
        // Only the stem is checked: a folder name containing "update" does not exclude.
        assert_eq!(exclude_reason(Path::new(r"C:\Apps\Updates\foo.exe"), true, &services), None);
    }

    #[test]
    fn updater_hit_does_not_set_last_used_but_appears_in_exes() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let hits = [hit(r"C:\Apps\Foo\FooUpdater.exe", 1, UsageSource::Bam), hit(r"C:\Apps\Foo\foo.exe", 200, UsageSource::Bam)];
        let r = run(&programs, &hits, &HashSet::new());
        let u = usage(&r, "Foo");
        assert_eq!(u.last_used, LastUsed { when: Some(days_ago(200)), source: Some(UsageSource::Bam), confident: true });
        assert_eq!(u.exes.len(), 2);
        assert_eq!(u.exes[0].exe_path, PathBuf::from(r"C:\Apps\Foo\FooUpdater.exe"));
        assert_eq!(u.exes[0].excluded, Some(ExcludeReason::NamePattern));
        assert!(!u.exes[0].is_main);
        assert_eq!(u.exes[1].excluded, None);
        assert!(u.exes[1].is_main);
        assert!(r.unmatched.is_empty());
    }

    #[test]
    fn service_exe_is_excluded() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let hits = [hit(r"C:\Apps\Foo\foo.exe", 3, UsageSource::Bam)];
        let services = HashSet::from([norm(Path::new(r"C:\Apps\Foo\foo.exe"))]);
        let u = &run(&programs, &hits, &services).by_program[&ProgramId("Foo".into())];
        assert_eq!(u.exes[0].excluded, Some(ExcludeReason::Service));
        assert_eq!(u.last_used, LastUsed { when: None, source: None, confident: true });
    }

    #[test]
    fn userassist_hit_counts_even_for_non_main_exe() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let hits = [
            hit(r"C:\Apps\Foo\tools\other.exe", 5, UsageSource::UserAssist),
            hit(r"C:\Apps\Foo\tools\other.exe", 9, UsageSource::Bam),
            hit(r"C:\Apps\Foo\foo.exe", 50, UsageSource::Bam),
        ];
        let r = run(&programs, &hits, &HashSet::new());
        let u = usage(&r, "Foo");
        assert_eq!(u.last_used, LastUsed { when: Some(days_ago(5)), source: Some(UsageSource::UserAssist), confident: true });
        assert_eq!(u.exes.len(), 2);
        assert!(!u.exes[0].is_main);
        assert_eq!(u.exes[0].excluded, None);
    }

    #[test]
    fn userassist_overrides_name_pattern_too() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let hits = [hit(r"C:\Apps\Foo\FooHelper.exe", 2, UsageSource::UserAssist)];
        let r = run(&programs, &hits, &HashSet::new());
        assert_eq!(usage(&r, "Foo").exes[0].excluded, None);
        assert_eq!(usage(&r, "Foo").last_used.when, Some(days_ago(2)));
    }

    #[test]
    fn userassist_uninstaller_hit_does_not_count() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let hits = [hit(r"C:\Apps\Foo\unins000.exe", 2, UsageSource::UserAssist)];
        let r = run(&programs, &hits, &HashSet::new());
        assert_eq!(usage(&r, "Foo").exes[0].excluded, Some(ExcludeReason::NamePattern));
        assert_eq!(usage(&r, "Foo").last_used.when, None);
    }

    #[test]
    fn filetime_only_gives_unconfident_date() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let hits = [hit(r"C:\Apps\Foo\foo.exe", 30, UsageSource::FileTime)];
        let r = run(&programs, &hits, &HashSet::new());
        assert_eq!(usage(&r, "Foo").last_used, LastUsed { when: Some(days_ago(30)), source: Some(UsageSource::FileTime), confident: false });
    }

    #[test]
    fn shared_root_install_location_claims_nothing() {
        let pf = env_path("ProgramFiles");
        let programs = [prog("Shared", Some(pf.to_str().unwrap()))];
        let exe = pf.join("Other").join("x.exe");
        let hits = [hit(exe.to_str().unwrap(), 1, UsageSource::Bam)];
        let r = run(&programs, &hits, &HashSet::new());
        assert!(usage(&r, "Shared").exes.is_empty());
        assert_eq!(r.unmatched, hits.to_vec());
    }

    #[test]
    fn no_install_location_matches_on_display_icon_file_name() {
        let mut p = prog("Bar", None);
        p.display_icon = Some(PathBuf::from(r"C:\x\bar.exe"));
        let hits = [hit(r"\X\BAR.EXE", 4, UsageSource::Prefetch)];
        let r = run(&[p], &hits, &HashSet::new());
        let u = usage(&r, "Bar");
        assert_eq!(u.exes.len(), 1);
        assert_eq!(u.exes[0].exe_path, PathBuf::from(r"\X\BAR.EXE"));
        // With no install location the display-icon exe is the best main-exe evidence there is.
        assert!(u.exes[0].is_main);
        assert_eq!(u.last_used.when, Some(days_ago(4)));
        assert!(r.unmatched.is_empty());
    }

    #[test]
    fn no_install_location_matches_on_uninstall_string_file_name_as_not_main() {
        let mut p = prog("Baz", None);
        p.uninstall_string = Some(r#""C:\x\remove.exe" /SILENT"#.to_string());
        let hits = [hit(r"D:\elsewhere\REMOVE.EXE", 4, UsageSource::Bam)];
        let r = run(&[p], &hits, &HashSet::new());
        let u = usage(&r, "Baz");
        assert_eq!(u.exes.len(), 1);
        assert_eq!(u.exes[0].excluded, Some(ExcludeReason::NotMain));
        assert_eq!(u.last_used.when, None);
    }

    #[test]
    fn msi_uninstall_string_does_not_claim_msiexec() {
        let mut p = prog("Msi", None);
        p.is_msi = true;
        p.uninstall_string = Some("MsiExec.exe /X{ABC-123}".to_string());
        let hits = [hit(r"C:\Windows\System32\msiexec.exe", 1, UsageSource::Bam)];
        let r = run(&[p], &hits, &HashSet::new());
        assert!(usage(&r, "Msi").exes.is_empty());
        assert_eq!(r.unmatched.len(), 1);
    }

    #[test]
    fn nested_install_locations_prefer_longest() {
        let programs = [prog("Outer", Some(r"C:\Apps\Suite")), prog("Inner", Some(r"C:\Apps\Suite\Plugin"))];
        let hits = [hit(r"C:\Apps\Suite\Plugin\plug.exe", 1, UsageSource::Bam), hit(r"C:\Apps\Suite\suite.exe", 2, UsageSource::Bam)];
        let r = run(&programs, &hits, &HashSet::new());
        assert_eq!(usage(&r, "Inner").exes.len(), 1);
        assert_eq!(usage(&r, "Inner").exes[0].exe_path, PathBuf::from(r"C:\Apps\Suite\Plugin\plug.exe"));
        assert_eq!(usage(&r, "Outer").exes.len(), 1);
        assert_eq!(usage(&r, "Outer").exes[0].exe_path, PathBuf::from(r"C:\Apps\Suite\suite.exe"));
    }

    #[test]
    fn prefetch_hit_gets_install_drive_and_merges_with_bam() {
        let programs = [prog("Foo", Some(r"D:\Apps\Foo"))];
        let mut pf = hit(r"\APPS\FOO\FOO.EXE", 10, UsageSource::Prefetch);
        pf.run_count = Some(7);
        let mut pf_old = hit(r"\APPS\FOO\FOO.EXE", 40, UsageSource::Prefetch);
        pf_old.run_count = Some(3);
        let bam = hit(r"D:\Apps\Foo\foo.exe", 2, UsageSource::Bam);
        let only_pf = hit(r"\APPS\FOO\BIN\TOOL.EXE", 20, UsageSource::Prefetch);
        let r = run(&programs, &[pf, pf_old, bam, only_pf], &HashSet::new());
        let u = usage(&r, "Foo");
        assert_eq!(u.exes.len(), 2);
        assert_eq!(u.exes[0].exe_path, PathBuf::from(r"D:\Apps\Foo\foo.exe"));
        assert_eq!(u.exes[0].last_run, days_ago(2));
        assert_eq!(u.exes[0].source, UsageSource::Bam);
        assert_eq!(u.exes[0].run_count, Some(7));
        assert_eq!(u.exes[1].exe_path, PathBuf::from(r"D:\APPS\FOO\BIN\TOOL.EXE"));
        assert_eq!(u.exes[1].run_count, None);
    }

    #[test]
    fn equal_install_locations_both_get_the_hit() {
        let programs = [prog("Suite", Some(r"C:\Apps\Foo")), prog("Component", Some(r"c:\apps\foo\"))];
        let hits = [hit(r"C:\Apps\Foo\foo.exe", 3, UsageSource::Bam)];
        let r = run(&programs, &hits, &HashSet::new());
        for id in ["Suite", "Component"] {
            assert_eq!(usage(&r, id).last_used.when, Some(days_ago(3)), "{id}");
            assert_eq!(usage(&r, id).exes[0].exe_path, PathBuf::from(r"C:\Apps\Foo\foo.exe"), "{id}");
        }
        assert!(r.unmatched.is_empty());
    }

    #[test]
    fn drive_less_hit_goes_to_every_drive_with_its_own_drive() {
        let programs = [prog("OnC", Some(r"C:\Apps\Foo")), prog("OnD", Some(r"D:\Apps\Foo"))];
        let hits = [hit(r"\APPS\FOO\BAR.EXE", 6, UsageSource::Prefetch)];
        let r = run(&programs, &hits, &HashSet::new());
        assert_eq!(usage(&r, "OnC").exes[0].exe_path, PathBuf::from(r"C:\APPS\FOO\BAR.EXE"));
        assert_eq!(usage(&r, "OnD").exes[0].exe_path, PathBuf::from(r"D:\APPS\FOO\BAR.EXE"));
        assert!(r.unmatched.is_empty());
    }

    #[test]
    fn uninstaller_shortcut_is_not_main() {
        let mut p = prog("Foo", Some(r"C:\Apps\Foo"));
        p.display_icon = Some(PathBuf::from(r"C:\Apps\Foo\uninstall.exe,0"));
        let shortcuts = targets(&[r"C:\Apps\Foo\unins000.exe", r"C:\Apps\Foo\Uninstall Foo.exe"]);
        let roots = vec![(PathBuf::from(r"C:\Apps\Foo\unins000.exe"), 900), (PathBuf::from(r"C:\Apps\Foo\foo.exe"), 100)];
        assert_eq!(main_exes(&p, &shortcuts, &roots, &[]), paths(&[r"C:\Apps\Foo\foo.exe"]));
        let services = HashSet::new();
        assert_eq!(exclude_reason(Path::new(r"C:\Apps\Foo\unins000.exe"), true, &services), Some(ExcludeReason::NamePattern));
        assert_eq!(exclude_reason(Path::new(r"C:\Apps\Foo\Uninstall.exe"), true, &services), Some(ExcludeReason::NamePattern));
        let hits = [hit(r"C:\Apps\Foo\unins000.exe", 1, UsageSource::Bam)];
        let r = match_usage(&MatchInput {
            programs: &[p],
            hits: &hits,
            shortcut_targets: &shortcuts,
            root_exes: &HashMap::from([(ProgramId("Foo".into()), roots)]),
            arg_exes: &HashMap::new(),
            service_exes: &services,
        });
        assert_eq!(usage(&r, "Foo").last_used.when, None);
    }

    #[test]
    fn unmatched_hits_are_returned() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let hits = [hit(r"C:\Nowhere\x.exe", 1, UsageSource::Bam)];
        let r = run(&programs, &hits, &HashSet::new());
        assert_eq!(r.unmatched, hits.to_vec());
        assert_eq!(usage(&r, "Foo"), &ProgramUsage { last_used: LastUsed { when: None, source: None, confident: true }, exes: vec![] });
    }

    #[test]
    fn squirrel_update_exe_falls_through_to_process_start_target() {
        let p = prog("Discord", Some(r"C:\Users\X\AppData\Local\Discord"));
        let update = PathBuf::from(r"C:\Users\X\AppData\Local\Discord\Update.exe");
        let shortcuts = [ShortcutTarget { path: update.clone(), arguments: "--processStart Discord.exe".into() }];
        let main = PathBuf::from(r"C:\Users\X\AppData\Local\Discord\app-1.0\Discord.exe");
        let roots = vec![(update.clone(), 2_000_000)];
        assert_eq!(main_exes(&p, &shortcuts, &roots, std::slice::from_ref(&main)), vec![main.clone()]);
        assert_eq!(exclude_reason(&update, false, &HashSet::new()), Some(ExcludeReason::NamePattern));

        let hits = [hit(main.to_str().unwrap(), 3, UsageSource::Bam), hit(update.to_str().unwrap(), 1, UsageSource::Bam)];
        let r = match_usage(&MatchInput {
            programs: &[p],
            hits: &hits,
            shortcut_targets: &shortcuts,
            root_exes: &HashMap::from([(ProgramId("Discord".into()), roots)]),
            arg_exes: &HashMap::from([(ProgramId("Discord".into()), vec![main.clone()])]),
            service_exes: &HashSet::new(),
        });
        let u = usage(&r, "Discord");
        assert_eq!(u.last_used, LastUsed { when: Some(days_ago(3)), source: Some(UsageSource::Bam), confident: true });
        let discord = u.exes.iter().find(|e| e.exe_path == main).unwrap();
        assert!(discord.is_main && discord.excluded.is_none());
    }

    #[test]
    fn arg_exe_names_finds_exe_tokens() {
        assert_eq!(arg_exe_names("--processStart Discord.exe"), ["DISCORD.EXE"]);
        assert_eq!(arg_exe_names(r#"--processStart="C:\Program Files\App\My App.exe" -x"#), ["MY APP.EXE"]);
        assert!(arg_exe_names("--profile default").is_empty());
        assert!(arg_exe_names("").is_empty());
    }

    #[test]
    fn real_hit_beats_newer_filetime() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let hits = [hit(r"C:\Apps\Foo\foo.exe", 300, UsageSource::Bam), hit(r"C:\Apps\Foo\foo.exe", 1, UsageSource::FileTime)];
        let r = run(&programs, &hits, &HashSet::new());
        let u = usage(&r, "Foo");
        assert_eq!(u.last_used, LastUsed { when: Some(days_ago(300)), source: Some(UsageSource::Bam), confident: true });
        assert_eq!(u.exes.len(), 1);
        assert_eq!((u.exes[0].source, u.exes[0].last_run), (UsageSource::Bam, days_ago(300)));
    }

    #[test]
    fn only_updater_hit_plus_filetime_gives_never_seen() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let hits = [hit(r"C:\Apps\Foo\FooUpdater.exe", 1, UsageSource::Bam), hit(r"C:\Apps\Foo\foo.exe", 1, UsageSource::FileTime)];
        let r = run(&programs, &hits, &HashSet::new());
        let u = usage(&r, "Foo");
        assert_eq!(u.last_used.when, None);
        // The file-time row is still listed on its own.
        assert!(u.exes.iter().any(|e| e.source == UsageSource::FileTime && e.exe_path == Path::new(r"C:\Apps\Foo\foo.exe")));
    }

    #[test]
    fn filetime_used_when_no_other_hits() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let hits = [hit(r"C:\Apps\Foo\foo.exe", 12, UsageSource::FileTime)];
        let r = run(&programs, &hits, &HashSet::new());
        let u = usage(&r, "Foo");
        assert_eq!(u.last_used.when, Some(days_ago(12)));
        assert!(!u.last_used.confident);
    }
}
