# Stale Uninstaller – Plan 2: Uninstall Queue, Leftovers and Quarantine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** From the program list, queue one or more programs, uninstall them (optionally via their own uninstaller, after an optional restore point), find what they left behind in files, registry, services, tasks and shortcuts, and move the chosen leftovers into a per-volume quarantine that can be undone or emptied.

**Architecture:** `stale-core` gains three modules with injectable OS access: `leftovers` (pure matching rules over a `FsReader` + `RegistryReader` view of the machine), `quarantine` (manifests under `%ProgramData%`, payload moved to `<drive>:\Stale Uninstaller\quarantine\<id>`, registry keys exported with `reg.exe` before deletion through a `RegistryWriter` trait), and `uninstall` (restore point, running the vendor uninstaller and waiting for its process tree, the §6.2 "not uninstalled" check, and an orchestrator that streams `UninstallEvent`s). `stale-gui` gains a view enum (program list / leftovers / quarantine), an uninstall summary + progress dialog, and worker threads for the three long operations.

**Tech Stack:** Rust stable MSVC, eframe/egui 0.37 (+ egui_extras tables), winreg, windows crate (Job objects, srclient), serde_json, chrono. `reg.exe` and `icacls.exe` (both in System32) are invoked as child processes.

**Spec:** `docs/superpowers/specs/2026-10-17-stale-uninstaller-design.md` — §4 toolbar, §6, §6.1, §6.2, §7. §6.3 (ghost programs) and §9 are plan 3.

Plan 1 (`docs/superpowers/plans/2026-10-17-stale-program-list.md`) built everything this plan calls: `Program`, `RegistryReader`/`FakeRegistry`, `pathutil::{norm, expand_env, first_path_in_command, exe_under, is_shared_root}`, `shortcuts::{ShortcutTarget, resolve_lnk, shortcut_targets, start_menu_dirs}`, `services::service_exes`, `size::dir_size`, `paths::{data_dir, logs_dir, user_profiles_root}`, `settings::Settings`, GUI `StaleApp { rows, checked, selected, … }`, `strings.rs`.

## Global Constraints

- Windows 10/11 x64; `stale-core` never depends on egui; every OS call in core goes through a trait or injected closure so tests use fakes (`FakeRegistry`, `FakeFs`, `FakeRegistryWriter`, closures). Tests never delete, move or write outside a `tempfile` dir and never touch the real registry.
- Anti-goal §2: nothing the user owns may vanish. Leftovers in Dokument/Bilder/Skrivbord/Nedladdningar are always `Low` and unchecked. Nothing is deleted outright: files move to quarantine, registry keys are exported before deletion.
- Quarantine payload root: `<drive>:\Stale Uninstaller\quarantine\<id>\` on the same volume as the file; manifests and `.reg` files in `%ProgramData%\Stale Uninstaller\quarantine\<id>\`. Admin-only ACL on every payload root. Auto-purge after `Settings.quarantine_days` (30, 0 = off) at start-up.
- Restore point: one per queue run, checkbox default on; a Windows refusal (24 h rule, System Restore off) is shown as information, never as failure.
- Vendor uninstaller: MSI → `msiexec /x {ProductCode}` (ProductCode = Uninstall key leaf when it is a `{GUID}`), else `UninstallString` verbatim; interactive; wait for the whole process tree; then §6.2: Uninstall key still exists AND install dir still has `*.exe` → `NotUninstalled`, no leftover scan, offer retry / forced.
- Name matching for leftovers: case-insensitive; strip version numbers, `(x64)`/`(x86)`/`64-bit`/`32-bit`, trailing `Setup`/`Installer`; names shorter than 4 characters or in the generic list (`Tools`, `Setup`, `Update`, `Updater`, `Common`, `Program`, `App`, `Software`, `Microsoft`, `Windows`, `Temp`, `Data`, `Local`, `Roaming`) never name-match.
- All UI strings Swedish in `strings.rs`. Long operations on worker threads; the window never blocks.
- Commit per task with the attribution trailer lines from the session.

## Review Focus

1. **A program whose InstallLocation is a shared root or the drive root** (e.g. `C:\Program Files\`): the leftover scan must not list that folder or its siblings; only name matches inside it. Pinned in Task 5.
2. **A locked file during quarantine move** (another process holds it): that file is reported, the rest of the entry still moves, the manifest records the partial result, undo still works for what moved. Pinned in Task 7.
3. **Undo when the original location is now occupied** (user reinstalled): the conflicting item stays in quarantine and is reported; nothing is overwritten. Pinned in Task 7.
4. **The vendor uninstaller spawns a child and exits immediately** (NSIS copies itself to Temp): the wait must cover the child, otherwise the leftover scan runs while the uninstaller is still deleting. Pinned in Task 8.
5. **Queue with several programs where one is cancelled by the user in the vendor dialog**: that program ends `NotUninstalled` and is absent from the leftover view; the others proceed. Pinned in Task 9.

---

### Task 1: Leftover model and name matching

**Files:**
- Create: `crates/stale-core/src/leftovers/mod.rs`, `crates/stale-core/src/leftovers/names.rs`
- Modify: `crates/stale-core/src/lib.rs` (add `pub mod leftovers;`)

**Interfaces:**
- Produces (`leftovers/mod.rs`):
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Confidence { High, Medium, Low }
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LeftoverKind {
    Dir { path: PathBuf, size: Option<u64> },
    File { path: PathBuf, size: Option<u64> },
    RegKey { key: RegKeyPath },
    RegValue { key: RegKeyPath, name: String },          // Run/RunOnce/App Paths entries
    Shortcut { lnk: PathBuf, target: PathBuf },
    Service { name: String, image: PathBuf },
    ScheduledTask { name: String, xml: PathBuf, command: PathBuf },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Leftover { pub kind: LeftoverKind, pub confidence: Confidence, pub note: Option<String> }  // note = Swedish-free key like "user-folder", "shared-publisher-key"; GUI maps to text
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProgramHint { pub name: String, pub publisher: Option<String>, pub install_location: Option<PathBuf>, pub uninstall_key: Option<RegKeyPath>, pub uninstall_string: Option<String>, pub display_icon: Option<PathBuf> }
impl ProgramHint { pub fn from_program(p: &Program) -> ProgramHint; pub fn manual(name: &str, dir: Option<PathBuf>) -> ProgramHint; }
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LeftoverReport { pub items: Vec<Leftover> }
impl Leftover { pub fn default_checked(&self) -> bool /* confidence != Low */; pub fn size(&self) -> Option<u64>; }
```
- Produces (`leftovers/names.rs`):
```rust
pub const GENERIC_NAMES: [&str; 14];     // the list in Global Constraints
pub fn normalize_name(s: &str) -> String;   // lowercase, strip version tokens (`3.6`, `v2`, `2024`), `(x64)` `(x86)` `64-bit` `32-bit`, trailing `setup`/`installer`, collapse whitespace, trim
pub fn is_matchable(name: &str) -> bool;    // normalized length >= 4 and not in GENERIC_NAMES
pub fn name_matches(candidate: &str, hint_name: &str) -> bool;   // normalize both; equal, or candidate equals the first word of hint when the hint has ≥2 words and that word is matchable (e.g. "Unity Hub" ↔ "UnityHub"/"Unity Hub"; also compare with spaces removed)
pub fn publisher_matches(candidate: &str, publisher: &str) -> bool;  // normalize, strip legal suffixes (inc, ltd, ab, gmbh, llc, corporation, corp, co), equal or spaces-removed equal
```

