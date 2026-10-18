//! Decides each program's main exe and turns raw usage hits into a per-program "last used".

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{UsageHit, UsageSource};
use crate::leftovers::{is_rejected_dir, ProgramHint};
use crate::model::{Program, ProgramId};
use crate::pathutil::{drive_letter, exe_under, first_path_in_command, norm, normed_under};
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
    /// The exe is started automatically: a scheduled-task action, a `Run`/`RunOnce` value or a
    /// Startup-folder shortcut.
    #[serde(default)]
    pub autostart: bool,
    /// The exe would be excluded but the user launched it, so only its UserAssist launches date
    /// it; its other traces (BAM, Prefetch) were dropped.
    #[serde(default)]
    pub user_launch_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExcludeReason {
    NamePattern,
    Service,
    /// The exe is the action of a scheduled task (background maintenance, not a user launch).
    ScheduledTask,
    /// Kept for deserialisation. Only produced for a hit that belongs to a program through a
    /// file-name claim alone (no install folder) and is not its main exe: that is the
    /// uninstaller's or icon's name, not proof of a subcomponent. Inside the install folder a
    /// non-main exe counts.
    NotMain,
}

/// `confident` is false only when the date comes from file timestamps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LastUsed {
    pub when: Option<DateTime<Utc>>,
    pub source: Option<UsageSource>,
    pub confident: bool,
    /// Lowercased exe file name when the date came from a non-main exe of the program (a
    /// subcomponent such as `link.exe`), else `None`.
    #[serde(default)]
    pub via: Option<String>,
    /// The date is a Prefetch/BAM trace of an autostart exe (see [`ExeUsage::autostart`]): the
    /// program was started automatically, not necessarily by the user.
    #[serde(default)]
    pub autostart: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProgramUsage {
    pub last_used: LastUsed,
    /// Sorted by `last_run`, newest first.
    pub exes: Vec<ExeUsage>,
    /// One of the program's main exes ([`program_main_exes`]), whether or not it ran: the one
    /// the display icon names, else the most recently used, else the first. The details panel
    /// reads the signature and version info from it.
    #[serde(default)]
    pub main_exe: Option<PathBuf>,
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
    /// `norm()`ed exe paths of scheduled-task actions.
    pub task_exes: &'a HashSet<String>,
    /// `norm()`ed targets of `Run`/`RunOnce` values and Startup-folder shortcuts (scheduled
    /// tasks are in `task_exes`).
    pub autostart_exes: &'a HashSet<String>,
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
/// 4. the largest exe in the install root that is neither name-excluded nor an installer/setup.
///
/// "Install location" is [`usage_install_dir`]; empty without one.
pub fn main_exes(
    p: &Program,
    shortcut_targets: &[ShortcutTarget],
    root_exes: &[(PathBuf, u64)],
    arg_exes: &[PathBuf],
) -> Vec<PathBuf> {
    main_exes_with_rule(p, shortcut_targets, root_exes, arg_exes).0
}

/// [`main_exes`] that never picks an exe `skip` refuses (services, scheduled-task actions): the
/// rule that would have picked it falls through to the next candidate or tier.
pub fn main_exes_skipping(
    p: &Program,
    shortcut_targets: &[ShortcutTarget],
    root_exes: &[(PathBuf, u64)],
    arg_exes: &[PathBuf],
    skip: &dyn Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    main_exes_rule_skipping(p, shortcut_targets, root_exes, arg_exes, skip).0
}

/// The main exe(s) and rule usage matching really uses: [`main_exes_rule_skipping`] that skips
/// service exes (a `norm()`ed set, see `listed`). A scheduled task may start the main exe
/// (an autostart at logon): the program is then running, so it stays the main exe.
pub fn program_main_exes(
    p: &Program,
    shortcut_targets: &[ShortcutTarget],
    root_exes: &[(PathBuf, u64)],
    arg_exes: &[PathBuf],
    service_exes: &HashSet<String>,
) -> (Vec<PathBuf>, MainRule) {
    main_exes_rule_skipping(p, shortcut_targets, root_exes, arg_exes, &|e| listed(service_exes, e))
}

/// The tier of [`main_exes`] that picked the main exe(s).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MainRule {
    Shortcut,
    ShortcutArguments,
    DisplayIcon,
    LargestRootExe,
    /// No main exe (no usable install location, or nothing found).
    None,
}

/// [`main_exes`] plus the rule that picked them.
pub fn main_exes_with_rule(
    p: &Program,
    shortcut_targets: &[ShortcutTarget],
    root_exes: &[(PathBuf, u64)],
    arg_exes: &[PathBuf],
) -> (Vec<PathBuf>, MainRule) {
    main_exes_rule_skipping(p, shortcut_targets, root_exes, arg_exes, &|_| false)
}

/// [`main_exes_with_rule`] with a `skip` filter, see [`main_exes_skipping`].
pub fn main_exes_rule_skipping(
    p: &Program,
    shortcut_targets: &[ShortcutTarget],
    root_exes: &[(PathBuf, u64)],
    arg_exes: &[PathBuf],
    skip: &dyn Fn(&Path) -> bool,
) -> (Vec<PathBuf>, MainRule) {
    let Some(install) = usage_install_dir(p, shortcut_targets) else {
        return (vec![], MainRule::None);
    };
    let install = install.as_path();
    let (excluded, ok): (Vec<&ShortcutTarget>, Vec<&ShortcutTarget>) =
        shortcut_targets.iter().filter(|t| exe_under(&t.path, install) && !skip(&t.path)).partition(|t| is_name_excluded(&t.path));
    let from_shortcuts = dedupe(ok.iter().map(|t| t.path.clone()));
    if !from_shortcuts.is_empty() {
        return (from_shortcuts, MainRule::Shortcut);
    }
    let wanted: HashSet<String> = excluded.iter().flat_map(|t| arg_exe_names(&t.arguments)).collect();
    let from_args = dedupe(
        arg_exes
            .iter()
            .filter(|e| exe_under(e, install) && file_name_upper(e).is_some_and(|n| wanted.contains(&n)))
            .filter(|e| !is_name_excluded(e) && !skip(e))
            .cloned(),
    );
    if !from_args.is_empty() {
        return (from_args, MainRule::ShortcutArguments);
    }
    let icon = p.display_icon.as_deref().map(strip_icon_index);
    if let Some(icon) = icon.filter(|i| is_exe(i) && exe_under(i, install) && !is_uninstaller(i) && !skip(i)) {
        return (vec![icon], MainRule::DisplayIcon);
    }
    root_exes
        .iter()
        .filter(|(exe, _)| !is_name_excluded(exe) && !is_installer(exe) && !skip(exe))
        .max_by_key(|(_, size)| *size)
        .map_or((vec![], MainRule::None), |(exe, _)| (vec![exe.clone()], MainRule::LargestRootExe))
}

/// The folder usage matching treats as the program's own: [`ProgramHint::effective_install_dir`]
/// (`InstallLocation`, else the uninstaller's or display icon's folder; a folder derived from the
/// uninstaller widens to the display icon's folder when that holds it), else the folder of the
/// target of a Start Menu shortcut named exactly like the program. The last rule is only for MSI
/// entries, which can carry no path at all (Google Chrome's: only `MsiExec.exe /X{…}`); browser
/// web apps ("YouTube" → `chrome_proxy.exe`) are not MSI and never take the browser's folder. A
/// target in a folder [`is_rejected_dir`] refuses (shared root, `%SystemRoot%`, caches, user folders) gives nothing.
pub fn usage_install_dir(p: &Program, shortcut_targets: &[ShortcutTarget]) -> Option<PathBuf> {
    if let Some(dir) = ProgramHint::from_program(p).effective_install_dir() {
        let from_install_location = p.install_location.as_deref().is_some_and(|l| norm(l) == norm(&dir));
        // A derived folder below the display icon's (`Application\141.0\Installer\setup.exe`
        // under `Application\chrome.exe`, `App\uninstall\uninstall.exe` under `App\app.exe`)
        // widens to the display icon's folder.
        let icon_dir = p.display_icon.as_deref().map(strip_icon_index);
        let icon_dir = icon_dir.as_deref().and_then(Path::parent).filter(|d| !is_rejected_dir(d));
        return match icon_dir {
            Some(icon_dir) if !from_install_location && normed_under(&norm(&dir), &norm(icon_dir)) => Some(icon_dir.to_path_buf()),
            _ => Some(dir),
        };
    }
    if !p.is_msi {
        return None;
    }
    let name = p.name.trim();
    shortcut_targets
        .iter()
        .filter(|t| t.lnk.file_stem().is_some_and(|s| s.to_string_lossy().trim().eq_ignore_ascii_case(name)))
        .filter(|t| is_exe(&t.path) && !is_uninstaller(&t.path))
        .filter_map(|t| t.path.parent())
        .find(|dir| !is_rejected_dir(dir))
        .map(Path::to_path_buf)
}

