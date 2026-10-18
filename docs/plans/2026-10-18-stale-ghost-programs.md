# Stale Uninstaller – Plan 3: Ghost Programs, Account Warning and Plan-2 Follow-ups Implementation Plan

**Goal:** A "Spökprogram" view that finds programs which are already uninstalled but left folders, registry keys, dangling references and execution traces behind, groups the evidence per program with an assessment, and lets the user quarantine the leftovers or mark them "Behåll"; plus the start-up warning for an elevated run under another account, and the items parked at the end of plan 2.

**Architecture:** `stale-core` gains `ghosts/` (evidence collectors over the existing `FsReader`/`RegistryReader`/shortcut/task/service sources, a pure grouper + assessor, and a converter to `Leftover` items so the existing leftovers view and quarantine are reused unchanged), `keep.rs` (the persistent "viktig/behåll" list that every scanner consults), and `session.rs` (interactive-session user vs token user). `stale-gui` gains `View::Ghosts`, a worker, and a view that hands selected ghosts to the existing `LeftoverSelection`/quarantine flow.

**Tech Stack:** as before (Rust, eframe/egui 0.37, winreg, windows crate, serde_json, chrono).

**Spec:** `docs/design/2026-10-17-stale-uninstaller-design.md` — §6.3 (ghost programs), §5 "Vilka användare omfattas" (start-up warning), §9 only the "viktig-lista" storage (the junk finder itself is a later plan). Plan 2 (`2026-10-18-stale-uninstall-flow.md`) built everything reused here: `leftovers::{Leftover, LeftoverKind, Confidence, ProgramHint, LeftoverReport, names::*, fs_scan::FsScanRoots, reg_scan::RegScanRoots, other_scan::*, scan::LeftoverSources}`, `fsys::FsReader`, `quarantine::*`, `acl::*`, GUI `LeftoverSelection`, `View`, workers.

## Global Constraints

- Windows 10/11 x64; `stale-core` never depends on egui; OS access through the existing traits/closures so tests use `FakeFs`/`FakeRegistry`/closures; tests never touch the real registry/disk outside tempdirs.
- Anti-goal §2 unchanged: nothing the user owns may vanish. Ghost items are never High-confidence Dir/File unless a dangling reference or a missing-path registry value points at exactly that folder; name-only folder matches are Medium; AppData-only ghosts are Low ("Osäker"). User folders (Dokument etc.) are never scanned for ghosts.
- Anything under a kept path, or any kept registry key, or any kept ghost name, is excluded from every scanner (leftovers and ghosts) — the keep list is consulted in `scan_leftovers` and `scan_ghosts`.
- Ghost removal goes through the existing `LeftoverSelection` → `spawn_quarantine` path; no new deletion code.
- All UI strings Swedish in `strings.rs`; long work on worker threads; commit per task with the session's trailer lines.

## Review Focus

1. **A folder in Program Files belonging to a program that has no Uninstall key by design** (portable tools, games from a launcher, drivers): it must appear only as "Rester utan installation"/Medium, never as "Troligen avinstallerat", and must be one click from "Behåll". Pinned in Task 6. *Amended in the final fix wave:* only "Troligen avinstallerat" is pre-checked ("Rester utan installation" is listed unchecked). A folder holding the existing target of a live service, driver, task, Run/App Paths/COM/shell entry, shortcut or surviving executable is "Osäker" (`in-use`, its items Low). Drivers are a non-goal: kernel and file-system drivers (`Type` 1/2) are never evidence, only Win32 services (`Type` 16/32) are. Launcher libraries (`LAUNCHER_FOLDERS`: Epic Games, Steam, steamapps, GOG Galaxy, XboxGames, Battle.net, Origin Games, EA Games, Ubisoft Game Launcher, Riot Games, Games) are not publisher containers; a folder in one is "Osäker" (`launcher-library`).
2. **A dangling Run value whose target is on a currently disconnected drive**: must not be listed as a ghost (the drive may come back). Pinned in Task 4.
3. **An installed program whose name is the first word of a ghost name** ("Python" installed, ghost folder "Python Launcher"): the ghost is "Osäker", unchecked. Pinned in Task 6.
4. **Keep list persistence**: marking "Behåll" survives restart and hides the ghost and its paths from both the ghost scan and the leftover scan. Pinned in Tasks 1 and 6.
5. **Elevated under another account**: the warning appears, names both accounts, and the queue summary shows the owning account for `Scope::OtherUser` programs. Pinned in Task 7.