- [ ] **Step 1: Write failing tests in `names.rs`**

```rust
#[test] fn normalize_strips_versions_and_arch() { assert_eq!(normalize_name("Blender 3.6 (x64)"), "blender"); assert_eq!(normalize_name("Foo Setup"), "foo"); assert_eq!(normalize_name("7-Zip 23.01 (x64 edition)"), "7-zip"); }
#[test] fn generic_and_short_names_are_not_matchable() { assert!(!is_matchable("Tools")); assert!(!is_matchable("Git")); assert!(is_matchable("Unity Hub")); }
#[test] fn name_matches_handles_spaces_and_first_word() { assert!(name_matches("UnityHub", "Unity Hub")); assert!(name_matches("Unity", "Unity Hub")); assert!(!name_matches("Unityverse", "Unity Hub")); assert!(!name_matches("Tools", "Foo Tools")); }
#[test] fn publisher_matches_ignores_legal_suffix() { assert!(publisher_matches("Unity Technologies", "Unity Technologies ApS")); assert!(publisher_matches("DiscordInc", "Discord Inc.")); assert!(!publisher_matches("Microsoft", "Microsoft Corporation Edge Team")); }
```
And in `mod.rs`: `default_checked` is false only for `Low`; `ProgramHint::from_program` copies the fields; `manual("Foo", None)` has empty publisher.

- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core leftovers`
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run tests, expect pass**
- [ ] **Step 5: Commit** — `feat(core): leftover model and name matching`

---

### Task 2: File-system view and folder leftovers

**Files:**
- Create: `crates/stale-core/src/fsys.rs` (trait + real + fake), `crates/stale-core/src/leftovers/fs_scan.rs`
- Modify: `crates/stale-core/src/lib.rs`, `leftovers/mod.rs` (declare `fs_scan`)

**Interfaces:**
- Produces (`fsys.rs`):
```rust
#[derive(Clone, Debug, PartialEq, Eq)] pub struct DirEntryInfo { pub path: PathBuf, pub is_dir: bool, pub size: u64 }
pub trait FsReader: Send + Sync {
    fn list_dir(&self, dir: &Path) -> Vec<DirEntryInfo>;          // non-recursive; empty if unreadable
    fn exists(&self, p: &Path) -> bool;
    fn is_dir(&self, p: &Path) -> bool;
    fn dir_size(&self, p: &Path) -> Option<u64>;
    fn read_text(&self, p: &Path) -> Option<String>;               // for task XML
    fn profile_dirs(&self) -> Vec<PathBuf>;                        // every dir under %SystemDrive%\Users except Public/Default/All Users/Default User
    fn env(&self, var: &str) -> Option<String>;
}
pub struct RealFs;        // std::fs + size::dir_size + std::env
pub struct FakeFs { … }   // builder: FakeFs::new().dir("C:\\Program Files\\Foo", size).file("C:\\x\\y.txt", size).text("C:\\x\\task.xml", "<…>").env("APPDATA", "C:\\Users\\A\\AppData\\Roaming").profile("C:\\Users\\A")
```
- Produces (`fs_scan.rs`):
```rust
pub struct FsScanRoots { pub program_files: Vec<PathBuf>, pub program_data: PathBuf, pub per_profile: Vec<PathBuf> /* relative: AppData\Roaming, AppData\Local, AppData\Local\Programs, AppData\Local\Temp */, pub user_folders: Vec<PathBuf> /* relative: Documents, Pictures, Desktop, Downloads */ }
impl FsScanRoots { pub fn from_env(fs: &dyn FsReader) -> FsScanRoots; }   // %ProgramFiles%, %ProgramFiles(x86)%, %ProgramData%
pub fn scan_folders(fs: &dyn FsReader, roots: &FsScanRoots, hint: &ProgramHint) -> Vec<Leftover>;
```
Rules: (a) `install_location` itself, if it exists, is not a shared root and is not a drive root → `Dir` High. (b) In every root in `program_files`, `program_data`, and each `per_profile` relative dir under each profile: direct children whose name `name_matches` the hint name → `Dir` Medium; children whose name `publisher_matches` → look one level deeper for a child that `name_matches` → that child `Dir` Medium (the publisher folder itself is never listed). (c) In each `user_folders` relative dir under each profile: direct children that `name_matches` → `Dir` Low with note `"user-folder"`. (d) Skip any candidate equal to `install_location` (already listed) and any candidate that is a shared root. Sizes via `fs.dir_size`. Dedupe by `norm(path)`.

- [ ] **Step 1: Write failing tests** (all with `FakeFs`)

```rust
#[test] fn install_location_is_high() { … }
#[test] fn name_match_in_program_files_and_appdata_is_medium() { /* C:\Program Files\Unity Hub and C:\Users\A\AppData\Roaming\UnityHub → 2 Medium */ }
#[test] fn publisher_folder_then_name_is_medium_but_publisher_itself_not_listed() { /* C:\Program Files\Unity Technologies\Unity Hub → listed; parent not */ }
#[test] fn user_folders_are_low_with_note() { /* C:\Users\A\Documents\Unity Hub → Low, note user-folder */ }
#[test] fn shared_root_install_location_is_not_listed_and_siblings_untouched() { /* hint.install_location = C:\Program Files → no High item; C:\Program Files\Other not listed */ }
#[test] fn generic_name_only_matches_by_path() { /* hint name "Tools", folder C:\Program Files\Tools → not listed */ }
#[test] fn every_profile_is_scanned() { /* profiles A and B both have AppData\Local\Foo → 2 items */ }
```
- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core fs_scan`
- [ ] **Step 3: Implement** `fsys.rs` (RealFs uses `std::fs::read_dir`, `size::dir_size`, `paths::user_profiles_root`) and `fs_scan.rs`
- [ ] **Step 4: Run tests, expect pass**; add `#[ignore] real_fs_lists_program_files` smoke test
- [ ] **Step 5: Commit** — `feat(core): file-system view and folder leftover scan`