/// File names of setups and uninstallers that many programs share (`setup.exe`,
/// `unins000.exe`, ...): they never claim a hit by name.
fn is_generic_installer_name(exe: &Path) -> bool {
    let stem = exe.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(stem.as_str(), "setup" | "install" | "installer" | "uninstall" | "uninst" | "uninstaller")
        || stem.strip_prefix("unins").is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
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

/// The [`norm`]ed exes an autostart entry (a `Run` value or a Startup shortcut) starts: `exe`
/// itself and, for a name-excluded launcher such as Squirrel's `Update.exe --processStart
/// Discord.exe`, each exe named in `arguments` ([`arg_exe_names`]) next to it. The real exe sits
/// in an `app-<version>` folder; matching compares through [`squirrel_neutral`].
pub fn autostart_targets(exe: &Path, arguments: &str) -> Vec<String> {
    let mut out = vec![norm(exe)];
    if let (true, Some(dir)) = (is_name_excluded(exe), exe.parent()) {
        out.extend(arg_exe_names(arguments).iter().map(|name| norm(&dir.join(name))));
    }
    out
}

/// `EAAntiCheat.Installer.exe`, `GameSetup.exe`: never the program itself, but running one is
/// not excluded from usage either (a setup can be the program, e.g. Visual Studio Installer's).
fn is_installer(exe: &Path) -> bool {
    exe.file_stem().is_some_and(|s| {
        let s = s.to_string_lossy().to_lowercase();
        s.contains("installer") || s.contains("setup")
    })
}

/// `<install>\APP-1.0.9253\DISCORD.EXE` -> `<install>\DISCORD.EXE` for [`norm`]ed paths: Squirrel
/// keeps each version in an `app-<version>` folder, so the main exe moves with every update.
fn squirrel_neutral(exe: &str, install: &str) -> String {
    let Some(rest) = exe.strip_prefix(install).and_then(|r| r.strip_prefix('\\')) else { return exe.to_string() };
    match rest.split_once('\\') {
        Some((dir, tail))
            if dir.strip_prefix("APP-").is_some_and(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.')) =>
        {
            format!(r"{install}\{tail}")
        }
        _ => exe.to_string(),
    }
}

/// `unins000.exe`, `Uninstall Foo.exe`, …
fn is_uninstaller(exe: &Path) -> bool {
    exe.file_stem().is_some_and(|s| s.to_string_lossy().to_lowercase().contains("unins"))
}

/// Whether `exe` is in a set of `norm()`ed exe paths. A drive-less path (a volume-stripped
/// Prefetch hit that was claimed by file name) matches an entry on any drive.
fn listed(set: &HashSet<String>, exe: &Path) -> bool {
    let n = norm(exe);
    if n.starts_with('\\') {
        set.iter().any(|e| e.get(2..).is_some_and(|rest| rest == n) && e.as_bytes().get(1) == Some(&b':'))
    } else {
        set.contains(&n)
    }
}

pub fn exclude_reason(exe: &Path, service_exes: &HashSet<String>) -> Option<ExcludeReason> {
    if is_name_excluded(exe) {
        Some(ExcludeReason::NamePattern)
    } else if listed(service_exes, exe) {
        Some(ExcludeReason::Service)
    } else {
        None
    }
}

/// A program prepared for matching, with everything normalized once.
struct Candidate<'a> {
    program: &'a Program,
    /// `norm()`ed [`usage_install_dir`].
    install: Option<String>,
    /// `norm()`ed main exes, Squirrel `app-<version>` folders removed ([`squirrel_neutral`]).
    main: HashSet<String>,
    /// The main exes as found on disk, in rule order (see [`Candidate::main_exe`]).
    mains: Vec<PathBuf>,
    /// Without an install folder: uppercased display-icon exe name (counts as main).
    icon_name: Option<String>,
    /// Without an install folder: uppercased uninstaller exe name (never MsiExec).
    uninstaller_name: Option<String>,
}

impl<'a> Candidate<'a> {
    fn new(program: &'a Program, input: &MatchInput) -> Self {
        let roots = input.root_exes.get(&program.id).map(Vec::as_slice).unwrap_or_default();
        let args = input.arg_exes.get(&program.id).map(Vec::as_slice).unwrap_or_default();
        let install = usage_install_dir(program, input.shortcut_targets).map(|d| norm(&d));
        let mains = program_main_exes(program, input.shortcut_targets, roots, args, input.service_exes).0;
        let main = mains
            .iter()
            .map(|e| squirrel_neutral(&norm(e), install.as_deref().unwrap_or_default()))
            .collect();
        // The display icon claims by name whenever it lies outside the folder (or there is none).
        let icon_name = program
            .display_icon
            .as_deref()
            .map(strip_icon_index)
            .filter(|i| is_exe(i) && !i.parent().is_none_or(is_rejected_dir) && !listed(input.service_exes, i))
            .filter(|i| !install.as_deref().is_some_and(|inst| normed_under(&norm(i), inst)))
            .and_then(|i| file_name_upper(&i));
        let uninstaller_name = if install.is_none() {
            program
                .uninstall_string
                .as_deref()
                .and_then(first_path_in_command)
                .filter(|u| is_exe(u) && !is_generic_installer_name(u))
                .and_then(|u| file_name_upper(&u))
                .filter(|n| n != "MSIEXEC.EXE")
        } else {
            None
        };
        Candidate { program, install, main, mains, icon_name, uninstaller_name }
    }

    /// The one main exe provenance reads ([`ProgramUsage::main_exe`]): the one the display icon
    /// names, else the most recently used one (`exes` is newest first), else the first.
    fn main_exe(&self, exes: &[ExeUsage]) -> Option<PathBuf> {
        let neutral = |p: &Path| squirrel_neutral(&norm(p), self.install.as_deref().unwrap_or_default());
        let icon = self.program.display_icon.as_deref().map(|i| norm(&strip_icon_index(i)));
        if let Some(icon) = icon {
            if let Some(m) = self.mains.iter().find(|m| norm(m) == icon) {
                return Some(m.clone());
            }
        }
        let used = exes.iter().filter(|e| e.is_main).find_map(|e| {
            let hit = neutral(&e.exe_path);
            self.mains.iter().find(|m| neutral(m) == hit)
        });
        used.or(self.mains.first()).cloned()
    }

    /// `exe` [`norm`]ed, its Squirrel `app-<version>` folder removed ([`squirrel_neutral`]).
    fn neutral(&self, exe: &Path) -> String {
        match &self.install {
            Some(install) => squirrel_neutral(&norm(exe), install),
            None => norm(exe),
        }
    }

    fn is_main(&self, exe: &Path) -> bool {
        let neutral = self.neutral(exe);
        self.main.contains(&neutral) || (self.icon_name.is_some() && file_name_upper(exe) == self.icon_name)
    }

    /// True for an exe inside the program's install folder (a subcomponent may count).
    fn owns_by_folder(&self, exe: &Path) -> bool {
        self.install.as_deref().is_some_and(|inst| normed_under(&norm(exe), inst))
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
            .map(|(i, _)| (i, with_install_drive(&hit.exe_path, candidates[i].install.as_deref())))
            .collect();
    }
    let name = file_name_upper(&hit.exe_path);
    candidates.iter().position(|c| c.claims_by_name(&name)).map(|i| (i, hit.exe_path.clone())).into_iter().collect()
}

/// Gives a drive-less (volume-stripped Prefetch) path the (`norm`ed) install folder's drive.
fn with_install_drive(exe: &Path, install: Option<&str>) -> PathBuf {
    let drive = install.map(Path::new).and_then(drive_letter);
    match drive {
        Some(d) if exe.to_string_lossy().starts_with('\\') => PathBuf::from(format!("{d}:{}", exe.display())),
        _ => exe.to_path_buf(),
    }
}

/// One `ExeUsage` per distinct exe (by `norm()`), newest first. Callers never mix FileTime
/// hits with other sources in one call (see `summarize`).
fn group_hits(
    hits: Vec<UsageHit>,
    is_main: impl Fn(&Path) -> bool,
    in_folder: impl Fn(&Path) -> bool,
    neutral: impl Fn(&Path) -> String,
    service_exes: &HashSet<String>,
    task_exes: &HashSet<String>,
    autostart_exes: &HashSet<String>,
) -> Vec<ExeUsage> {
    let mut groups: BTreeMap<String, Vec<UsageHit>> = BTreeMap::new();
    for hit in hits {
        groups.entry(norm(&hit.exe_path)).or_default().push(hit);
    }
    let mut exes: Vec<ExeUsage> = groups
        .into_values()
        .filter_map(|group| {
            let exe = group.first()?.exe_path.clone();
            let main = is_main(&exe);
            let setup_named = is_installer(&exe);
            let is_ua = |h: &UsageHit| h.source == UsageSource::UserAssist;
            // A main exe with a setup-like name (Visual Studio Installer's `setup.exe`) counts from
            // execution traces, but a UserAssist entry for it does not.
            let ua_blocked = setup_named && main;
            let traces: Vec<&UsageHit> = group.iter().filter(|h| !(ua_blocked && is_ua(h))).collect();
            let all_blocked = traces.is_empty();
            let group: Vec<UsageHit> = if all_blocked { group } else { traces.into_iter().cloned().collect() };
            // A cached or bundled setup is never use of the program, whatever recorded it.
            let installer = setup_named && (!main || all_blocked);
            // A scheduled task that starts the main exe is an autostart: the program runs, so
            // it counts. A task running any other exe is maintenance.
            let background = exclude_reason(&exe, service_exes)
                .or((!main && listed(task_exes, &exe)).then_some(ExcludeReason::ScheduledTask))
                .or((!main && !in_folder(&exe)).then_some(ExcludeReason::NotMain));
            // UserAssist records launches by the user, so an otherwise excluded exe counts when the
            // user started it (running an uninstaller never does). Only those launches date it:
            // its Prefetch/BAM traces come from tasks, services or other programs, not the user.
            let user_launched = background.is_some() && group.iter().any(is_ua) && !is_uninstaller(&exe);
            let (group, excluded): (Vec<UsageHit>, Option<ExcludeReason>) = if installer {
                (group, Some(ExcludeReason::NamePattern))
            } else if user_launched {
                (group.into_iter().filter(is_ua).collect(), None)
            } else {
                (group, background)
            };
            let latest = group.iter().max_by_key(|h| h.last_run)?;
            Some(ExeUsage {
                exe_path: latest.exe_path.clone(),
                last_run: latest.last_run,
                source: latest.source,
                run_count: group.iter().filter_map(|h| h.run_count).max(),
                is_main: main,
                excluded,
                autostart: listed(task_exes, &exe) || listed(autostart_exes, &exe) || autostart_exes.contains(&neutral(&exe)),
                user_launch_only: user_launched && !installer,
            })
        })
        .collect();
    exes.sort_by_key(|e| std::cmp::Reverse(e.last_run));
    exes
}

fn last_used<'a>(exes: impl IntoIterator<Item = &'a ExeUsage>) -> LastUsed {
    exes.into_iter()
        .filter(|e| e.excluded.is_none())
        // On a tie the main exe wins, so `via` stays empty.
        .max_by_key(|e| (e.last_run, e.is_main))
        .map(|e| LastUsed {
            when: Some(e.last_run),
            source: Some(e.source),
            confident: e.source != UsageSource::FileTime,
            via: (!e.is_main && e.source != UsageSource::FileTime)
                .then(|| e.exe_path.file_name().map(|n| n.to_string_lossy().to_lowercase()))
                .flatten(),
            // A user launch (UserAssist) or a file time is no sign of an autostart.
            autostart: e.autostart && matches!(e.source, UsageSource::Bam | UsageSource::Prefetch),
        })
        .unwrap_or(LastUsed { when: None, source: None, confident: true, via: None, autostart: false })
}