---

### Task 1: Keep list ("Behåll"/viktig-lista)

**Files:**
- Create: `crates/stale-core/src/keep.rs`
- Modify: `lib.rs`, `leftovers/scan.rs` (filter), `paths.rs` (`keep_list_path()`)

**Interfaces:**
```rust
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)] #[serde(default)]
pub struct KeepList { pub paths: Vec<PathBuf>, pub keys: Vec<RegKeyPath>, pub names: Vec<String> }
impl KeepList {
    pub fn load_from(path: &Path) -> KeepList;           // defaults on missing/unparsable
    pub fn save_to(&self, path: &Path) -> std::io::Result<()>;   // atomic (tmp + rename)
    pub fn is_path_kept(&self, p: &Path) -> bool;        // norm() equal or strictly under a kept path (`\` boundary)
    pub fn is_key_kept(&self, k: &RegKeyPath) -> bool;   // equal or under (case-insensitive)
    pub fn is_name_kept(&self, name: &str) -> bool;      // normalize_name equality
    pub fn keep_path(&mut self, p: PathBuf); pub fn keep_key(&mut self, k: RegKeyPath); pub fn keep_name(&mut self, n: String);   // dedupe
    pub fn remove_path(&mut self, p: &Path); pub fn remove_key(&mut self, k: &RegKeyPath); pub fn remove_name(&mut self, n: &str);
}
pub fn paths::keep_list_path() -> PathBuf;   // data_dir()\important.json
pub fn leftovers::scan::filter_kept(report: LeftoverReport, keep: &KeepList) -> LeftoverReport;   // drops Dir/File under kept paths, RegKey/RegValue under kept keys, Shortcut whose target is under a kept path, Service/ScheduledTask whose image/command is under a kept path
```
`LeftoverSources` gains `pub keep: KeepList` (live: loaded from `keep_list_path()`); `scan_leftovers` applies `filter_kept` last.

- [ ] **Step 1: Write failing tests**: `keep_list_roundtrip_and_defaults`; `is_path_kept_requires_boundary` (`C:\Apps\Foo` kept → `C:\Apps\Foo\x` true, `C:\Apps\Foobar` false); `is_key_kept_under`; `filter_kept_drops_items_under_kept_paths_and_keys` (one of each kind); `keep_list_dedupes`.
- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core keep`
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run tests, expect pass**
- [ ] **Step 5: Commit** — `feat(core): keep list and leftover filtering`

---

### Task 2: Plan-2 follow-ups (core)

**Files:**
- Modify: `crates/stale-core/src/acl.rs` (sweep skips files), `quarantine/ops.rs` (service status via API; restore re-inherits ACL), `leftovers/fs_scan.rs` (user-data subfolder note)

**Interfaces:**
```rust
// acl.rs: the child sweep in secure_dir only considers directories and reparse points; plain files (settings.json, logs) are never renamed aside.
// ops.rs
pub fn service_state(name: &str) -> Result<Option<u32>, String>;   // QueryServiceStatusEx via OpenSCManagerW/OpenServiceW (windows feature Win32_System_Services): Some(dwCurrentState) or None when the service does not exist (ERROR_SERVICE_DOES_NOT_EXIST); replaces parse_sc_state; SERVICE_STOPPED == 1
// wait loop uses service_state; `sc stop` still via proc; `sc delete` → DeleteService via the same handles (keep `sc delete` if simpler, but the state check must be API-based)
// restore: after a successful move back, call acl::reinherit(path) = SetNamedSecurityInfoW(DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION, empty DACL) so the item inherits its parent's ACL again; failure → Failure note "acl-not-reinherited", restore still counts
// acl.rs
pub fn reinherit(path: &Path) -> Result<(), String>;
// fs_scan.rs: when install_location contains a direct child named (case-insensitive) userdata, saves, saved, savegames, screenshots, profiles, workspace or projects, the High Dir item gets Confidence::Medium and note "contains-user-data" (GUI: NOTE_CONTAINS_USER_DATA "innehåller troligen egna filer (t.ex. sparfiler)")
```

- [ ] **Step 1: Write failing tests**: `sweep_leaves_files_alone` (acl, fake-free: tempdir with a file + an untrusted-looking dir name — use the injected `DirOps` closure pattern already in acl.rs); `install_location_with_userdata_child_is_medium_with_note` (fs_scan, FakeFs); `reinherit_is_called_after_restore` (ops, via a new `QuarantineOps.reinherit: Box<dyn Fn(&Path) -> Result<(), String> + Send + Sync>` recorded by the fake); `service_state_none_when_missing` is live-only (`#[ignore]`, queries a nonexistent service name).
- [ ] **Step 2: Run, expect failure**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run tests, expect pass**; run the ignored service-state test once.
- [ ] **Step 5: Commit** — `fix(core): plan-2 follow-ups (sweep files, service API, ACL re-inherit, user-data note)`