---

### Task 3: Registry leftovers

**Files:**
- Create: `crates/stale-core/src/leftovers/reg_scan.rs`
- Modify: `leftovers/mod.rs`

**Interfaces:**
```rust
pub struct RegScanRoots { pub software: Vec<RegKeyPath> /* HKLM\SOFTWARE, HKLM\SOFTWARE\WOW6432Node, HKCU\Software, plus HKU\<SID>\Software for loaded SIDs except .DEFAULT/_Classes */, pub classes: Vec<RegKeyPath> /* HKLM\SOFTWARE\Classes, HKCU\Software\Classes */, pub run: Vec<RegKeyPath> /* HKLM & HKCU ...\CurrentVersion\Run, RunOnce; WOW6432Node variants */, pub app_paths: Vec<RegKeyPath> /* HKLM & HKCU ...\CurrentVersion\App Paths; WOW6432Node */ }
impl RegScanRoots { pub fn standard(reg: &dyn RegistryReader) -> RegScanRoots; }
pub fn scan_registry(reg: &dyn RegistryReader, roots: &RegScanRoots, hint: &ProgramHint) -> Vec<Leftover>;
```
Rules: (a) `hint.uninstall_key` exists → `RegKey` High. (b) Under each `software` root: subkey `publisher_matches` → inside it, subkey `name_matches` → `RegKey` Medium; if the publisher key has **only** that one subkey (and no values), list the publisher key instead (Medium). Subkey directly under the root that `name_matches` → Medium. If a publisher key has other subkeys that do not match (shared publisher), the matched child gets note `"shared-publisher-key"` but stays Medium. (c) Under each `classes` root: subkeys `CLSID\{…}` whose `InprocServer32`/`LocalServer32` default value path is under `install_location` → `RegKey` High (the `{…}` key); other direct subkeys (file types, protocols) whose default value or `shell\open\command` default contains a path under `install_location` → High. Limit the CLSID walk to reading only `InprocServer32`/`LocalServer32` default values. (d) `run` roots: values whose `first_path_in_command` is under `install_location` → `RegValue` High. (e) `app_paths` roots: subkeys whose default value is under `install_location` → `RegKey` High. (f) Without `install_location`, (c)(d)(e) match on the exe file name of `display_icon`/`uninstall_string` (never `msiexec.exe`). Dedupe by key path.