/// Summarizes one program's hits (spec §5). File timestamps are a pure fallback: they never
/// merge into an exe that has a real source, and they date the program only when it has no
/// other hits at all (excluded ones included).
fn summarize(
    hits: Vec<UsageHit>,
    is_main: impl Fn(&Path) -> bool,
    in_folder: impl Fn(&Path) -> bool,
    neutral: impl Fn(&Path) -> String,
    service_exes: &HashSet<String>,
    task_exes: &HashSet<String>,
    autostart_exes: &HashSet<String>,
) -> ProgramUsage {
    let (file_times, real): (Vec<UsageHit>, Vec<UsageHit>) = hits.into_iter().partition(|h| h.source == UsageSource::FileTime);
    let real = group_hits(real, &is_main, &in_folder, &neutral, service_exes, task_exes, autostart_exes);
    let real_exes: HashSet<String> = real.iter().map(|e| norm(&e.exe_path)).collect();
    let file_times: Vec<ExeUsage> = group_hits(file_times, &is_main, &in_folder, &neutral, service_exes, task_exes, autostart_exes)
        .into_iter()
        .filter(|e| !real_exes.contains(&norm(&e.exe_path)))
        .collect();
    let last_used = if real.is_empty() { last_used(&file_times) } else { last_used(&real) };
    let mut exes = real;
    exes.extend(file_times);
    exes.sort_by_key(|e| std::cmp::Reverse(e.last_run));
    ProgramUsage { last_used, exes, main_exe: None }
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
        .map(|(c, hits)| {
            let usage = summarize(
                hits,
                |e| c.is_main(e),
                |e| c.owns_by_folder(e),
                |e| c.neutral(e),
                input.service_exes,
                input.task_exes,
                input.autostart_exes,
            );
            (c.program.id.clone(), ProgramUsage { main_exe: c.main_exe(&usage.exes), ..usage })
        })
        .collect();
    MatchResult { by_program, unmatched }
}

/// How one hit fared for one program (see [`explain_match`]).
#[derive(Clone, Debug, PartialEq)]
pub struct HitExplanation {
    pub hit: UsageHit,
    /// `counted:main`, `counted`, `excluded:<ExcludeReason>`, `superseded` (a file time hidden by
    /// a real source for the same exe), `filetime-ignored` (file times only date a program without
    /// other hits, see `summarize`), `ignored:user-launch-only` (a BAM/Prefetch trace of an exe
    /// that only the user's UserAssist launches date), `assigned-to:<other program names>`,
    /// `drive-less-unmatched` (volume-stripped path no install location claims) or `no-candidate`.
    pub outcome: String,
}