---

### Task 3: Ghost evidence — orphan folders

**Files:**
- Create: `crates/stale-core/src/ghosts/mod.rs`, `crates/stale-core/src/ghosts/folders.rs`
- Modify: `lib.rs`

**Interfaces:**
```rust
// ghosts/mod.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Evidence {
    Folder { path: PathBuf, size: Option<u64>, modified: Option<DateTime<Utc>>, root_kind: RootKind },
    RegKeyMissingPath { key: RegKeyPath, value_name: String, path: PathBuf },
    RegKeyOrphan { key: RegKeyPath },
    DanglingRef { kind: RefKind, location: String /* key path, lnk path, service name, task name */, target: PathBuf, value_name: Option<String> },
    UsageTrace { exe: PathBuf, last_run: DateTime<Utc>, source: UsageSource },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] pub enum RootKind { ProgramFiles, ProgramData, AppData, LocalPrograms }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] pub enum RefKind { Run, AppPaths, ClassesServer, ShellCommand, Service, ScheduledTask, Shortcut }
#[derive(Clone, Debug, PartialEq)]
pub struct InstalledIndex { names: Vec<String> /* normalized */, locations: Vec<String> /* norm()ed install locations, icon dirs, uninstaller dirs */, first_words: Vec<String> }
impl InstalledIndex { pub fn from_programs(programs: &[Program]) -> InstalledIndex; pub fn matches_name(&self, name: &str) -> bool /* name_matches against any installed name */; pub fn matches_first_word(&self, name: &str) -> bool; pub fn covers_path(&self, p: &Path) -> bool /* equal or under any location */; }
// ghosts/folders.rs
pub fn orphan_folders(fs: &dyn FsReader, roots: &FsScanRoots, installed: &InstalledIndex, keep: &KeepList) -> Vec<Evidence>;
```
Rules: for each root in `program_files`, `program_data`, per-profile `AppData\Roaming`, `AppData\Local`, `AppData\Local\Programs` (never Temp, never user folders): each direct child dir `c` that is not a shared root, not kept, not `installed.covers_path`, and whose name is `is_matchable` and not `installed.matches_name` → candidate. If `c` has 2+ subdirs and no files at top level and its name is not a known product (heuristic: treat as a publisher container) → recurse one level and apply the same test to its children; the container itself is never listed. Skip well-known system/vendor folders by name: `Microsoft`, `Microsoft Shared`, `Common Files`, `Windows*`, `WindowsApps`, `Packages`, `Temp`, `Google`→(container rule applies), `Internet Explorer`, `Windows Defender*`, `Windows NT`, `Windows Mail`, `Windows Media Player`, `Windows Photo Viewer`, `Windows Portable Devices`, `Windows Security`, `WindowsPowerShell`, `ModifiableWindowsApps`, `dotnet`, `MSBuild`, `Reference Assemblies`, `Uninstall Information`, `Package Cache`, `Intel`, `NVIDIA Corporation`, `AMD`, `Realtek`, `Dell`, `HP`, `Lenovo`, `ASUS`, `Microsoft OneDrive`, `Mozilla` (container), `Adobe` (container), `Common`, `Crashpad`, `D3DSCache`, `ConnectedDevicesPlatform`, `Comms`, `Publishers`, `SquirrelTemp`, `Programs`, `CEF`, `pip`, `npm`, `npm-cache`, `nuget`, `.cache`-style dot folders. Put the list in `ghosts::SKIP_FOLDERS` as a const. Sizes via `fs.dir_size`; `modified` via a new `FsReader::modified(&Path) -> Option<DateTime<Utc>>` (add to the trait, Real + Fake).