- [ ] **Step 1: Write failing tests** with `FakeRegistry`: `uninstall_key_is_high`; `publisher_then_name_is_medium_and_lonely_publisher_key_is_listed_instead`; `shared_publisher_child_has_note`; `clsid_inproc_under_install_is_high`; `run_value_under_install_is_high`; `app_paths_key_is_high`; `generic_name_matches_nothing`; `classes_scan_reads_only_server_values` (FakeRegistry records which keys were read — add `FakeRegistry::reads()` returning the list of paths read).
- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core reg_scan`
- [ ] **Step 3: Implement**; add `reads()` tracking to `FakeRegistry` (a `RefCell<Vec<String>>`/`Mutex`).
- [ ] **Step 4: Run tests, expect pass**
- [ ] **Step 5: Commit** — `feat(core): registry leftover scan`

---

### Task 4: Services, scheduled tasks and shortcut leftovers

**Files:**
- Create: `crates/stale-core/src/leftovers/other_scan.rs`
- Modify: `crates/stale-core/src/shortcuts.rs` (`ShortcutTarget` gains `pub lnk: PathBuf`; `shortcut_targets` fills it), `scan.rs` + `usage/matching.rs` tests that construct `ShortcutTarget` (add the field), `leftovers/mod.rs`

**Interfaces:**
```rust
pub fn scan_services(reg: &dyn RegistryReader, hint: &ProgramHint) -> Vec<Leftover>;   // HKLM\SYSTEM\CurrentControlSet\Services\*: ImagePath (same normalisation as services::service_exes) under install_location → Service High
pub fn scan_tasks(fs: &dyn FsReader, tasks_root: &Path /* %SystemRoot%\System32\Tasks */, hint: &ProgramHint) -> Vec<Leftover>;   // recursive; each file is XML; extract every <Command>…</Command> text (simple string search, no XML crate), expand env, if under install_location → ScheduledTask High with name = path relative to tasks_root
pub fn scan_shortcuts(targets: &[ShortcutTarget], desktop_dirs_targets: &[ShortcutTarget], hint: &ProgramHint) -> Vec<Leftover>;   // target under install_location → Shortcut High
pub fn desktop_dirs(fs: &dyn FsReader) -> Vec<PathBuf>;   // %PUBLIC%\Desktop and <profile>\Desktop for every profile
```

- [ ] **Step 1: Write failing tests**: `service_with_image_under_install_is_high` (FakeRegistry with `\??\C:\Apps\Foo\svc.exe`); `task_xml_command_under_install_is_high` (FakeFs text with two `<Command>` elements, one matching); `task_with_env_var_command_matches` (`%ProgramFiles%\Foo\x.exe`); `shortcut_into_install_is_high_and_others_ignored`; `shortcut_target_carries_lnk_path`.
- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core other_scan`
- [ ] **Step 3: Implement**; `FsReader::list_dir` recursion for tasks is done in `scan_tasks` (depth unlimited; tasks tree is small).
- [ ] **Step 4: Run all tests, expect pass** (the `ShortcutTarget` field change touches existing tests)
- [ ] **Step 5: Commit** — `feat(core): service, scheduled-task and shortcut leftovers`

---

### Task 5: Leftover aggregation and the standalone search

**Files:**
- Create: `crates/stale-core/src/leftovers/scan.rs`
- Modify: `leftovers/mod.rs`

**Interfaces:**
```rust
pub struct LeftoverSources { pub reg: Arc<dyn RegistryReader>, pub fs: Arc<dyn FsReader>, pub fs_roots: FsScanRoots, pub reg_roots: RegScanRoots, pub tasks_root: PathBuf, pub start_menu_targets: Vec<ShortcutTarget>, pub desktop_targets: Vec<ShortcutTarget> }
impl LeftoverSources { pub fn live() -> LeftoverSources; }
pub fn scan_leftovers(src: &LeftoverSources, hint: &ProgramHint) -> LeftoverReport;   // runs Tasks 2–4 scanners, concatenates, dedupes (by norm(path) / key path / service name / task name / lnk), sorts: High, Medium, Low; within a group Dir/File first, then RegKey/RegValue, then Shortcut/Service/ScheduledTask
pub fn mark_safe(report: &LeftoverReport) -> Vec<bool>;   // true for High and Medium, false for Low  ("Markera säkra")
```
Standalone search (§6.1 last paragraph): callers build `ProgramHint::manual(name, dir)`; nothing else needed.