/// Explains, for each hit in `input.hits`, whether [`match_usage`] assigns it to `program` and
/// whether it then counts, or why not. Diagnostics only (`dump usage`).
pub fn explain_match(input: &MatchInput, program: &ProgramId) -> Vec<HitExplanation> {
    let candidates: Vec<Candidate> = input.programs.iter().map(|p| Candidate::new(p, input)).collect();
    let me = candidates.iter().position(|c| &c.program.id == program);
    let usage = match_usage(input).by_program.remove(program);
    input
        .hits
        .iter()
        .map(|hit| {
            let owners = assign_hit(hit, &candidates);
            let mine = me.and_then(|i| owners.iter().find(|(o, _)| *o == i));
            let outcome = match mine {
                Some((_, path)) => {
                    let n = norm(path);
                    let file_time = hit.source == UsageSource::FileTime;
                    let exe = usage.as_ref().and_then(|u| {
                        u.exes.iter().find(|e| norm(&e.exe_path) == n && (e.source == UsageSource::FileTime) == file_time)
                    });
                    let has_real = usage.as_ref().is_some_and(|u| u.exes.iter().any(|e| e.source != UsageSource::FileTime));
                    match exe {
                        Some(_) if file_time && has_real => "filetime-ignored".to_string(),
                        Some(e) => match e.excluded {
                            Some(r) => format!("excluded:{r:?}"),
                            // Dropped: only the user's launches date this exe.
                            None if e.user_launch_only && hit.source != UsageSource::UserAssist => {
                                "ignored:user-launch-only".to_string()
                            }
                            None if e.is_main => "counted:main".to_string(),
                            None => "counted".to_string(),
                        },
                        None => "superseded".to_string(),
                    }
                }
                None if !owners.is_empty() => {
                    let names: Vec<&str> = owners.iter().map(|(o, _)| candidates[*o].program.name.as_str()).collect();
                    format!("assigned-to:{}", names.join(", "))
                }
                None if hit.exe_path.to_string_lossy().starts_with('\\') => "drive-less-unmatched".to_string(),
                None => "no-candidate".to_string(),
            };
            HitExplanation { hit: hit.clone(), outcome }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};

    use super::*;
    use crate::model::{Hive, Scope};
    use crate::registry::key;

    fn prog(id: &str, install: Option<&str>) -> Program {
        Program {
            account: None,
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
    fn run_with_tasks(programs: &[Program], hits: &[UsageHit], tasks: &HashSet<String>) -> MatchResult {
        let shortcuts = targets(&[r"C:\Apps\Foo\foo.exe"]);
        let (roots, args, services) = (HashMap::new(), HashMap::new(), HashSet::new());
        match_usage(&MatchInput { task_exes: tasks, ..input(programs, hits, &shortcuts, &roots, &args, &services) })
    }

    /// Like [`run_with_tasks`], with the given services and no scheduled tasks.
    fn run(programs: &[Program], hits: &[UsageHit], services: &HashSet<String>) -> MatchResult {
        let shortcuts = targets(&[r"C:\Apps\Foo\foo.exe"]);
        let (roots, args) = (HashMap::new(), HashMap::new());
        match_usage(&input(programs, hits, &shortcuts, &roots, &args, services))
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
        assert_eq!(exclude_reason(exe, &services), Some(ExcludeReason::NamePattern));
        assert_eq!(exclude_reason(Path::new(r"C:\Apps\Foo\CrashReporter.exe"), &HashSet::new()), Some(ExcludeReason::NamePattern));
        assert_eq!(exclude_reason(Path::new(r"C:\Apps\Foo\AGENT.EXE"), &services), Some(ExcludeReason::Service));
        assert_eq!(exclude_reason(Path::new(r"C:\Apps\Foo\other.exe"), &services), None);
        assert_eq!(exclude_reason(Path::new(r"C:\Apps\Foo\foo.exe"), &services), None);
        // Only the stem is checked: a folder name containing "update" does not exclude.
        assert_eq!(exclude_reason(Path::new(r"C:\Apps\Updates\foo.exe"), &services), None);
    }

    #[test]
    fn updater_hit_does_not_set_last_used_but_appears_in_exes() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let hits = [hit(r"C:\Apps\Foo\FooUpdater.exe", 1, UsageSource::Bam), hit(r"C:\Apps\Foo\foo.exe", 200, UsageSource::Bam)];
        let r = run(&programs, &hits, &HashSet::new());
        let u = usage(&r, "Foo");
        assert_eq!(u.last_used, LastUsed { when: Some(days_ago(200)), source: Some(UsageSource::Bam), confident: true, via: None, autostart: false });
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
        assert_eq!(u.last_used, LastUsed { when: None, source: None, confident: true, via: None, autostart: false });
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
        assert_eq!(u.last_used, LastUsed { when: Some(days_ago(5)), source: Some(UsageSource::UserAssist), confident: true, via: Some("other.exe".into()), autostart: false });
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
        assert_eq!(usage(&r, "Foo").last_used, LastUsed { when: Some(days_ago(30)), source: Some(UsageSource::FileTime), confident: false, via: None, autostart: false });
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
        assert_eq!(exclude_reason(Path::new(r"C:\Apps\Foo\unins000.exe"), &services), Some(ExcludeReason::NamePattern));
        assert_eq!(exclude_reason(Path::new(r"C:\Apps\Foo\Uninstall.exe"), &services), Some(ExcludeReason::NamePattern));
        let hits = [hit(r"C:\Apps\Foo\unins000.exe", 1, UsageSource::Bam)];
        let r = match_usage(&MatchInput {
            programs: &[p],
            hits: &hits,
            shortcut_targets: &shortcuts,
            root_exes: &HashMap::from([(ProgramId("Foo".into()), roots)]),
            arg_exes: &HashMap::new(),
            service_exes: &services,
            task_exes: &HashSet::new(),
            autostart_exes: &HashSet::new(),
        });
        assert_eq!(usage(&r, "Foo").last_used.when, None);
    }

    #[test]
    fn unmatched_hits_are_returned() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let hits = [hit(r"C:\Nowhere\x.exe", 1, UsageSource::Bam)];
        let r = run(&programs, &hits, &HashSet::new());
        assert_eq!(r.unmatched, hits.to_vec());
        assert_eq!(
            usage(&r, "Foo"),
            &ProgramUsage {
                last_used: LastUsed { when: None, source: None, confident: true, via: None, autostart: false },
                exes: vec![],
                main_exe: Some(PathBuf::from(r"C:\Apps\Foo\foo.exe")),
            }
        );
    }

    #[test]
    fn every_program_carries_its_first_main_exe() {
        let mut bar = prog("Bar", Some(r"C:\Apps\Bar"));
        bar.display_icon = Some(PathBuf::from(r"C:\Apps\Bar\bar.exe,0"));
        let programs = [prog("Foo", Some(r"C:\Apps\Foo")), bar, prog("Baz", None)];
        let r = run(&programs, &[], &HashSet::new());
        assert_eq!(usage(&r, "Foo").main_exe, Some(PathBuf::from(r"C:\Apps\Foo\foo.exe")));
        assert_eq!(usage(&r, "Bar").main_exe, Some(PathBuf::from(r"C:\Apps\Bar\bar.exe")));
        assert_eq!(usage(&r, "Baz").main_exe, None);
        // A service is never the main exe.
        let services: HashSet<String> = [norm(Path::new(r"C:\Apps\Foo\foo.exe"))].into();
        let r = run(&programs[..1], &[], &services);
        assert_eq!(usage(&r, "Foo").main_exe, None);
    }

    #[test]
    fn provenance_main_exe_prefers_the_icon_then_the_newest_used() {
        let shortcuts = targets(&[r"C:\VS\Common7\IDE\Blend.exe", r"C:\VS\Common7\IDE\devenv.exe"]);
        let (roots, args, services) = (HashMap::new(), HashMap::new(), HashSet::new());
        let main_exe = |p: &Program, hits: &[UsageHit]| {
            let programs = [p.clone()];
            let r = match_usage(&input(&programs, hits, &shortcuts, &roots, &args, &services));
            usage(&r, &p.id.0).main_exe.clone()
        };
        let mut vs = prog("VS", Some(r"C:\VS"));
        // Without icon or usage: the first main exe.
        assert_eq!(main_exe(&vs, &[]), Some(PathBuf::from(r"C:\VS\Common7\IDE\Blend.exe")));
        // The newest used main exe beats the first.
        let used = [hit(r"C:\VS\Common7\IDE\Blend.exe", 9, UsageSource::Bam), hit(r"C:\VS\Common7\IDE\devenv.exe", 2, UsageSource::Bam)];
        assert_eq!(main_exe(&vs, &used), Some(PathBuf::from(r"C:\VS\Common7\IDE\devenv.exe")));
        // The display icon beats both.
        vs.display_icon = Some(PathBuf::from(r"c:\vs\common7\ide\DEVENV.EXE,0"));
        let blend_newer = [hit(r"C:\VS\Common7\IDE\Blend.exe", 1, UsageSource::Bam)];
        assert_eq!(main_exe(&vs, &blend_newer), Some(PathBuf::from(r"C:\VS\Common7\IDE\devenv.exe")));
        // Matching itself is unchanged: both shortcut targets are main exes.
        let programs = [vs.clone()];
        let r = match_usage(&input(&programs, &used, &shortcuts, &roots, &args, &services));
        assert!(usage(&r, "VS").exes.iter().all(|e| e.is_main));
    }

    #[test]
    fn squirrel_update_exe_falls_through_to_process_start_target() {
        let p = prog("Discord", Some(r"C:\Users\X\AppData\Local\Discord"));
        let update = PathBuf::from(r"C:\Users\X\AppData\Local\Discord\Update.exe");
        let shortcuts = [ShortcutTarget { path: update.clone(), arguments: "--processStart Discord.exe".into(), lnk: PathBuf::new() }];
        let main = PathBuf::from(r"C:\Users\X\AppData\Local\Discord\app-1.0\Discord.exe");
        let roots = vec![(update.clone(), 2_000_000)];
        assert_eq!(main_exes(&p, &shortcuts, &roots, std::slice::from_ref(&main)), vec![main.clone()]);
        assert_eq!(exclude_reason(&update, &HashSet::new()), Some(ExcludeReason::NamePattern));

        let hits = [hit(main.to_str().unwrap(), 3, UsageSource::Bam), hit(update.to_str().unwrap(), 1, UsageSource::Bam)];
        let r = match_usage(&MatchInput {
            programs: &[p],
            hits: &hits,
            shortcut_targets: &shortcuts,
            root_exes: &HashMap::from([(ProgramId("Discord".into()), roots)]),
            arg_exes: &HashMap::from([(ProgramId("Discord".into()), vec![main.clone()])]),
            service_exes: &HashSet::new(),
            task_exes: &HashSet::new(),
            autostart_exes: &HashSet::new(),
        });
        let u = usage(&r, "Discord");
        assert_eq!(u.last_used, LastUsed { when: Some(days_ago(3)), source: Some(UsageSource::Bam), confident: true, via: None, autostart: false });
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
        assert_eq!(u.last_used, LastUsed { when: Some(days_ago(300)), source: Some(UsageSource::Bam), confident: true, via: None, autostart: false });
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

    fn explain(programs: &[Program], hits: &[UsageHit], id: &str) -> Vec<String> {
        let shortcuts = targets(&[r"C:\Apps\Foo\foo.exe"]);
        let input = MatchInput {
            programs,
            hits,
            shortcut_targets: &shortcuts,
            root_exes: &HashMap::new(),
            arg_exes: &HashMap::new(),
            service_exes: &HashSet::new(),
            task_exes: &HashSet::new(),
            autostart_exes: &HashSet::new(),
        };
        explain_match(&input, &ProgramId(id.to_string())).into_iter().map(|e| e.outcome).collect()
    }

    #[test]
    fn explains_hit_outside_install_dir_as_no_candidate() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let hits = [hit(r"C:\Elsewhere\foo.exe", 1, UsageSource::Bam), hit(r"\ELSEWHERE\FOO.EXE", 1, UsageSource::Prefetch)];
        assert_eq!(explain(&programs, &hits, "Foo"), ["no-candidate", "drive-less-unmatched"]);
    }

    #[test]
    fn explains_excluded_updater() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let hits = [hit(r"C:\Apps\Foo\FooUpdater.exe", 1, UsageSource::Bam), hit(r"C:\Apps\Foo\foo.exe", 2, UsageSource::Bam)];
        assert_eq!(explain(&programs, &hits, "Foo"), ["excluded:NamePattern", "counted:main"]);
    }

    #[test]
    fn explains_filetime_ignored_beside_real_hits() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let hits = [hit(r"C:\Apps\Foo\FooUpdater.exe", 1, UsageSource::Bam), hit(r"C:\Apps\Foo\foo.exe", 2, UsageSource::FileTime)];
        assert_eq!(explain(&programs, &hits, "Foo"), ["excluded:NamePattern", "filetime-ignored"]);
        assert_eq!(explain(&programs, &hits[1..], "Foo"), ["counted:main"]);
    }

    #[test]
    fn explains_hit_assigned_to_other_program() {
        let programs = [prog("Outer", Some(r"C:\Apps\Suite")), prog("Inner", Some(r"C:\Apps\Suite\Plugin"))];
        let hits = [hit(r"C:\Apps\Suite\Plugin\plug.exe", 1, UsageSource::Bam)];
        assert_eq!(explain(&programs, &hits, "Outer"), ["assigned-to:Inner"]);
        assert_eq!(explain(&programs, &hits, "Inner"), ["counted"]);
    }

    #[test]
    fn explain_marks_hits_dropped_by_user_launch_rule() {
        // An updater the user once started: only its UserAssist launch dates it.
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let exe = r"C:\Apps\Foo\FooUpdater.exe";
        let hits = [hit(exe, 3, UsageSource::UserAssist), hit(exe, 5, UsageSource::Bam), hit(exe, 5, UsageSource::Prefetch)];
        assert_eq!(explain(&programs, &hits, "Foo"), ["counted", "ignored:user-launch-only", "ignored:user-launch-only"]);
        // A main exe with UserAssist and BAM hits counts both.
        let main = [hit(r"C:\Apps\Foo\foo.exe", 5, UsageSource::UserAssist), hit(r"C:\Apps\Foo\foo.exe", 3, UsageSource::Bam)];
        assert_eq!(explain(&programs, &main, "Foo"), ["counted:main", "counted:main"]);
    }

    fn input<'a>(
        programs: &'a [Program],
        hits: &'a [UsageHit],
        shortcuts: &'a [ShortcutTarget],
        roots: &'a HashMap<ProgramId, Vec<(PathBuf, u64)>>,
        args: &'a HashMap<ProgramId, Vec<PathBuf>>,
        services: &'a HashSet<String>,
    ) -> MatchInput<'a> {
        static EMPTY: std::sync::LazyLock<HashSet<String>> = std::sync::LazyLock::new(HashSet::new);
        MatchInput { programs, hits, shortcut_targets: shortcuts, root_exes: roots, arg_exes: args, service_exes: services, task_exes: &EMPTY, autostart_exes: &EMPTY }
    }

    /// Observed live: Steam's Uninstall entry has no InstallLocation, only
    /// `C:\Program Files (x86)\Steam\uninstall.exe`; BAM/Prefetch/UserAssist hits on `steam.exe`
    /// were claimed by name by a Steam game whose uninstall string runs `steam.exe`, and
    /// `steamwebhelper.exe` matched nothing.
    #[test]
    fn program_without_install_location_matches_in_its_uninstallers_folder() {
        let mut steam = prog("Steam", None);
        steam.display_icon = Some(PathBuf::from(r"C:\Program Files (x86)\Steam\uninstall.exe"));
        steam.uninstall_string = Some(r"C:\Program Files (x86)\Steam\uninstall.exe".into());
        let mut game = prog("Dota Underlords", None);
        game.uninstall_string = Some(r#""C:\Program Files (x86)\Steam\steam.exe" steam://uninstall/1046930"#.into());
        let steam_exe = r"C:\Program Files (x86)\Steam\steam.exe";
        let hits = [
            hit(steam_exe, 1, UsageSource::Bam),
            hit(r"\PROGRAM FILES (X86)\STEAM\STEAM.EXE", 1, UsageSource::Prefetch),
            hit(r"C:\Program Files (x86)\Steam\bin\cef\cef.win64\steamwebhelper.exe", 0, UsageSource::Bam),
        ];
        let roots = HashMap::from([(
            ProgramId("Steam".into()),
            vec![(PathBuf::from(steam_exe), 4_000_000), (PathBuf::from(r"C:\Program Files (x86)\Steam\uninstall.exe"), 9_000_000)],
        )]);
        let (none, services) = (HashMap::new(), HashSet::new());
        let programs = [steam.clone(), game];
        let r = match_usage(&input(&programs, &hits, &[], &roots, &none, &services));
        let u = usage(&r, "Steam");
        assert_eq!(u.last_used.when, Some(days_ago(1)));
        assert!(u.exes.iter().any(|e| norm(&e.exe_path) == norm(Path::new(steam_exe)) && e.is_main && e.excluded.is_none()));
        assert_eq!(u.exes.len(), 2, "steam.exe (BAM + Prefetch merged) and steamwebhelper.exe: {:?}", u.exes);
        assert!(usage(&r, "Dota Underlords").exes.is_empty());
        assert!(r.unmatched.is_empty());
        assert_eq!(usage_install_dir(&steam, &[]), Some(PathBuf::from(r"C:\Program Files (x86)\Steam")));
    }

    /// Observed live: Google Chrome is an MSI entry with no InstallLocation, DisplayIcon or
    /// uninstaller path (`MsiExec.exe /X{E9AB118B-…}`); the only link to its files is the Start
    /// Menu shortcut `Google Chrome.lnk` -> `C:\Program Files\Google\Chrome\Application\chrome.exe`.
    /// Review finding: a non-MSI Chrome entry (no InstallLocation) names its uninstaller in a
    /// versioned `Installer` folder below the display icon's folder; the folder must be the
    /// display icon's, so `chrome.exe` stays the main exe.
    #[test]
    fn nested_uninstaller_folder_widens_to_display_icon_folder() {
        let mut chrome = prog("Google Chrome", None);
        chrome.display_icon = Some(PathBuf::from(r"C:\Program Files\Google\Chrome\Application\chrome.exe"));
        chrome.uninstall_string = Some(
            r#""C:\Program Files\Google\Chrome\Application\141.0.7390.108\Installer\setup.exe" --uninstall --system-level"#.into(),
        );
        assert_eq!(usage_install_dir(&chrome, &[]), Some(PathBuf::from(r"C:\Program Files\Google\Chrome\Application")));
        let exe = r"C:\Program Files\Google\Chrome\Application\chrome.exe";
        assert_eq!(main_exes_with_rule(&chrome, &[], &[], &[]), (vec![PathBuf::from(exe)], MainRule::DisplayIcon));
        let hits = [hit(exe, 0, UsageSource::Bam)];
        let (no_roots, no_args, services) = (HashMap::new(), HashMap::new(), HashSet::new());
        let r = match_usage(&input(std::slice::from_ref(&chrome), &hits, &[], &no_roots, &no_args, &services));
        let u = usage(&r, "Google Chrome");
        assert_eq!(u.last_used.when, Some(days_ago(0)));
        assert!(u.exes[0].is_main && u.exes[0].excluded.is_none());

        let mut app = prog("App", None);
        app.display_icon = Some(PathBuf::from(r"C:\Apps\App\app.exe"));
        app.uninstall_string = Some(r"C:\Apps\App\uninstall\uninstall.exe".into());
        assert_eq!(usage_install_dir(&app, &[]), Some(PathBuf::from(r"C:\Apps\App")));
    }

    /// The display icon outside the folder derived from the uninstaller still claims its exe by
    /// file name (as before the folder rule existed).
    #[test]
    fn display_icon_outside_derived_folder_still_claims_by_name() {
        let mut p = prog("Bar", None);
        p.display_icon = Some(PathBuf::from(r"C:\Apps\Bar\bar.exe"));
        p.uninstall_string = Some(r"D:\Vendor\Uninstallers\Bar\uninstall.exe".into());
        assert_eq!(usage_install_dir(&p, &[]), Some(PathBuf::from(r"D:\Vendor\Uninstallers\Bar")));
        let hits = [hit(r"C:\Apps\Bar\bar.exe", 3, UsageSource::Bam)];
        let (no_roots, no_args, services) = (HashMap::new(), HashMap::new(), HashSet::new());
        let r = match_usage(&input(std::slice::from_ref(&p), &hits, &[], &no_roots, &no_args, &services));
        let u = usage(&r, "Bar");
        assert_eq!(u.last_used.when, Some(days_ago(3)));
        assert!(u.exes[0].is_main);
    }

    #[test]
    fn msi_shortcut_into_a_user_folder_gives_no_folder() {
        let mut p = prog("Tool", None);
        p.is_msi = true;
        let shortcuts = [ShortcutTarget {
            path: env_path("USERPROFILE").join("Downloads").join("x.exe"),
            arguments: String::new(),
            lnk: PathBuf::from(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Tool.lnk"),
        }];
        assert_eq!(usage_install_dir(&p, &shortcuts), None);
    }

    /// Observed live: "ASUS Motherboard" (no folder, uninstaller `setup.exe`) claimed every
    /// unclaimed `setup.exe` hit by name.
    #[test]
    fn generic_setup_exe_is_not_name_claimed() {
        let hits = [hit(r"C:\Program Files (x86)\Microsoft Visual Studio\Installer.d454a31ec5884299aebc72b9310115fb\setup.exe", 1, UsageSource::Bam)];
        for uninstaller in ["setup.exe", "Setup.exe", "install.exe", "uninstall.exe", "unins000.exe", "uninst.exe"] {
            let mut p = prog("ASUS Motherboard", None);
            p.uninstall_string = Some(format!(r#""C:\ProgramData\Package Cache\{{5E1B1F9A}}\{uninstaller}" /uninstall"#));
            assert_eq!(usage_install_dir(&p, &[]), None, "{uninstaller}");
            let hits = [UsageHit { exe_path: PathBuf::from(format!(r"C:\Elsewhere\{uninstaller}")), ..hits[0].clone() }];
            let r = run(std::slice::from_ref(&p), &hits, &HashSet::new());
            assert!(usage(&r, "ASUS Motherboard").exes.is_empty(), "{uninstaller}");
            assert_eq!(r.unmatched.len(), 1);
        }
        // A specific uninstaller name still claims (as not-main).
        let mut p = prog("Baz", None);
        p.uninstall_string = Some(r#""C:\ProgramData\Package Cache\{5E1B1F9A}\BazRemover.exe""#.into());
        let hits = [hit(r"D:\x\BazRemover.exe", 1, UsageSource::Bam)];
        assert_eq!(usage(&run(&[p], &hits, &HashSet::new()), "Baz").exes.len(), 1);
    }

    #[test]
    fn msi_entry_without_paths_uses_start_menu_shortcut_named_like_it() {
        let mut chrome = prog("Google Chrome", None);
        chrome.name = "Google Chrome".into();
        chrome.is_msi = true;
        chrome.uninstall_string = Some("MsiExec.exe /X{E9AB118B-2341-3DD2-BD45-27B55F5F3802}".into());
        let exe = r"C:\Program Files\Google\Chrome\Application\chrome.exe";
        let shortcuts = [
            ShortcutTarget {
                path: PathBuf::from(exe),
                arguments: String::new(),
                lnk: PathBuf::from(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Google Chrome.lnk"),
            },
            // A shortcut with another name into a shared root never gives a folder.
            ShortcutTarget {
                path: PathBuf::from(r"C:\Windows\System32\cmd.exe"),
                arguments: String::new(),
                lnk: PathBuf::from(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Paint.NET.lnk"),
            },
        ];
        assert_eq!(usage_install_dir(&chrome, &shortcuts), Some(PathBuf::from(r"C:\Program Files\Google\Chrome\Application")));
        let mut paint = prog("Paint.NET", None);
        paint.is_msi = true;
        assert_eq!(usage_install_dir(&paint, &shortcuts), None, "shortcut target in System32");
        // Observed live: Chrome/Edge web apps ("YouTube", "Messenger") are non-MSI entries with
        // a same-named Start Menu shortcut to the browser's `chrome_proxy.exe`; they must not
        // take the browser's folder (and with it the browser's usage).
        let mut youtube = prog("YouTube", None);
        youtube.uninstall_string = Some(r#""C:\Program Files\Google\Chrome\Application\chrome.exe" --uninstall-app-id=agimnkijcaahngcdmfeangaknmldooml"#.into());
        let pwa = [ShortcutTarget {
            path: PathBuf::from(r"C:\Program Files\Google\Chrome\Application\chrome_proxy.exe"),
            arguments: "--profile-directory=Default --app-id=agimnkijcaahngcdmfeangaknmldooml".into(),
            lnk: PathBuf::from(r"C:\Users\X\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Chrome Apps\YouTube.lnk"),
        }];
        assert_eq!(usage_install_dir(&youtube, &pwa), None);

        let hits = [
            hit(exe, 0, UsageSource::Bam),
            hit(r"C:\Program Files\Google\Chrome\Application\141.0.7390.108\Installer\setup.exe", 0, UsageSource::Bam),
        ];
        let (no_roots, no_args, services) = (HashMap::new(), HashMap::new(), HashSet::new());
        let r = match_usage(&input(std::slice::from_ref(&chrome), &hits, &shortcuts, &no_roots, &no_args, &services));
        let u = usage(&r, "Google Chrome");
        assert_eq!(u.last_used.when, Some(days_ago(0)));
        assert!(u.exes.iter().any(|e| e.exe_path == Path::new(exe) && e.is_main && e.excluded.is_none()));
    }

    /// Observed live: Toggl Track (Squirrel) has a root stub `TogglTrack.exe` (picked as the
    /// largest root exe) while BAM records `app-10.18.1\TogglTrack.exe`.
    #[test]
    fn squirrel_versioned_app_folder_counts_as_main() {
        let install = r"C:\Users\Jobb\AppData\Local\TogglTrack";
        let toggl = prog("Toggl Track", Some(install));
        let roots = HashMap::from([(
            ProgramId("Toggl Track".into()),
            vec![(PathBuf::from(format!(r"{install}\TogglTrack.exe")), 600_000), (PathBuf::from(format!(r"{install}\Update.exe")), 1_900_000)],
        )]);
        let hits = [hit(&format!(r"{install}\app-10.18.1\TogglTrack.exe"), 0, UsageSource::Bam)];
        let (none, services) = (HashMap::new(), HashSet::new());
        let r = match_usage(&input(std::slice::from_ref(&toggl), &hits, &[], &roots, &none, &services));
        let u = usage(&r, "Toggl Track");
        assert_eq!(u.last_used.when, Some(days_ago(0)));
        assert!(u.exes[0].is_main);

        // Also when the main exe was found in an older app folder (Discord's shortcut arguments).
        let discord = prog("Discord", Some(r"C:\Users\Jimmy\AppData\Local\Discord"));
        let args = HashMap::from([(
            ProgramId("Discord".into()),
            vec![PathBuf::from(r"C:\Users\Jimmy\AppData\Local\Discord\app-1.0.9253\Discord.exe")],
        )]);
        let shortcuts = [ShortcutTarget {
            path: PathBuf::from(r"C:\Users\Jimmy\AppData\Local\Discord\Update.exe"),
            arguments: "--processStart Discord.exe".into(),
            lnk: PathBuf::new(),
        }];
        let hits = [hit(r"C:\Users\Jimmy\AppData\Local\Discord\app-1.0.9254\Discord.exe", 2, UsageSource::Bam)];
        let r = match_usage(&input(std::slice::from_ref(&discord), &hits, &shortcuts, &HashMap::new(), &args, &services));
        assert_eq!(usage(&r, "Discord").last_used.when, Some(days_ago(2)));
    }

    /// Observed live: Battlefield 6's largest root exe is `EAAntiCheat.Installer.exe`, so `bf6.exe`
    /// (in BAM) was "not main".
    #[test]
    fn largest_root_exe_skips_installers() {
        let install = r"H:\SteamLibrary\steamapps\common\Battlefield 6";
        let p = prog("Battlefield 6", Some(install));
        let roots = vec![
            (PathBuf::from(format!(r"{install}\EAAntiCheat.Installer.exe")), 90_000_000),
            (PathBuf::from(format!(r"{install}\EAAntiCheat.GameServiceLauncher.exe")), 5_000_000),
            (PathBuf::from(format!(r"{install}\GameSetup.exe")), 80_000_000),
            (PathBuf::from(format!(r"{install}\bf6.exe")), 2_000_000),
        ];
        assert_eq!(main_exes_with_rule(&p, &[], &roots, &[]), (vec![PathBuf::from(format!(r"{install}\bf6.exe"))], MainRule::LargestRootExe));
    }

    fn via_of(u: &ProgramUsage) -> (Option<DateTime<Utc>>, Option<&str>) {
        (u.last_used.when, u.last_used.via.as_deref())
    }

    #[test]
    fn non_main_exe_counts_and_sets_via() {
        let vs = r"C:\Program Files\Microsoft Visual Studio\18\Professional";
        let programs = [prog("VS", Some(vs))];
        let hits = [hit(&format!(r"{vs}\VC\Tools\MSVC\14.50\bin\Hostx64\x64\LINK.EXE"), 0, UsageSource::Bam)];
        let r = run(&programs, &hits, &HashSet::new());
        let u = usage(&r, "VS");
        assert_eq!(via_of(u), (Some(days_ago(0)), Some("link.exe")));
        assert_eq!(u.exes[0].excluded, None);
        assert!(!u.exes[0].is_main);
    }

    #[test]
    fn main_exe_hit_wins_without_via() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let newer_other = [hit(r"C:\Apps\Foo\foo.exe", 1, UsageSource::Bam), hit(r"C:\Apps\Foo\tool.exe", 0, UsageSource::Bam)];
        let r = run(&programs, &newer_other, &HashSet::new());
        assert_eq!(via_of(usage(&r, "Foo")), (Some(days_ago(0)), Some("tool.exe")));
        let newer_main = [hit(r"C:\Apps\Foo\foo.exe", 0, UsageSource::Bam), hit(r"C:\Apps\Foo\tool.exe", 1, UsageSource::Bam)];
        let r = run(&programs, &newer_main, &HashSet::new());
        assert_eq!(via_of(usage(&r, "Foo")), (Some(days_ago(0)), None));
        let tie = [hit(r"C:\Apps\Foo\tool.exe", 0, UsageSource::Bam), hit(r"C:\Apps\Foo\foo.exe", 0, UsageSource::Bam)];
        let r = run(&programs, &tie, &HashSet::new());
        assert_eq!(via_of(usage(&r, "Foo")), (Some(days_ago(0)), None));
    }

    #[test]
    fn updater_still_excluded() {
        let chrome = r"C:\Program Files\Google\Chrome";
        let programs = [prog("Chrome", Some(chrome))];
        let hits = [
            hit(&format!(r"{chrome}\GoogleUpdate.exe"), 0, UsageSource::Bam),
            hit(&format!(r"{chrome}\updater.exe"), 0, UsageSource::Prefetch),
            hit(&format!(r"{chrome}\Application\crashpad_handler.exe"), 0, UsageSource::Bam),
            hit(&format!(r"{chrome}\unins000.exe"), 0, UsageSource::Bam),
            hit(&format!(r"{chrome}\Application\141.0\Installer\setup.exe"), 0, UsageSource::Bam),
        ];
        let r = run(&programs, &hits, &HashSet::new());
        let u = usage(&r, "Chrome");
        assert_eq!(u.last_used.when, None);
        assert!(u.exes.iter().all(|e| e.excluded == Some(ExcludeReason::NamePattern)), "{:?}", u.exes);
    }

    #[test]
    fn scheduled_task_exe_does_not_count() {
        let programs = [prog("Office", Some(r"C:\Program Files\Microsoft Office"))];
        let hits = [
            hit(r"C:\Program Files\Microsoft Office\root\VFS\ActionsServer.exe", 0, UsageSource::Bam),
            hit(r"C:\Program Files\Microsoft Office\Office16\opushutil.exe", 0, UsageSource::Prefetch),
            hit(r"C:\Program Files\Microsoft Office\Office16\WINWORD.EXE", 5, UsageSource::Bam),
        ];
        let tasks = HashSet::from([
            norm(Path::new(r"C:\Program Files\Microsoft Office\root\VFS\ActionsServer.exe")),
            norm(Path::new(r"C:\Program Files\Microsoft Office\Office16\opushutil.exe")),
        ]);
        let r = run_with_tasks(&programs, &hits, &tasks);
        let u = usage(&r, "Office");
        assert_eq!(via_of(u), (Some(days_ago(5)), Some("winword.exe")));
        assert_eq!(u.exes.iter().filter(|e| e.excluded == Some(ExcludeReason::ScheduledTask)).count(), 2);
    }

    #[test]
    fn firefox_default_browser_agent_task_does_not_count() {
        let ff = r"C:\Program Files\Mozilla Firefox";
        let programs = [prog("Firefox", Some(ff))];
        let agent = format!(r"{ff}\default-browser-agent.exe");
        let hits = [hit(&agent, 0, UsageSource::Bam)];
        let r = run_with_tasks(&programs, &hits, &HashSet::from([norm(Path::new(&agent))]));
        assert_eq!(usage(&r, "Firefox").last_used.when, None);
        assert_eq!(usage(&r, "Firefox").exes[0].excluded, Some(ExcludeReason::ScheduledTask));
    }

    #[test]
    fn service_exe_chosen_as_display_icon_never_counts() {
        // Office: the display icon is the ClickToRun service binary, outside the install folder.
        let svc = r"C:\Program Files\Common Files\Microsoft Shared\ClickToRun\OfficeClickToRun.exe";
        let mut p = prog("Office", Some(r"C:\Program Files\Microsoft Office"));
        p.display_icon = Some(PathBuf::from(svc));
        let services = HashSet::from([norm(Path::new(svc))]);
        let none = HashMap::new();
        let hits = [
            hit(r"\PROGRAM FILES\COMMON FILES\MICROSOFT SHARED\CLICKTORUN\OFFICECLICKTORUN.EXE", 0, UsageSource::Prefetch),
            hit(r"C:\Program Files\Microsoft Office\root\Office16\EXCEL.EXE", 9, UsageSource::Bam),
        ];
        let roots = HashMap::from([(ProgramId("Office".into()), vec![(PathBuf::from(r"C:\Program Files\Microsoft Office\root\Office16\EXCEL.EXE"), 5)])]);
        let r = match_usage(&input(std::slice::from_ref(&p), &hits, &[], &roots, &none, &services));
        let u = usage(&r, "Office");
        assert_eq!(u.last_used.when, Some(days_ago(9)));
        // The service binary no longer claims anything by name, so it does not count at all.
        assert!(u.exes.iter().all(|e| !e.exe_path.to_string_lossy().contains("CLICKTORUN") || e.excluded.is_some()));
        // A scheduled task that runs the main exe is an autostart of the program: it counts.
        let tasks = HashSet::from([norm(Path::new(svc))]);
        let r = run_with_tasks(std::slice::from_ref(&p), &hits[..1], &tasks);
        assert_eq!(usage(&r, "Office").last_used.when, Some(days_ago(0)));
        assert_eq!(main_exes_skipping(&p, &[], &[], &[], &|e| norm(e) == norm(Path::new(svc))), Vec::<PathBuf>::new());
    }

    #[test]
    fn userassist_installer_hit_does_not_count() {
        let programs = [prog("Docker", Some(r"C:\Program Files\Docker\Docker"))];
        let hits = [
            hit(r"C:\Program Files\Docker\Docker\Docker Desktop Installer.exe", 0, UsageSource::UserAssist),
            hit(r"C:\Program Files\Docker\Docker\tools\helper2.exe", 3, UsageSource::UserAssist),
        ];
        let r = run(&programs, &hits, &HashSet::new());
        let u = usage(&r, "Docker");
        assert_eq!(via_of(u), (Some(days_ago(3)), Some("helper2.exe")));
        assert!(u.exes.iter().any(|e| e.excluded == Some(ExcludeReason::NamePattern)));
    }

    #[test]
    fn setup_named_main_exe_counts_from_prefetch() {
        let vsi = r"C:\Program Files (x86)\Microsoft Visual Studio\Installer";
        let programs = [prog("VSI", Some(vsi))];
        let setup = format!(r"{vsi}\setup.exe");
        let hits = [
            hit(&setup, 5, UsageSource::Prefetch),
            hit(&setup, 3, UsageSource::Bam),
            hit(&setup, 1, UsageSource::UserAssist),
        ];
        let shortcuts = targets(&[&setup]);
        let roots = HashMap::new();
        let none = HashMap::new();
        let services = HashSet::new();
        let r = match_usage(&input(&programs, &hits, &shortcuts, &roots, &none, &services));
        let u = usage(&r, "VSI");
        assert!(u.exes[0].is_main);
        assert_eq!(u.exes[0].excluded, None);
        // The UserAssist entry is dropped; the newest trace counts.
        assert_eq!(via_of(u), (Some(days_ago(3)), None));
        // UserAssist alone does not count.
        let r = match_usage(&input(&programs, &hits[2..], &shortcuts, &roots, &none, &services));
        assert_eq!(usage(&r, "VSI").last_used.when, None);
    }

    #[test]
    fn userassist_bypass_dates_service_exe_only_from_userassist() {
        // A service binary the user once started from Start, running all the time since.
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let exe = r"C:\Apps\Foo\tray.exe";
        let hits = [
            UsageHit { run_count: Some(1), ..hit(exe, 365, UsageSource::UserAssist) },
            UsageHit { run_count: Some(40), ..hit(exe, 0, UsageSource::Prefetch) },
            hit(exe, 0, UsageSource::Bam),
        ];
        let r = run(&programs, &hits, &HashSet::from([norm(Path::new(exe))]));
        let u = usage(&r, "Foo");
        assert_eq!(u.last_used.when, Some(days_ago(365)));
        assert_eq!(u.last_used.source, Some(UsageSource::UserAssist));
        assert_eq!((u.exes[0].last_run, u.exes[0].source, u.exes[0].run_count), (days_ago(365), UsageSource::UserAssist, Some(1)));
        assert_eq!(u.exes[0].excluded, None);
    }

    #[test]
    fn userassist_bypass_dates_task_exe_only_from_userassist() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let exe = r"C:\Apps\Foo\agent.exe";
        let hits = [hit(exe, 365, UsageSource::UserAssist), hit(exe, 0, UsageSource::Bam)];
        let r = run_with_tasks(&programs, &hits, &HashSet::from([norm(Path::new(exe))]));
        let u = usage(&r, "Foo");
        assert_eq!(u.last_used.when, Some(days_ago(365)));
        assert_eq!(u.last_used.source, Some(UsageSource::UserAssist));
        assert_eq!(u.exes[0].excluded, None);
    }

    #[test]
    fn userassist_bypass_dates_name_pattern_exe_only_from_userassist() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let exe = r"C:\Apps\Foo\FooUpdater.exe";
        let hits = [hit(exe, 365, UsageSource::UserAssist), hit(exe, 0, UsageSource::Bam)];
        let r = run(&programs, &hits, &HashSet::new());
        let u = usage(&r, "Foo");
        assert_eq!(u.last_used.when, Some(days_ago(365)));
        assert_eq!(u.last_used.source, Some(UsageSource::UserAssist));
        // Without a user launch the exe does not count at all.
        let r = run(&programs, &hits[1..], &HashSet::new());
        assert_eq!(usage(&r, "Foo").last_used.when, None);
    }

    #[test]
    fn autostart_task_running_main_exe_counts() {
        // MSI Afterburner: clicked once from Start a year ago, started at every logon by a task.
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let exe = r"C:\Apps\Foo\foo.exe";
        let hits = [hit(exe, 365, UsageSource::UserAssist), hit(exe, 0, UsageSource::Bam)];
        let r = run_with_tasks(&programs, &hits, &HashSet::from([norm(Path::new(exe))]));
        let u = usage(&r, "Foo");
        assert!(u.exes[0].is_main);
        assert_eq!(u.exes[0].excluded, None);
        assert_eq!(via_of(u), (Some(days_ago(0)), None));
        assert_eq!(u.last_used.source, Some(UsageSource::Bam));
        assert!(u.last_used.autostart);
        assert!(u.exes[0].autostart);
        // A newer user launch wins: the user clicked it, no autostart marker.
        let hits = [hit(exe, 0, UsageSource::UserAssist), hit(exe, 2, UsageSource::Bam)];
        let r = run_with_tasks(&programs, &hits, &HashSet::from([norm(Path::new(exe))]));
        let u = usage(&r, "Foo");
        assert_eq!(u.last_used.source, Some(UsageSource::UserAssist));
        assert!(!u.last_used.autostart);
    }

    #[test]
    fn run_key_target_is_autostart() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let exe = r"C:\Apps\Foo\foo.exe";
        let hits = [hit(exe, 0, UsageSource::Prefetch)];
        let (shortcuts, roots, args, services) = (targets(&[exe]), HashMap::new(), HashMap::new(), HashSet::new());
        let autostart = HashSet::from([norm(Path::new(exe))]);
        let r = match_usage(&MatchInput { autostart_exes: &autostart, ..input(&programs, &hits, &shortcuts, &roots, &args, &services) });
        assert!(usage(&r, "Foo").last_used.autostart);
        // FileTime never carries the marker.
        let hits = [hit(exe, 0, UsageSource::FileTime)];
        let r = match_usage(&MatchInput { autostart_exes: &autostart, ..input(&programs, &hits, &shortcuts, &roots, &args, &services) });
        assert!(!usage(&r, "Foo").last_used.autostart);
    }

    #[test]
    fn squirrel_run_value_marks_process_start_target_autostart() {
        use crate::registry::{FakeRegistry, RegValue};
        let install = r"C:\Users\A\AppData\Local\Discord";
        let mut reg = FakeRegistry::new();
        let run = key(Hive::CurrentUser, r"Software\Microsoft\Windows\CurrentVersion\Run");
        reg.set(run, "Discord", RegValue::Str(format!(r#""{install}\Update.exe" --processStart Discord.exe"#)));
        let autostart = crate::leftovers::reg_scan::run_value_exes(&reg);
        assert!(autostart.contains(&norm(Path::new(&format!(r"{install}\Update.exe")))), "{autostart:?}");
        let programs = [prog("Discord", Some(install))];
        let exe = format!(r"{install}\app-1.0.9253\Discord.exe");
        let hits = [hit(&exe, 0, UsageSource::Prefetch)];
        let (shortcuts, roots, args, services) = (targets(&[]), HashMap::new(), HashMap::new(), HashSet::new());
        let r = match_usage(&MatchInput { autostart_exes: &autostart, ..input(&programs, &hits, &shortcuts, &roots, &args, &services) });
        let u = usage(&r, "Discord");
        assert!(u.exes[0].autostart, "{u:?}");
        assert!(u.last_used.autostart);
        // A Startup shortcut at Update.exe names the same target.
        let from_shortcut = autostart_targets(Path::new(&format!(r"{install}\Update.exe")), "--processStart Discord.exe");
        assert!(from_shortcut.contains(&norm(Path::new(&format!(r"{install}\Discord.exe")))), "{from_shortcut:?}");
        // Only a name-excluded launcher's arguments count.
        assert_eq!(autostart_targets(Path::new(r"C:\Apps\Foo\foo.exe"), "--open bar.exe"), vec![norm(Path::new(r"C:\Apps\Foo\foo.exe"))]);
    }

    #[test]
    fn program_main_exes_skip_services_only() {
        let p = prog("Foo", Some(r"C:\Apps\Foo"));
        let shortcuts = targets(&[r"C:\Apps\Foo\agent.exe"]);
        let roots = vec![(PathBuf::from(r"C:\Apps\Foo\agent.exe"), 100), (PathBuf::from(r"C:\Apps\Foo\app.exe"), 50)];
        let agent = HashSet::from([norm(Path::new(r"C:\Apps\Foo\agent.exe"))]);
        let (mains, rule) = program_main_exes(&p, &shortcuts, &roots, &[], &agent);
        assert_eq!((mains, rule), (paths(&[r"C:\Apps\Foo\app.exe"]), MainRule::LargestRootExe));
        let (mains, rule) = program_main_exes(&p, &shortcuts, &roots, &[], &HashSet::new());
        assert_eq!((mains, rule), (paths(&[r"C:\Apps\Foo\agent.exe"]), MainRule::Shortcut));
    }

    #[test]
    fn cached_setup_display_icon_gives_never() {
        let mut p = prog("SSMS", None);
        p.display_icon = Some(PathBuf::from(r"C:\ProgramData\Package Cache\{ABC}\SSMS-Setup-ENU.exe"));
        p.uninstall_string = Some(r#""C:\ProgramData\Package Cache\{ABC}\SSMS-Setup-ENU.exe" /uninstall"#.into());
        let hits = [
            hit(r"C:\ProgramData\Package Cache\{ABC}\SSMS-Setup-ENU.exe", 0, UsageSource::Bam),
            hit(r"\PROGRAMDATA\PACKAGE CACHE\{ABC}\SSMS-SETUP-ENU.EXE", 0, UsageSource::Prefetch),
        ];
        let r = run(&[p], &hits, &HashSet::new());
        assert_eq!(usage(&r, "SSMS").last_used.when, None);
    }

    #[test]
    fn old_saved_exclude_reasons_still_deserialize() {
        assert_eq!(serde_json::from_str::<ExcludeReason>("\"NotMain\"").unwrap(), ExcludeReason::NotMain);
        assert_eq!(serde_json::from_str::<ExcludeReason>("\"ScheduledTask\"").unwrap(), ExcludeReason::ScheduledTask);
    }

    #[test]
    fn service_exe_still_excluded() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let hits = [hit(r"C:\Apps\Foo\bin\agent.exe", 0, UsageSource::Bam)];
        let services = HashSet::from([norm(Path::new(r"C:\Apps\Foo\bin\agent.exe"))]);
        let r = run(&programs, &hits, &services);
        assert_eq!(usage(&r, "Foo").exes[0].excluded, Some(ExcludeReason::Service));
        assert_eq!(usage(&r, "Foo").last_used.when, None);
    }

    #[test]
    fn name_claimed_non_main_hit_does_not_count() {
        // No install folder, so the uninstaller's file name claims `remove.exe` hits: not a subcomponent.
        let mut p = prog("Baz", None);
        p.uninstall_string = Some(r#""C:\x\remove.exe" /SILENT"#.to_string());
        let r = run(&[p], &[hit(r"D:\elsewhere\REMOVE.EXE", 0, UsageSource::Bam)], &HashSet::new());
        let u = usage(&r, "Baz");
        assert_eq!(u.exes[0].excluded, Some(ExcludeReason::NotMain));
        assert_eq!(u.last_used.when, None);
    }

    #[test]
    fn filetime_fallback_never_sets_via() {
        let programs = [prog("Foo", Some(r"C:\Apps\Foo"))];
        let r = run(&programs, &[hit(r"C:\Apps\Foo\tool.exe", 3, UsageSource::FileTime)], &HashSet::new());
        assert_eq!(via_of(usage(&r, "Foo")), (Some(days_ago(3)), None));
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