- [ ] **Step 1: Write failing tests** (FakeFs): `orphan_folder_in_program_files_is_evidence`; `installed_program_folder_is_not_orphan` (covers_path); `name_match_to_installed_is_not_orphan`; `publisher_container_descends_one_level`; `skip_list_and_shared_roots_ignored`; `kept_path_is_ignored`; `appdata_roots_scanned_per_profile`.
- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core ghosts`
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run tests, expect pass**
- [ ] **Step 5: Commit** — `feat(core): ghost evidence from orphan folders`

---

### Task 4: Ghost evidence — registry

**Files:**
- Create: `crates/stale-core/src/ghosts/registry.rs`

**Interfaces:**
```rust
pub const PATH_VALUE_NAMES: [&str; 8] = ["InstallPath", "InstallDir", "InstallLocation", "Install_Dir", "InstallRoot", "Path", "AppPath", "ExePath"];
pub fn registry_evidence(reg: &dyn RegistryReader, fs: &dyn FsReader, roots: &RegScanRoots, installed: &InstalledIndex, keep: &KeepList, drive_present: &dyn Fn(char) -> bool) -> Vec<Evidence>;
```
Rules: (a) under each `software` root: direct children and (for containers, i.e. keys with ≥2 subkeys and no path value of their own) their children: read values named in `PATH_VALUE_NAMES` (Str/ExpandStr, env-expanded, quotes stripped, trailing `\` trimmed); if the value is an absolute path on a present drive and `!fs.exists(path)` and `!installed.covers_path(path)` and not kept → `RegKeyMissingPath`. Skip keys named `Microsoft`, `Classes`, `Policies`, `Clients`, `RegisteredApplications`, `WOW6432Node`, `Wow6432Node`, `Intel`, `Google`→container, and any key matched by `installed.matches_name`. (b) `run` roots: each value whose `first_path_in_command` is absolute, on a present drive, missing on disk → `DanglingRef { kind: Run, location: key path, target, value_name }`. (c) `app_paths`: subkeys whose default path is missing → `DanglingRef { AppPaths }`. (d) `classes` roots: `CLSID\{…}` with `InprocServer32`/`LocalServer32` default missing → `ClassesServer` (location = the `{…}` key path); other direct subkeys whose `shell\open\command` default's exe is missing → `ShellCommand`. Same one-pass discipline as `reg_scan` (c). Drive-absent targets (`!drive_present(letter)`) are skipped everywhere (Review Focus 2). Keys kept via `keep.is_key_kept` skipped.

- [ ] **Step 1: Write failing tests** (FakeRegistry + FakeFs): `missing_install_path_value_is_evidence`; `existing_path_is_not_evidence`; `installed_name_key_is_skipped`; `dangling_run_value_is_evidence`; `run_value_on_absent_drive_is_skipped` (Review Focus 2); `dangling_clsid_server_is_evidence`; `kept_key_is_skipped`.
- [ ] **Step 2: Run, expect failure**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run tests, expect pass**
- [ ] **Step 5: Commit** — `feat(core): ghost evidence from the registry`

---

### Task 5: Ghost evidence — services, tasks, shortcuts, usage traces

**Files:**
- Create: `crates/stale-core/src/ghosts/refs.rs`

**Interfaces:**
```rust
pub fn service_evidence(reg: &dyn RegistryReader, fs: &dyn FsReader, drive_present: &dyn Fn(char) -> bool) -> Vec<Evidence>;   // ImagePath (normalize_image_path + first_path_in_command) missing on a present drive → DanglingRef { Service, location: service name }
pub fn task_evidence(fs: &dyn FsReader, tasks_root: &Path, drive_present: &dyn Fn(char) -> bool) -> Vec<Evidence>;             // <Command> missing → DanglingRef { ScheduledTask, location: task name }
pub fn shortcut_evidence(fs: &dyn FsReader, targets: &[ShortcutTarget], drive_present: &dyn Fn(char) -> bool) -> Vec<Evidence>; // target.path missing (and lnk exists) → DanglingRef { Shortcut, location: lnk path }
pub fn usage_evidence(fs: &dyn FsReader, unmatched: &[UsageHit], drive_present: &dyn Fn(char) -> bool) -> Vec<Evidence>;         // hits whose exe_path is absolute, on a present drive, missing → UsageTrace; drive-less (volume-stripped Prefetch) hits are resolved by trying every present drive letter: if the path exists on none → UsageTrace with the first present drive prepended (note: source Prefetch); keep the newest hit per norm(exe)
```
Skip `%SystemRoot%`-internal targets (System32/SysWOW64/WinSxS) for services/tasks (Windows' own dangling entries are not ghosts). Shortcuts: skip targets under `%SystemRoot%`.

- [ ] **Step 1: Write failing tests**: `missing_service_image_is_evidence`, `system32_service_is_skipped`, `missing_task_command_is_evidence`, `dangling_shortcut_is_evidence_and_existing_target_is_not`, `usage_trace_for_missing_exe_newest_only`, `driveless_prefetch_hit_resolved_against_present_drives`.
- [ ] **Step 2: Run, expect failure**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run tests, expect pass**
- [ ] **Step 5: Commit** — `feat(core): ghost evidence from services, tasks, shortcuts and usage traces`

---

### Task 6: Grouping, assessment, conversion and the scan entry point

**Files:**
- Create: `crates/stale-core/src/ghosts/group.rs`, `crates/stale-core/src/ghosts/scan.rs`
- Modify: `ghosts/mod.rs` (re-exports), `examples/dump.rs` (`dump ghosts`)

**Interfaces:**
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] pub enum Assessment { ProbablyUninstalled, LeftoversOnly, Uncertain }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ghost { pub name: String /* display: the most descriptive original name among its evidence */, pub key: String /* normalize_name */, pub publisher: Option<String>, pub evidence: Vec<Evidence>, pub assessment: Assessment, pub total_size: u64, pub last_used: Option<DateTime<Utc>> }
pub fn ghost_name_of(e: &Evidence) -> Option<String>;   // Folder → folder name; RegKeyMissingPath/RegKeyOrphan → key leaf (if the leaf is a version-like or generic name, the parent leaf); DanglingRef/UsageTrace → the first path component below a known root that is matchable (Program Files\Foo\bin\x.exe → "Foo"; AppData\Local\Programs\Foo → "Foo"; AppData\Local\Foo → "Foo"); None if nothing matchable
pub fn group(evidence: Vec<Evidence>, installed: &InstalledIndex, keep: &KeepList) -> Vec<Ghost>;   // group by normalize_name(ghost_name_of); drop groups whose name is kept (keep.is_name_kept) or matches an installed name; assessment per the table below; sort by assessment (ProbablyUninstalled, LeftoversOnly, Uncertain) then total_size desc
pub fn to_leftovers(g: &Ghost) -> LeftoverReport;   // Folder → Dir (Medium; Low + note "appdata-only" if the ghost is Uncertain); RegKeyMissingPath/RegKeyOrphan → RegKey Medium; DanglingRef Run → RegValue High; AppPaths/ClassesServer/ShellCommand → RegKey High (the key named in location); Service → Service High; ScheduledTask → ScheduledTask High (xml path from tasks_root + name); Shortcut → Shortcut High; UsageTrace → nothing (evidence only)
pub struct GhostSources { pub leftovers: LeftoverSources /* reuses fs, reg, roots, targets, keep */, pub programs: Vec<Program>, pub unmatched_hits: Vec<UsageHit>, pub drive_present: Box<dyn Fn(char) -> bool + Send + Sync> }
impl GhostSources { pub fn live(programs: Vec<Program>, unmatched_hits: Vec<UsageHit>) -> GhostSources; }
pub fn scan_ghosts(src: &GhostSources) -> Vec<Ghost>;
```
Assessment: `ProbablyUninstalled` if the group has ≥1 `DanglingRef` or `UsageTrace` or `RegKeyMissingPath`, and `!installed.matches_first_word(name)`; `Uncertain` if `installed.matches_first_word(name)` (Review Focus 3) or the only evidence is Folder(s) with `root_kind == AppData` (Review Focus 1 analogue); otherwise `LeftoversOnly`. `last_used` = newest `UsageTrace.last_run`.