- [ ] **Step 1: Write failing tests**: `report_is_sorted_by_confidence_then_kind`; `duplicates_across_scanners_are_merged` (install_location also matched by name → one item, High); `shared_root_install_location_yields_no_dir_items_but_name_matches_elsewhere` (Review Focus 1); `mark_safe_unchecks_low_only`; `manual_hint_without_dir_matches_by_name_only`.
- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core leftovers::scan`
- [ ] **Step 3: Implement** (`live()` uses `WinRegistry`, `RealFs`, `shortcuts::shortcut_targets(start_menu_dirs())`, desktop dirs)
- [ ] **Step 4: Run tests, expect pass**; extend `examples/dump.rs` with an optional argument: `dump leftovers "<program name>"` prints the report for the first program whose name contains the argument (run it once elevated on a real program you still have installed, e.g. 7-Zip, and paste the output in the report — nothing is deleted).
- [ ] **Step 5: Commit** — `feat(core): leftover scan aggregation`

---

### Task 6: Registry writer and quarantine file moves

**Files:**
- Create: `crates/stale-core/src/regwrite.rs`, `crates/stale-core/src/quarantine/mod.rs` (types only for now), `crates/stale-core/src/quarantine/fsmove.rs`
- Modify: `lib.rs`, `paths.rs`

**Interfaces:**
```rust
// regwrite.rs
pub trait RegistryWriter: Send + Sync {
    fn export_key(&self, key: &RegKeyPath, to_file: &Path) -> Result<(), String>;   // reg.exe export "HKLM\…" file /y
    fn delete_key(&self, key: &RegKeyPath) -> Result<(), String>;                   // recursive, winreg delete_subkey_all with KEY_WOW64_64KEY
    fn delete_value(&self, key: &RegKeyPath, name: &str) -> Result<(), String>;
    fn import_file(&self, file: &Path) -> Result<(), String>;                       // reg.exe import file
}
pub struct WinRegistryWriter;
pub struct FakeRegistryWriter { pub log: Mutex<Vec<String>> /* "export HKLM\x -> file", "delete HKLM\x", "delete-value HKLM\x\Name", "import file" */, pub fail_on: Mutex<Vec<String>> }
// paths.rs additions
pub fn quarantine_manifest_dir() -> PathBuf;                 // data_dir()\quarantine (created)
pub fn quarantine_payload_root(drive: char) -> PathBuf;      // "<drive>:\Stale Uninstaller\quarantine"
// quarantine/mod.rs (types)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct QuarantineId(pub String);   // "<yyyymmdd-hhmmss>-<4 hex>"
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct MovedItem { pub original: PathBuf, pub stored: PathBuf, pub is_dir: bool, pub size: u64 }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct ExportedKey { pub key: RegKeyPath, pub reg_file: PathBuf }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct RemovedValue { pub key: RegKeyPath, pub name: String, pub reg_file: PathBuf }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct RemovedService { pub name: String, pub reg_file: PathBuf }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct RemovedTask { pub name: String, pub xml_backup: PathBuf, pub original_xml: PathBuf }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Failure { pub what: String, pub error: String }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuarantineManifest { pub id: QuarantineId, pub program_name: String, pub created: DateTime<Utc>, pub moved: Vec<MovedItem>, pub keys: Vec<ExportedKey>, pub values: Vec<RemovedValue>, pub services: Vec<RemovedService>, pub tasks: Vec<RemovedTask>, pub failures: Vec<Failure> }
impl QuarantineManifest { pub fn size_per_drive(&self) -> BTreeMap<char, u64>; pub fn total_size(&self) -> u64; }
// quarantine/fsmove.rs
pub trait FileMover: Send + Sync {
    fn move_path(&self, from: &Path, to: &Path) -> Result<(), String>;    // rename; on cross-volume error copy+delete (dirs recursive)
    fn ensure_payload_root(&self, root: &Path) -> Result<(), String>;      // create_dir_all + icacls (admins/system only) — icacls failure is logged, not fatal
    fn size_of(&self, p: &Path) -> u64;
    fn exists(&self, p: &Path) -> bool;
    fn remove_all(&self, p: &Path) -> Result<(), String>;
    fn volume_ok(&self, drive: char) -> bool;                              // NTFS and writable; else fall back to C:
}
pub struct RealFileMover;
pub struct FakeFileMover { … }   // in-memory tree; `lock(path)` makes move fail for that path; records calls
pub fn stored_path_for(root: &Path, id: &QuarantineId, original: &Path) -> PathBuf;   // root\<id>\<drive letter>\<path without drive>, e.g. D:\Stale Uninstaller\quarantine\<id>\D\Program Files\Unity Hub
```

- [ ] **Step 1: Write failing tests**: `stored_path_keeps_relative_structure`; `fake_mover_moves_and_reports_locked_file`; `fake_registry_writer_logs_operations_and_can_fail`; `manifest_size_per_drive_sums_moved_items`; `payload_root_uses_same_drive`.
- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core quarantine`
- [ ] **Step 3: Implement** (`RealFileMover::ensure_payload_root` runs `icacls <root> /inheritance:r /grant:r *S-1-5-32-544:(OI)(CI)F *S-1-5-18:(OI)(CI)F`; `WinRegistryWriter` builds `reg.exe` argument strings with `HKEY_LOCAL_MACHINE\…` full hive names)
- [ ] **Step 4: Run tests, expect pass**; `#[ignore]` live test `reg_export_import_roundtrip_in_hkcu_stale_test_key` (creates `HKCU\Software\StaleUninstallerTest`, exports, deletes, imports, verifies, deletes)
- [ ] **Step 5: Commit** — `feat(core): registry writer and quarantine file mover`

---

### Task 7: Quarantine operations

**Files:**
- Create: `crates/stale-core/src/quarantine/store.rs`, `crates/stale-core/src/quarantine/ops.rs`
- Modify: `quarantine/mod.rs`

**Interfaces:**
```rust
// store.rs — manifests on disk
pub fn manifest_path(id: &QuarantineId) -> PathBuf;                 // quarantine_manifest_dir()\<id>\manifest.json
pub fn save_manifest(dir: &Path, m: &QuarantineManifest) -> Result<(), String>;
pub fn list_manifests(dir: &Path) -> Vec<QuarantineManifest>;        // unreadable ones skipped, sorted newest first
pub fn new_id(now: DateTime<Utc>) -> QuarantineId;
// ops.rs
pub struct QuarantineOps { pub mover: Arc<dyn FileMover>, pub regw: Arc<dyn RegistryWriter>, pub reg: Arc<dyn RegistryReader>, pub fs: Arc<dyn FsReader>, pub manifest_dir: PathBuf, pub payload_root_for: Box<dyn Fn(char) -> PathBuf + Send + Sync>, pub tasks_root: PathBuf, pub stop_service: Box<dyn Fn(&str) -> Result<(), String> + Send + Sync> /* `sc stop` + `sc delete`, live */, pub delete_task: Box<dyn Fn(&str) -> Result<(), String> + Send + Sync> /* schtasks /delete /tn name /f */ }
impl QuarantineOps { pub fn live() -> QuarantineOps; }
pub enum QuarantineProgress { Item { done: usize, total: usize, what: String }, Finished(QuarantineManifest) }
pub fn quarantine(ops: &QuarantineOps, program_name: &str, items: &[Leftover], now: DateTime<Utc>, progress: &dyn Fn(QuarantineProgress)) -> QuarantineManifest;
   // order: services (export key → stop+delete), tasks (copy XML to manifest dir → schtasks delete), registry values (export parent key once → delete value), registry keys (export → delete), shortcuts (move .lnk), files/dirs (move; payload root chosen by drive, `volume_ok` else C:). Every failure → manifest.failures, continue. Manifest saved at the end (and after each group).
pub enum RestoreOutcome { Restored, Partial(Vec<Failure>) }
pub fn restore(ops: &QuarantineOps, m: &QuarantineManifest) -> RestoreOutcome;
   // reverse order; a moved item whose original path now exists → Failure "occupied", stays in quarantine; .reg imported; tasks re-created via `schtasks /create /xml`; services restored by importing their key (note: service needs reboot to start — recorded as note). Manifest updated: restored items removed; if everything restored, manifest dir and payload dirs deleted.
pub fn purge(ops: &QuarantineOps, m: &QuarantineManifest) -> Result<(), String>;   // delete payload dirs and manifest dir permanently
pub fn auto_purge(ops: &QuarantineOps, days: u32, now: DateTime<Utc>) -> Vec<QuarantineId>;   // 0 = off; purges manifests older than `days`
```

- [ ] **Step 1: Write failing tests** (FakeFileMover, FakeRegistryWriter, FakeRegistry, FakeFs, tempdir manifest dir): `quarantine_moves_files_exports_keys_and_saves_manifest`; `locked_file_is_recorded_and_rest_continues` (Review Focus 2); `payload_root_is_per_drive`; `restore_puts_everything_back_and_removes_manifest`; `restore_skips_occupied_original_and_keeps_manifest` (Review Focus 3); `purge_deletes_payload_and_manifest`; `auto_purge_respects_days_and_zero_means_off`; `service_and_task_are_exported_before_removal` (order asserted via logs).
- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core quarantine`
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run tests, expect pass**
- [ ] **Step 5: Commit** — `feat(core): quarantine, restore and purge`

---

### Task 8: Restore point and vendor-uninstaller runner

**Files:**
- Create: `crates/stale-core/src/uninstall/mod.rs`, `crates/stale-core/src/uninstall/restore_point.rs`, `crates/stale-core/src/uninstall/runner.rs`
- Modify: `lib.rs`

**Interfaces:**
```rust
// mod.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] pub enum UninstallMode { Normal, Forced }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub enum RestorePointStatus { Created(i64), SkippedFrequency, Disabled, Failed(String) }
#[derive(Clone, Debug, PartialEq)] pub enum VendorResult { Uninstalled { exit_code: i32 }, NotUninstalled { exit_code: Option<i32> }, NoUninstaller }
// restore_point.rs
pub trait RestorePointApi: Send + Sync { fn create(&self, description: &str) -> RestorePointStatus; }
pub struct SrClient;    // FFI: srclient.dll SRSetRestorePointW with BEGIN_SYSTEM_CHANGE/APPLICATION_UNINSTALL then END_SYSTEM_CHANGE; sequence 0 with success → SkippedFrequency; ERROR_SERVICE_DISABLED → Disabled
pub struct FakeRestorePoint(pub RestorePointStatus);
// runner.rs
pub fn uninstall_command(p: &Program) -> Option<Vec<String>>;    // MSI: ["msiexec.exe", "/x", "{GUID}"] when key leaf is a GUID (else from UninstallString); otherwise shell-split UninstallString (quoted first token + rest); None if empty or exe path missing on disk (via a passed `exists: &dyn Fn(&Path) -> bool`)
pub trait ProcessRunner: Send + Sync { fn run_and_wait_tree(&self, cmd: &[String]) -> Result<i32, String>; }   // exit code of the root process after the whole job is empty
pub struct JobObjectRunner;   // CreateJobObjectW + CREATE_SUSPENDED + AssignProcessToJobObject + ResumeThread; poll JOBOBJECT_BASIC_ACCOUNTING_INFORMATION.ActiveProcesses == 0 every 250 ms (JOB_OBJECT_LIMIT_BREAKAWAY_OK not set); timeout none
pub struct FakeProcessRunner { pub exit: i32, pub calls: Mutex<Vec<Vec<String>>> }
pub fn still_installed(reg: &dyn RegistryReader, fs: &dyn FsReader, p: &Program) -> bool;   // §6.2: uninstall key exists AND install_location has at least one *.exe
pub fn run_vendor_uninstaller(p: &Program, runner: &dyn ProcessRunner, reg: &dyn RegistryReader, fs: &dyn FsReader) -> VendorResult;
```

- [ ] **Step 1: Write failing tests**: `msi_command_uses_product_code`; `exe_command_keeps_arguments` (`"C:\Foo\unins000.exe" /SILENT` → 2 tokens); `missing_uninstaller_exe_is_none`; `still_installed_requires_key_and_exe`; `vendor_result_not_uninstalled_when_key_and_exe_remain` (FakeProcessRunner exit 1602, FakeRegistry still has key, FakeFs has exe); `vendor_result_uninstalled_when_key_gone`.
- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core uninstall`
- [ ] **Step 3: Implement**; `JobObjectRunner` needs windows features `Win32_System_JobObjects, Win32_System_Threading` (`cargo add -p stale-core windows --features <existing>,Win32_System_JobObjects,Win32_System_Threading`); `SrClient` via `#[link(name = "srclient")]` extern with `RESTOREPOINTINFOW`/`STATEMGRSTATUS` structs declared locally.
- [ ] **Step 4: Run tests, expect pass**; `#[ignore]` live test `job_runner_waits_for_child_tree` (`cmd /c "start /b cmd /c timeout 2 & exit 0"` must take ≥ 2 s — Review Focus 4) — run it once.
- [ ] **Step 5: Commit** — `feat(core): restore point and vendor uninstaller runner`

---

### Task 9: Uninstall orchestrator

**Files:**
- Create: `crates/stale-core/src/uninstall/orchestrator.rs`
- Modify: `uninstall/mod.rs`

**Interfaces:**
```rust
pub struct QueueOptions { pub restore_point: bool, pub run_vendor: bool /* false in Forced */, pub mode: UninstallMode }
pub struct QueueItem { pub program: Program }
#[derive(Clone, Debug)]
pub enum UninstallEvent {
    RestorePoint(RestorePointStatus),
    Started { index: usize },
    Vendor { index: usize, result: VendorResult },
    Leftovers { index: usize, report: LeftoverReport },
    NotUninstalled { index: usize, exit_code: Option<i32> },
    Warning(String),
    QueueDone,
}
pub struct UninstallSources { pub restore: Arc<dyn RestorePointApi>, pub runner: Arc<dyn ProcessRunner>, pub reg: Arc<dyn RegistryReader>, pub fs: Arc<dyn FsReader>, pub leftovers: Arc<LeftoverSources> }
impl UninstallSources { pub fn live() -> UninstallSources; }
pub fn run_queue(src: &UninstallSources, items: &[QueueItem], opts: &QueueOptions, tx: &Sender<UninstallEvent>);
   // restore point once (if opts.restore_point) → for each item: Started; if run_vendor: Vendor(result); NotUninstalled → emit NotUninstalled and skip leftovers; else Leftovers(scan_leftovers(hint)); Forced mode: no vendor step, straight to Leftovers; stops early if tx.send fails; QueueDone
```