- [ ] **Step 1: Write failing tests**: `ghost_name_of_picks_matchable_component`; `group_merges_folder_key_and_run_value_by_name`; `assessment_probably_uninstalled_needs_dangling_or_trace`; `assessment_uncertain_on_first_word_match_or_appdata_only` (Review Focus 1 & 3); `installed_name_group_is_dropped`; `kept_name_is_dropped` (Review Focus 4); `to_leftovers_confidences`; `scan_ghosts_end_to_end_with_fakes` (folder + run value + unmatched hit → one ghost ProbablyUninstalled).
- [ ] **Step 2: Run, expect failure**
- [ ] **Step 3: Implement**; `dump ghosts` prints one block per ghost: name, assessment, size, last used, each evidence line. Run it once non-elevated and paste the first 40 lines in the report.
- [ ] **Step 4: Run tests, expect pass**
- [ ] **Step 5: Commit** — `feat(core): ghost grouping, assessment and scan`

---

### Task 7: Session-account warning and OtherUser display

**Files:**
- Create: `crates/stale-core/src/session.rs`
- Modify: `crates/stale-gui/src/main.rs` or `app.rs` (start-up warning), `ui/uninstall_dialog.rs` (account line), `strings.rs`, `programs.rs` (`Program.account: Option<String>` for OtherUser — the SID's account name via `LookupAccountSidW`, feature `Win32_Security`), `model.rs`

**Interfaces:**
```rust
// session.rs
pub struct SessionIdentity { pub token_user: String /* DOMAIN\name */, pub session_user: Option<String> }
pub fn identity() -> SessionIdentity;   // GetUserNameExW(NameSamCompatible) for the token; WTSQuerySessionInformationW(WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTSUserName/WTSDomainName) for the interactive session (features Win32_System_RemoteDesktop, Win32_System_WindowsProgramming or SystemInformation as needed)
pub fn runs_as_other_account(id: &SessionIdentity) -> bool;   // session_user is Some and differs case-insensitively from token_user
// model.rs: Program gains `pub account: Option<String>` (serde default), filled for Scope::OtherUser by programs.rs via a `sid_to_account: &dyn Fn(&str) -> Option<String>` parameter on the internal reader (live: LookupAccountSidW; tests: closure)
```
GUI: at start-up (`StaleApp::new`), if `runs_as_other_account` → `warnings.push(strings::other_account_warning(session, token))` ("Du kör som {token} men är inloggad som {session}: per-användarprogram och AppData avser {token}."). Queue summary: for items with `scope == OtherUser` show `"{name} (konto: {account})"` and a line `strings::OTHER_ACCOUNT_QUEUE` ("Program från andra konton avinstalleras med dina administratörsrättigheter; kontrollera avinstallerarens sökväg nedan.") plus the uninstall command path (`first_path_in_command`) under the name.

- [ ] **Step 1: Write failing tests**: `runs_as_other_account_compares_case_insensitively` (pure); `other_user_program_gets_account_name` (programs.rs with closure); GUI `other_account_warning_text_is_swedish` (strings).
- [ ] **Step 2: Run, expect failure**
- [ ] **Step 3: Implement**; `#[ignore]` live test `identity_live_has_token_user`, run once.
- [ ] **Step 4: Run tests, expect pass**
- [ ] **Step 5: Commit** — `feat: warn when elevated under another account; show owning account in the queue`

---

### Task 8: GUI — ghosts view, Behåll, removal

**Files:**
- Create: `crates/stale-gui/src/ui/ghosts_view.rs`, `crates/stale-gui/src/ghost_sel.rs` (pure)
- Modify: `app.rs` (`View::Ghosts`, state, unmatched hits kept from `ScanEvent::UnmatchedHits`), `worker.rs` (`spawn_ghost_scan`), `ui/toolbar.rs` (`Spökprogram` button), `ui/leftovers_view.rs` (accepts a "back to ghosts" origin), `strings.rs`, `lib.rs`, `ui/mod.rs`

**Interfaces:**
```rust
// ghost_sel.rs
pub struct GhostSelection { pub ghosts: Vec<Ghost>, pub checked: Vec<bool> }
impl GhostSelection { pub fn new(ghosts: Vec<Ghost>) -> Self /* checked = assessment != Uncertain */; pub fn checked_reports(&self) -> Vec<(String, LeftoverReport)>; pub fn summary(&self) -> (usize, usize, u64); pub fn remove_by_key(&mut self, key: &str); }
// worker.rs
pub enum GhostEvent { Done(Vec<Ghost>), Failed(String) }
pub fn spawn_ghost_scan(programs: Vec<Program>, unmatched: Vec<UsageHit>) -> Receiver<GhostEvent>;   // thread "stale-ghosts", builds GhostSources::live inside
// app.rs
pub struct StaleApp { …, pub unmatched_hits: Vec<UsageHit>, pub ghosts: Option<GhostSelection>, ghost_rx: Option<Receiver<GhostEvent>>, pub keep: KeepList, pub leftovers_origin: View /* Programs or Ghosts */, … }
impl StaleApp { pub fn show_ghosts(&mut self); pub fn keep_ghost(&mut self, key: &str) /* adds the ghost's name and all its Folder paths + keys to keep, saves, removes it from the selection */; pub fn open_leftovers_from_ghosts(&mut self) /* checked_reports → LeftoverSelection, view = Leftovers, origin = Ghosts */; }
```
View: heading `strings::GHOSTS_TITLE` ("Spökprogram") + `Till programlistan` + `Uppdatera`; spinner + `strings::GHOSTS_SCANNING` ("Letar efter rester av avinstallerade program…") while scanning; empty state `strings::GHOSTS_EMPTY` ("Inga spökprogram hittades"); per ghost a row group: checkbox, name (bold), assessment badge (`strings::ASSESS_PROBABLY` "Troligen avinstallerat" in normal colour / `ASSESS_LEFTOVERS` "Rester utan installation" / `ASSESS_UNCERTAIN` "Osäker" in `STALE_COLOR`), size, last used (`relative_date`), and a collapsible evidence list (one line each: kind label from strings + path/key/target; `UsageTrace` shows `"körd {date} ({source})"`); per-row button `Behåll`. Footer: `"{n} av {m} markerade · {size}"` + `Visa rester för markerade` (→ `open_leftovers_from_ghosts`). In the leftovers view, `Till programlistan` becomes `Tillbaka` when `leftovers_origin == Ghosts` and returns to the ghosts view (refreshing it after a removal).
`apply_event(UnmatchedHits(h))` stores `unmatched_hits` (replace Task 9's ignore).

- [ ] **Step 1: Write failing tests**: `ghost_sel.rs`: `uncertain_ghosts_start_unchecked`, `checked_reports_convert_only_checked`, `remove_by_key`; `app.rs`: `keep_ghost_persists_name_and_paths_and_removes_row` (tempfile keep path via an internal `keep_ghost_at`), `unmatched_hits_are_stored`.
- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-gui ghost`
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run tests, expect pass**; `cargo clippy --workspace --all-targets` clean; release build.
- [ ] **Step 5: Commit** — `feat(gui): ghost programs view with keep list and removal`

---

## Self-review notes

- Spec coverage: §6.3 trace sources 1–4 (Tasks 3–5), grouping and assessment table (Task 6), presentation + Behåll + same confidence/quarantine (Tasks 1, 6, 8), user folders never scanned (Task 3 roots), §5 account warning (Task 7), §9 keep-list storage (Task 1). Parked plan-2 items (Task 2).
- Names consistent: `Evidence`, `RootKind`, `RefKind`, `InstalledIndex`, `Ghost`, `Assessment`, `GhostSources`, `scan_ghosts`, `to_leftovers`, `KeepList`, `filter_kept`, `GhostSelection`, `spawn_ghost_scan`, `GhostEvent`, `SessionIdentity`.
- Review Focus 1–5 pinned in Tasks 6, 4, 6, 1/6, 7.