- [ ] **Step 1: Write failing tests** (fakes; events collected): `queue_emits_restore_point_then_per_item_events_then_done`; `cancelled_vendor_uninstall_skips_leftovers_but_queue_continues` (Review Focus 5: item 0 FakeProcessRunner exit 1602 with key+exe remaining, item 1 fine → events `NotUninstalled{0}`, `Leftovers{1}`); `forced_mode_never_runs_vendor`; `no_uninstaller_falls_to_forced_with_warning`; `restore_point_skipped_is_information_not_failure`.
- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core orchestrator`
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run tests, expect pass**
- [ ] **Step 5: Commit** — `feat(core): uninstall queue orchestrator`

---

### Task 10: GUI — queue dialog, progress and workers

**Files:**
- Create: `crates/stale-gui/src/ui/uninstall_dialog.rs`, `crates/stale-gui/src/flow.rs` (pure state machine)
- Modify: `app.rs`, `worker.rs`, `ui/toolbar.rs`, `strings.rs`, `lib.rs`

**Interfaces:**
```rust
// flow.rs — pure, tested
#[derive(Clone, Debug, PartialEq)] pub enum Step { RestorePoint, Vendor, Leftovers, Done }
#[derive(Clone, Debug, PartialEq)] pub enum ItemState { Pending, Running, Uninstalled, NotUninstalled { exit_code: Option<i32> }, Skipped }
#[derive(Clone, Debug, PartialEq)]
pub struct FlowState { pub items: Vec<(ProgramId, String /*name*/, ItemState)>, pub step: Step, pub restore_point: Option<RestorePointStatus>, pub reports: Vec<(usize, LeftoverReport)>, pub warnings: Vec<String>, pub finished: bool }
impl FlowState { pub fn new(items: &[(ProgramId, String)]) -> Self; pub fn apply(&mut self, ev: UninstallEvent); pub fn all_reports_in(&self) -> bool /* finished && every non-NotUninstalled item has a report */; }
// worker.rs additions
pub fn spawn_uninstall(items: Vec<QueueItem>, opts: QueueOptions) -> Receiver<UninstallEvent>;
// app.rs additions
pub enum View { Programs, Leftovers, Quarantine }
pub struct PendingQueue { pub ids: Vec<ProgramId>, pub mode: UninstallMode, pub restore_point: bool, pub run_vendor: bool }   // the summary dialog's state
pub struct StaleApp { …, pub view: View, pub pending: Option<PendingQueue>, pub flow: Option<FlowState>, uninstall_rx: Option<Receiver<UninstallEvent>>, … }
impl StaleApp { pub fn start_queue(&mut self); pub fn open_queue_dialog(&mut self, mode: UninstallMode); }   // uses checked ∩ visible rows (per the Task 10 deferred note: only visible, present rows)
```
UI: `Avinstallera` and `Tvingad borttagning` enabled when `checked` is non-empty → `open_queue_dialog`. Dialog (`egui::Window` modal-ish, centered): list of names, checkbox "Skapa återställningspunkt för hela omgången" (default on), checkbox "Kör programmens egna avinstallerare" (default on, forced off and disabled in Forced mode), buttons Avbryt / Starta. Progress: the same window shows the step indicator (Återställningspunkt · Egen avinstallerare · Rester · Klart) and per-item status lines; when `flow.finished`, show "Visa rester" (→ `view = Leftovers`, Task 11) and, for `NotUninstalled` items, buttons "Försök igen" (re-queues that item alone, Normal) and "Tvingad borttagning" (re-queues it Forced).

- [ ] **Step 1: Write failing tests in `flow.rs`**: `apply_tracks_item_states_and_reports`; `all_reports_in_ignores_not_uninstalled_items`; `restore_point_status_recorded`; in `app.rs`: `open_queue_dialog_uses_only_visible_checked_rows`.
- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-gui flow`
- [ ] **Step 3: Implement** (`spawn_uninstall` thread "stale-uninstall" with `UninstallSources::live()`; drain `uninstall_rx` in `logic()` like the scan channel; after `QueueDone` call `refresh()` so the list drops uninstalled programs)
- [ ] **Step 4: Run tests, expect pass**; release build; the elevated manual run is the user's (do not bypass UAC) — record what to check: queue dialog opens for two checked rows; forced mode disables the vendor checkbox.
- [ ] **Step 5: Commit** — `feat(gui): uninstall queue dialog and progress`

---

### Task 11: GUI — leftovers view and removal

**Files:**
- Create: `crates/stale-gui/src/ui/leftovers_view.rs`, `crates/stale-gui/src/leftover_sel.rs` (pure selection model)
- Modify: `app.rs`, `worker.rs`, `strings.rs`, `ui/toolbar.rs` (Sök rester), `lib.rs`

**Interfaces:**
```rust
// leftover_sel.rs
pub struct LeftoverSelection { pub groups: Vec<LeftoverGroup> }
pub struct LeftoverGroup { pub program_name: String, pub items: Vec<Leftover>, pub checked: Vec<bool> }
impl LeftoverSelection { pub fn from_reports(reports: &[(String, LeftoverReport)]) -> Self /* checked = default_checked */; pub fn mark_safe(&mut self); pub fn checked_items(&self) -> Vec<(String, Vec<Leftover>)>; pub fn summary(&self) -> (usize /*checked*/, usize /*total*/, u64 /*bytes*/); }
// worker.rs
pub fn spawn_quarantine(batches: Vec<(String, Vec<Leftover>)>) -> Receiver<QuarantineEvent>;
pub enum QuarantineEvent { Progress { done: usize, total: usize, what: String }, Finished(Vec<QuarantineManifest>), Failed(String) }
// app.rs
pub struct StaleApp { …, pub leftovers: Option<LeftoverSelection>, quarantine_rx: Option<Receiver<QuarantineEvent>>, pub last_manifests: Vec<QuarantineManifest>, … }
impl StaleApp { pub fn open_leftovers_from_flow(&mut self); pub fn remove_checked_leftovers(&mut self); pub fn search_leftovers_manual(&mut self, name: String, dir: Option<PathBuf>); }
```
UI (`leftovers_view.rs`): per program a collapsible header; inside, three sections (Mappar och filer / Registernycklar / Övrigt) with counts and bytes; each item a checkbox + path/key + size at right; `Low` items show the warning icon and note text ("troligen dina egna filer" for `user-folder`, "delas med andra program från samma utgivare" for `shared-publisher-key`); confidence colour: Low in `STALE_COLOR`. Footer: "{n} av {m} markerade · {size} frigörs", buttons Markera säkra / Avbryt / Ta bort markerade. Removal shows a progress bar then a summary (per program: items moved, failures listed) with button "Till programlistan". `Sök rester` toolbar button opens a small window with name + optional folder, runs `search_leftovers_manual` on a worker (`spawn_leftover_scan(hint)`), and opens the same view.

- [ ] **Step 1: Write failing tests in `leftover_sel.rs`**: `defaults_follow_confidence`; `mark_safe_sets_high_medium_true_low_false`; `summary_counts_checked_bytes`; `checked_items_groups_by_program`.
- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-gui leftover_sel`
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run tests, expect pass**; release build.
- [ ] **Step 5: Commit** — `feat(gui): leftovers view and removal to quarantine`

---

### Task 12: GUI — quarantine view and start-up auto-purge

**Files:**
- Create: `crates/stale-gui/src/ui/quarantine_view.rs`
- Modify: `app.rs`, `worker.rs`, `ui/toolbar.rs` (Karantän button with count), `strings.rs`, `main.rs`

**Interfaces:**
```rust
// worker.rs
pub enum QuarantineAdminEvent { Listed(Vec<QuarantineManifest>), Restored { id: QuarantineId, outcome: RestoreOutcome }, Purged(QuarantineId), PurgedAll, Failed(String) }
pub fn spawn_quarantine_list() -> Receiver<QuarantineAdminEvent>;
pub fn spawn_restore(id: QuarantineId) -> Receiver<QuarantineAdminEvent>;
pub fn spawn_purge(ids: Vec<QuarantineId>) -> Receiver<QuarantineAdminEvent>;
// app.rs
pub struct StaleApp { …, pub quarantine: Vec<QuarantineManifest>, … }
```
UI: table Program · Datum · Storlek (per disk, e.g. "C: 38 MB · D: 41 MB") · Status (antal fel) with per-row buttons Ångra / Ta bort permanent, and a toolbar row with Töm karantän (confirm dialog: "Detta frigör {size} och kan inte ångras.") and Tillbaka. Restore outcome `Partial` shows the failures list. `main.rs`: before `run_native`, run `quarantine::auto_purge(&QuarantineOps::live(), settings.quarantine_days, Utc::now())` on a thread and log the purged ids. Settings menu (⚙) gains "Töm karantän automatiskt efter N dagar" as a `DragValue` 0..=365 bound to `settings.quarantine_days`, saved on change.

- [ ] **Step 1: Write failing tests in `app.rs`**: `quarantine_list_event_replaces_rows`; `restored_entry_is_removed_from_list_when_fully_restored_and_kept_when_partial`.
- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-gui quarantine`
- [ ] **Step 3: Implement**
- [ ] **Step 4: Run tests, expect pass**; `cargo clippy --workspace --all-targets` clean; release build. Manual elevated check (user): install a throwaway program (e.g. 7-Zip portable installer or Notepad++), uninstall it through Stale with restore point off, verify leftovers listed and moved, Ångra puts the files back, Ta bort permanent frees them.
- [ ] **Step 5: Commit** — `feat(gui): quarantine view and auto-purge`

---

## Self-review notes

- Spec coverage: §4 toolbar (Tasks 10–12), §6 flow and options (Tasks 8–10), §6.1 scan locations and confidence (Tasks 2–5), §6.1 standalone search (Tasks 5, 11), §6.2 (Tasks 8–10), §7 quarantine/undo/purge/auto-purge/ACL/per-volume (Tasks 6, 7, 12). §6.3 and §9 → plan 3. The deferred "elevated with another account" warning → plan 3.
- Names consistent across tasks: `Leftover`, `LeftoverKind`, `Confidence`, `ProgramHint`, `LeftoverReport`, `LeftoverSources`, `scan_leftovers`, `FsReader`, `RealFs`/`FakeFs`, `RegistryWriter`, `FileMover`, `QuarantineManifest`, `QuarantineOps`, `quarantine`/`restore`/`purge`/`auto_purge`, `RestorePointApi`, `ProcessRunner`, `run_vendor_uninstaller`, `UninstallEvent`, `run_queue`, `FlowState`, `LeftoverSelection`.
- Review Focus 1–5 pinned in Tasks 5, 7, 7, 8, 9.
