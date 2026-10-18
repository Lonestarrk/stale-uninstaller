# Stale Uninstaller – Plan 1: Program List with Last-Used Implementation Plan

**Goal:** A working `stale.exe` that lists every classic installed program with drive, size and a trustworthy "senast använd" date, in the layout-B window (filter panel, table, details), sorted oldest-first.

**Architecture:** Cargo workspace with two crates. `stale-core` holds all logic with no egui dependency: registry access behind a `RegistryReader` trait (fake in tests), usage providers (BAM, UserAssist, Prefetch, file times) behind a `UsageProvider` trait, pure matching rules, and a scan orchestrator that streams `ScanEvent`s over a channel. `stale-gui` is an eframe app that spawns the scan on a worker thread and renders rows from the events; its filter/sort/format logic lives in pure modules with unit tests.

**Tech Stack:** Rust stable (MSVC target), eframe/egui, winreg, windows (Win32 API), serde/serde_json, chrono, embed-manifest.

**Spec:** `docs/design/2026-10-17-stale-uninstaller-design.md`

This is plan 1 of 2 for version 1. Plan 2 (uninstall queue, leftover scan, quarantine, undo) is written after this plan ships, and builds on the `Program`, `ProgramUsage`, `RegistryReader` and `paths` interfaces defined here.

## Global Constraints

- Target: Windows 10/11 x64 only. Toolchain `stable-x86_64-pc-windows-msvc`.
- `stale-core` must not depend on `eframe`/`egui`. Everything Windows-specific in core goes behind a trait or a clearly named `win_` function so tests never touch the real registry, Prefetch folder or file system outside a temp dir.
- The exe requests elevation: manifest with `requestedExecutionLevel level="requireAdministrator"` and per-monitor DPI awareness.
- All user-visible strings are Swedish and live in `crates/stale-gui/src/strings.rs`. No Swedish literals elsewhere in the GUI crate.
- Persistent data under `%ProgramData%\Stale\` (settings in `settings.json`, logs in `logs\`).
- Hidden rows (no `DisplayName`, `SystemComponent=1`, or `ParentKeyName` set) are excluded unless the "show hidden" setting is on.
- "Senast använd" in the table = latest launch of a *main exe* (shortcut target → `DisplayIcon` → largest exe in install root) via BAM/Prefetch, or a UserAssist entry for the program; exes whose file name contains `update`, `updater`, `service`, `helper`, `crash`, `report`, `daemon`, `tray`, or that are registered as a service, never count.
- The UI never blocks: all scanning on worker threads; programs appear before usage, usage before sizes.
- Commit after every task. Commit messages end with the attribution lines in the session reminder.

## Review Focus

1. **Registry values of unexpected type** (e.g. `DisplayName` stored as `REG_BINARY`, `EstimatedSize` as `REG_SZ`, `InstallLocation` as `REG_EXPAND_SZ`): the entry is listed with that field missing, never a panic or a dropped list. Pinned in Task 3.
2. **Paths with quotes, env vars and trailing junk** (`"%ProgramFiles%\Foo\unins000.exe" /SILENT`, `C:\Foo\` with trailing backslash): drive derivation still yields `C`. Pinned in Task 3.
3. **A corrupt, truncated or uncompressed `.pf` file**: that file is skipped with a warning event, every other Prefetch file still contributes. Pinned in Task 6.
4. **`InstallLocation` pointing at a shared root** (`C:\Program Files\`, `C:\`, `%LOCALAPPDATA%`): must not claim every exe on the disk as that program's; such a location is treated as absent for matching. Pinned in Task 7.
5. **Thousands of rows and slow folder sizes**: the table is usable before sizes arrive; `ScanEvent::Programs` always precedes `Usage`, which precedes any `Sizes`. Pinned in Task 8.

---

### Task 0: Developer environment

Rust and the MSVC C++ toolchain are not installed on this machine (checked 2026-10-17: no `cargo`, no `VC\Tools\MSVC`, no Windows SDK libs). Both installs change the system and take several GB; **get the user's go-ahead before running them.**

**Files:** none.

- [ ] **Step 1: Install rustup (installs stable MSVC toolchain by default)**

Run (PowerShell):
```powershell
winget install --id Rustlang.Rustup -e --accept-package-agreements --accept-source-agreements
```
Then open a new shell. Expected: `cargo --version` prints `cargo 1.x`.

- [ ] **Step 2: Add the C++ workload to Visual Studio 2026 (elevated PowerShell)**

```powershell
& "C:\Program Files (x86)\Microsoft Visual Studio\Installer\setup.exe" modify --installPath "C:\Program Files\Microsoft Visual Studio\18\Professional" --add Microsoft.VisualStudio.Workload.NativeDesktop --includeRecommended --passive --norestart
```
Expected after completion: `Get-ChildItem "C:\Program Files\Microsoft Visual Studio\18\Professional\VC\Tools\MSVC"` lists a version folder, and `C:\Program Files (x86)\Windows Kits\10\Lib` exists.

- [ ] **Step 3: Verify linking works**

```powershell
cargo new --bin $env:TEMP\stale-linkcheck; cargo run --manifest-path $env:TEMP\stale-linkcheck\Cargo.toml
```
Expected: prints `Hello, world!`. Then delete the temp project.

---

### Task 1: Workspace scaffold

**Files:**
- Create: `Cargo.toml` (workspace), `rust-toolchain.toml`, `crates/stale-core/Cargo.toml`, `crates/stale-core/src/lib.rs`, `crates/stale-gui/Cargo.toml`, `crates/stale-gui/src/main.rs`
- Modify: `.gitignore` (add `target/`), `CLAUDE.md` (replace placeholder with real commands)

**Interfaces:**
- Produces: workspace members `stale-core` (lib) and `stale-gui` (bin named `stale`). Later tasks add dependencies with `cargo add -p <crate> <dep>`.

- [ ] **Step 1: Create workspace files**

`Cargo.toml`: `[workspace] members = ["crates/*"]`, `resolver = "2"`, `[workspace.package] edition = "2021"`, `[profile.release] opt-level = "s", lto = true, strip = true`.
`rust-toolchain.toml`: `[toolchain] channel = "stable"`, `targets = ["x86_64-pc-windows-msvc"]`.
`crates/stale-gui/Cargo.toml`: `[[bin]] name = "stale" path = "src/main.rs"`, dependency `stale-core = { path = "../stale-core" }`.
`lib.rs`: `pub fn version() -> &'static str { env!("CARGO_PKG_VERSION") }`. `main.rs`: prints `stale {version}`.

- [ ] **Step 2: Write a smoke test in `crates/stale-core/src/lib.rs`**

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn version_is_semver() { assert!(super::version().split('.').count() == 3); }
}
```

- [ ] **Step 3: Build and test**

Run: `cargo build && cargo test --workspace`
Expected: both succeed, `cargo run -p stale-gui` prints `stale 0.1.0`.

- [ ] **Step 4: Update CLAUDE.md**

Replace the placeholder "Status" section with: build (`cargo build`), run (`cargo run -p stale-gui`, needs elevation at runtime), test all (`cargo test --workspace`), single test (`cargo test -p stale-core <name>`), lint (`cargo clippy --workspace --all-targets`), and the two-crate architecture summary from this plan's header. Keep it under 40 lines.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "chore: cargo workspace with stale-core and stale-gui"
```

---

### Task 2: Core model and registry abstraction

**Files:**
- Create: `crates/stale-core/src/model.rs`, `crates/stale-core/src/registry.rs`, `crates/stale-core/src/registry/fake.rs`, `crates/stale-core/src/registry/win.rs`
- Modify: `crates/stale-core/src/lib.rs` (declare modules, re-export model)
- Add deps: `cargo add -p stale-core winreg chrono serde --features serde/derive`, `cargo add -p stale-core thiserror`

**Interfaces:**
- Produces (`model.rs`):
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Hive { LocalMachine, CurrentUser, Users }
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegKeyPath { pub hive: Hive, pub path: String }   // path uses backslashes, no leading slash
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProgramId(pub String);                               // "{hive:?}\{path}"
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope { Machine, CurrentUser, OtherUser }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub id: ProgramId,
    pub key: RegKeyPath,
    pub name: String,
    pub publisher: Option<String>,
    pub version: Option<String>,
    pub install_location: Option<PathBuf>,
    pub install_date: Option<chrono::NaiveDate>,
    pub uninstall_string: Option<String>,
    pub quiet_uninstall_string: Option<String>,
    pub display_icon: Option<PathBuf>,
    pub is_msi: bool,
    pub estimated_size_bytes: Option<u64>,
    pub drive: Option<char>,          // uppercase letter
    pub hidden: bool,
    pub scope: Scope,
}
```
- Produces (`registry.rs`):
```rust
#[derive(Clone, Debug, PartialEq)]
pub enum RegValue { Str(String), ExpandStr(String), MultiStr(Vec<String>), Dword(u32), Qword(u64), Binary(Vec<u8>), Other }
pub trait RegistryReader: Send + Sync {
    fn subkeys(&self, key: &RegKeyPath) -> Result<Vec<String>, RegError>;
    fn values(&self, key: &RegKeyPath) -> Result<Vec<(String, RegValue)>, RegError>;
    fn key_exists(&self, key: &RegKeyPath) -> bool;
}
#[derive(Debug, thiserror::Error)] pub enum RegError { #[error("key not found")] NotFound, #[error("access denied")] AccessDenied, #[error("{0}")] Other(String) }
pub struct FakeRegistry;   // in registry/fake.rs, always compiled (used by other crates' tests later)
impl FakeRegistry { pub fn new() -> Self; pub fn add_key(&mut self, key: RegKeyPath); pub fn set(&mut self, key: RegKeyPath, name: &str, value: RegValue); }
pub struct WinRegistry;    // in registry/win.rs, implements RegistryReader via winreg, read-only, with KEY_WOW64_64KEY
pub fn key(hive: Hive, path: &str) -> RegKeyPath;   // convenience constructor
```

- [ ] **Step 1: Write failing tests in `registry/fake.rs`**

```rust
#[test] fn fake_lists_subkeys_and_values() {
    let mut r = FakeRegistry::new();
    r.set(key(Hive::LocalMachine, r"SOFTWARE\A\B"), "DisplayName", RegValue::Str("B".into()));
    assert_eq!(r.subkeys(&key(Hive::LocalMachine, r"SOFTWARE\A")).unwrap(), vec!["B".to_string()]);
    assert!(r.key_exists(&key(Hive::LocalMachine, r"SOFTWARE\A")));   // parents implied
    assert!(matches!(r.subkeys(&key(Hive::LocalMachine, r"SOFTWARE\Nope")), Err(RegError::NotFound)));
}
#[test] fn fake_paths_are_case_insensitive() { /* set with "software\a", read with "SOFTWARE\A" */ }
```

- [ ] **Step 2: Run, expect compile failure** — `cargo test -p stale-core fake_`

- [ ] **Step 3: Implement `model.rs`, `registry.rs`, `FakeRegistry` (BTreeMap keyed by lowercased `"{hive}\{path}"`), `WinRegistry`**

`WinRegistry::values` maps winreg types: `REG_SZ→Str`, `REG_EXPAND_SZ→ExpandStr`, `REG_MULTI_SZ→MultiStr`, `REG_DWORD→Dword`, `REG_QWORD→Qword`, `REG_BINARY→Binary`, else `Other`. Map `ERROR_FILE_NOT_FOUND→NotFound`, `ERROR_ACCESS_DENIED→AccessDenied`.

- [ ] **Step 4: Run tests, expect pass** — `cargo test -p stale-core`

- [ ] **Step 5: Add an ignored live test** `#[test] #[ignore] fn win_registry_reads_uninstall_root()` asserting `WinRegistry.subkeys(HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall)` is non-empty. Run `cargo test -p stale-core -- --ignored win_registry` once and confirm pass.

- [ ] **Step 6: Commit** — `git commit -m "feat(core): program model and registry abstraction"`

---

### Task 3: Installed-program enumeration

**Files:**
- Create: `crates/stale-core/src/programs.rs`, `crates/stale-core/src/pathutil.rs`
- Modify: `crates/stale-core/src/lib.rs`

**Interfaces:**
- Consumes: `RegistryReader`, `Program`, `RegKeyPath`, `Hive`, `Scope`.
- Produces:
```rust
pub const UNINSTALL_ROOTS: [(Hive, &str, Scope); 3];   // HKLM\SOFTWARE\...\Uninstall, HKLM\SOFTWARE\WOW6432Node\...\Uninstall (both Machine), HKCU\Software\...\Uninstall (CurrentUser)
pub fn enumerate_programs(reg: &dyn RegistryReader) -> Vec<Program>;           // never fails; unreadable roots are skipped
pub fn program_from_values(key: RegKeyPath, scope: Scope, values: &[(String, RegValue)]) -> Option<Program>;  // None only when there is no usable DisplayName AND no UninstallString
// pathutil.rs
pub fn expand_env(s: &str) -> String;                   // %VAR% expansion, unknown vars left as-is
pub fn first_path_in_command(cmd: &str) -> Option<PathBuf>;   // strips quotes, takes the executable before arguments, expands env vars, handles `MsiExec.exe /X{...}` → Some(msiexec path)
pub fn drive_letter(p: &Path) -> Option<char>;          // uppercase; None for UNC or relative
```

- [ ] **Step 1: Write failing tests in `programs.rs`**

```rust
fn vals(pairs: &[(&str, RegValue)]) -> Vec<(String, RegValue)> { ... }
#[test] fn parses_full_entry() {
    let p = program_from_values(key(Hive::LocalMachine, r"...\Uninstall\Unity Hub"), Scope::Machine, &vals(&[
        ("DisplayName", Str("Unity Hub")), ("Publisher", Str("Unity Technologies")), ("DisplayVersion", Str("3.8.0")),
        ("InstallLocation", Str(r"D:\Program Files\Unity Hub\")), ("InstallDate", Str("20240120")),
        ("EstimatedSize", Dword(421888)), ("UninstallString", Str(r#""D:\Program Files\Unity Hub\Uninstall Unity Hub.exe""#)),
    ])).unwrap();
    assert_eq!(p.drive, Some('D')); assert_eq!(p.estimated_size_bytes, Some(421888 * 1024));
    assert_eq!(p.install_date, Some(NaiveDate::from_ymd_opt(2024, 1, 20).unwrap())); assert!(!p.hidden); assert!(!p.is_msi);
}
#[test] fn hidden_when_system_component_or_parent_key_or_no_name() { /* three cases, all hidden == true; the no-name case keeps name = key leaf */ }
#[test] fn msi_flag_and_drive_from_display_icon_then_uninstall_string() {
    // WindowsInstaller=Dword(1) → is_msi; no InstallLocation, DisplayIcon=Str(r"E:\Apps\x\x.exe,0") → drive E
    // no InstallLocation, no DisplayIcon, UninstallString=Str(r#""%ProgramFiles%\Foo\unins000.exe" /SILENT"#) → drive C (env expanded)
}
#[test] fn wrong_value_types_are_ignored_not_fatal() {
    // DisplayName=Binary(..) with UninstallString present → Some(program) with hidden==true and name==key leaf
    // EstimatedSize=Str("123") → estimated_size_bytes == None; InstallLocation=ExpandStr(r"%ProgramFiles%\Bar") → install_location == Some(C:\Program Files\Bar)
}
#[test] fn enumerate_reads_all_three_roots_and_skips_missing_root() { /* FakeRegistry with 2 roots populated, 1 absent → 2 programs, scopes correct, ids unique */ }
#[test] fn install_date_bad_format_is_none() { /* "2024-01-20" and "" → None */ }
```
And in `pathutil.rs`: `first_path_in_command` for the quoted/env/trailing-args cases above and `drive_letter(r"\\server\share") == None`.

- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core programs`

- [ ] **Step 3: Implement**

`drive` derivation order: `install_location` → `display_icon` (strip trailing `,<n>`) → `first_path_in_command(uninstall_string)`. `InstallLocation` is normalized: quotes stripped, env expanded, trailing separators trimmed, empty → `None`. `EstimatedSize` is in KB.

- [ ] **Step 4: Run tests, expect pass** — `cargo test -p stale-core`

- [ ] **Step 5: Commit** — `git commit -m "feat(core): enumerate installed programs from Uninstall keys"`

---

### Task 4: Usage model and UserAssist provider

**Files:**
- Create: `crates/stale-core/src/usage/mod.rs`, `crates/stale-core/src/usage/userassist.rs`, `crates/stale-core/src/usage/filetime.rs`, `crates/stale-core/src/knownfolders.rs`
- Add deps: `cargo add -p stale-core windows --features Win32_Foundation,Win32_UI_Shell,Win32_System_Com,Win32_Storage_FileSystem`

**Interfaces:**
- Produces (`usage/mod.rs`):
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UsageSource { Prefetch, Bam, UserAssist, FileTime }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageHit { pub exe_path: PathBuf, pub last_run: DateTime<Utc>, pub source: UsageSource, pub run_count: Option<u32> }
pub trait UsageProvider: Send { fn name(&self) -> &'static str; fn collect(&self) -> Result<Vec<UsageHit>, String>; }
```
- Produces (`usage/filetime.rs`): `pub fn filetime_to_utc(ft: u64) -> Option<DateTime<Utc>>` — `0`, or anything before 1990-01-01 or after now+1 day → `None`.
- Produces (`usage/userassist.rs`):
```rust
pub const USERASSIST_EXE_GUID: &str = "{CEBFF5CD-ACE2-4F4F-9178-9926F41749EA}";
pub const USERASSIST_LNK_GUID: &str = "{F4E57C4B-2036-45F0-A9AB-443BCFE33D9F}";
pub fn rot13(s: &str) -> String;
pub fn parse_userassist_value(name_rot13: &str, data: &[u8], resolve: &dyn Fn(&str) -> Option<PathBuf>) -> Option<UsageHit>;
   // data must be >= 68 bytes; run_count = u32 LE at 4; last_run = FILETIME LE at 60; names starting with "{GUID}\" are resolved via `resolve`, names starting with "UEME_" are skipped
pub struct UserAssistProvider { pub reg: Arc<dyn RegistryReader> }   // reads both GUID Count keys under HKCU and under every HKU\<SID> subkey that is not "_Classes"/".DEFAULT"; resolves known-folder GUIDs via knownfolders::resolve
impl UsageProvider for UserAssistProvider { ... }
```
- Produces (`knownfolders.rs`): `pub fn resolve(guid: &str) -> Option<PathBuf>` via `SHGetKnownFolderPath` (builder implements; returns `None` for unknown GUIDs rather than erroring).

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn rot13_roundtrips() { assert_eq!(rot13("Zvpebfbsg"), "Microsoft"); assert_eq!(rot13("{PROSS5PQ-NPR2}"), "{CEBFF5CD-ACE2}"); }
#[test] fn parses_value_with_filetime_at_60() {
    let mut data = vec![0u8; 72]; data[4..8].copy_from_slice(&7u32.to_le_bytes());
    data[60..68].copy_from_slice(&133_500_000_000_000_000u64.to_le_bytes());   // 2024-01-17ish
    let hit = parse_userassist_value(&rot13(r"C:\Apps\foo.exe"), &data, &|_| None).unwrap();
    assert_eq!(hit.exe_path, PathBuf::from(r"C:\Apps\foo.exe")); assert_eq!(hit.run_count, Some(7)); assert_eq!(hit.source, UsageSource::UserAssist);
}
#[test] fn resolves_known_folder_prefix() { /* "{6D80...}\Foo\foo.exe" with resolve returning C:\Program Files → C:\Program Files\Foo\foo.exe */ }
#[test] fn skips_ueme_and_short_data_and_zero_filetime() { /* three Nones */ }
#[test] fn filetime_zero_and_absurd_are_none() { assert!(filetime_to_utc(0).is_none()); assert!(filetime_to_utc(u64::MAX).is_none()); }
```

- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core usage`

- [ ] **Step 3: Implement** the pure functions, `knownfolders::resolve`, and `UserAssistProvider` (HKU enumeration through `RegistryReader::subkeys(key(Hive::Users, ""))`).

- [ ] **Step 4: Run tests, expect pass**; add `#[ignore]` live test `userassist_live_returns_hits` and run it once with `-- --ignored`.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): usage model and UserAssist provider"`

---

### Task 5: BAM provider

**Files:**
- Create: `crates/stale-core/src/usage/bam.rs`, `crates/stale-core/src/devicepath.rs`

**Interfaces:**
- Consumes: `RegistryReader`, `UsageHit`, `filetime_to_utc`.
- Produces:
```rust
// devicepath.rs
pub trait DeviceMap: Send + Sync { fn drive_for_device(&self, device: &str) -> Option<char>; }   // "\Device\HarddiskVolume3" → 'C'
pub struct WinDeviceMap;   // built once via QueryDosDeviceW over A: .. Z:
pub struct FakeDeviceMap(pub HashMap<String, char>);
pub fn device_to_drive_path(p: &str, map: &dyn DeviceMap) -> Option<PathBuf>;   // "\Device\HarddiskVolume3\Program Files\x.exe" → C:\Program Files\x.exe; unknown device → None
// bam.rs
pub const BAM_ROOTS: [&str; 2] = [r"SYSTEM\CurrentControlSet\Services\bam\State\UserSettings", r"SYSTEM\CurrentControlSet\Services\bam\UserSettings"];   // 1809+, then 1709/1803
pub fn parse_bam_value(name: &str, data: &[u8], map: &dyn DeviceMap) -> Option<UsageHit>;   // skip "Version"/"SequenceNumber"; data >= 8; FILETIME LE at 0; source Bam
pub struct BamProvider { pub reg: Arc<dyn RegistryReader>, pub map: Arc<dyn DeviceMap> }
```

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn device_path_maps_to_drive() { /* FakeDeviceMap {"\Device\HarddiskVolume3": 'C'}; also unknown device → None; also a name without \Device prefix (already C:\...) passes through */ }
#[test] fn parse_bam_value_reads_filetime_and_skips_metadata() { /* "Version" → None; short data → None; good → Some with source Bam */ }
#[test] fn bam_provider_reads_first_existing_root_for_every_sid() { /* FakeRegistry with State\UserSettings\S-1-5-21-x containing 2 values + 1 "Version" → 2 hits */ }
```

- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core bam`

- [ ] **Step 3: Implement**; `WinDeviceMap::new()` calls `QueryDosDeviceW` for each letter and stores the first line of the result.

- [ ] **Step 4: Run tests, expect pass**; live ignored test `bam_live_returns_hits`.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): BAM usage provider"`

---

### Task 6: Prefetch provider

**Files:**
- Create: `crates/stale-core/src/usage/prefetch.rs`, `crates/stale-core/src/usage/prefetch/decompress.rs`, `crates/stale-core/tests/fixtures/prefetch/` (one real `.pf` copied from `C:\Windows\Prefetch`, e.g. the `NOTEPAD.EXE-*.pf`; it is ~50-200 KB, commit it)

**Interfaces:**
- Produces:
```rust
// decompress.rs — raw FFI to ntdll, no windows-crate feature needed
pub fn decompress_mam(data: &[u8]) -> Result<Vec<u8>, String>;   // expects "MAM\x04" magic, u32 LE uncompressed size at 4, XPRESS Huffman payload at 8; uses RtlGetCompressionWorkSpaceSize + RtlDecompressBufferEx with COMPRESSION_FORMAT_XPRESS_HUFF (4); data without the magic is returned unchanged
// prefetch.rs
#[derive(Debug, PartialEq)]
pub struct PrefetchFile { pub version: u32, pub exe_name: String, pub last_runs: Vec<DateTime<Utc>>, pub run_count: u32, pub referenced_files: Vec<String> }
pub fn parse_prefetch(bytes: &[u8]) -> Result<PrefetchFile, String>;
   // header: version u32 LE at 0 (17/23/26/30 supported), b"SCCA" at 4, exe name UTF-16 at 16 (max 29 chars, NUL-terminated)
   // file info: filename-strings offset u32 at 100, size u32 at 104 (UTF-16, NUL-separated)
   // last run FILETIMEs at 128: count 1 for v17/23, 8 for v26/30 (drop zero entries)
   // run count u32 at: v17 152, v23 152, v26 208, v30 200 if bytes[200..204] != 0 else 208
pub struct PrefetchProvider { pub dir: PathBuf }   // default C:\Windows\Prefetch
impl UsageProvider for PrefetchProvider { ... }    // one UsageHit per (referenced_file ending in exe_name, last_run max); exe_path keeps the "\VOLUME{...}\..." form — matching handles it in Task 7; files that fail to parse are skipped and counted; collect() never fails because of a single file
pub fn strip_volume_prefix(p: &str) -> Option<String>;   // "\VOLUME{guid}\PROGRAM FILES\X.EXE" → "\PROGRAM FILES\X.EXE"
```

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn decompress_rejects_truncated_header() { assert!(decompress_mam(b"MAM\x04\x10").is_err()); }
#[test] fn parse_rejects_bad_signature() { assert!(parse_prefetch(&[0u8; 300]).is_err()); }
#[test] fn parse_rejects_truncated_file() { let f = std::fs::read(fixture()).unwrap(); let raw = decompress_mam(&f).unwrap(); assert!(parse_prefetch(&raw[..150]).is_err()); }
#[test] fn parses_real_fixture() {
    let raw = decompress_mam(&std::fs::read(fixture()).unwrap()).unwrap();
    let pf = parse_prefetch(&raw).unwrap();
    assert!(pf.exe_name.ends_with(".EXE")); assert!(pf.version == 26 || pf.version == 30);
    assert!(!pf.last_runs.is_empty() && pf.last_runs[0].year() >= 2015); assert!(pf.referenced_files.iter().any(|f| f.ends_with(&pf.exe_name)));
}
#[test] fn provider_skips_corrupt_file_but_keeps_good_ones() { /* temp dir with fixture + a 40-byte garbage .pf → collect() Ok with >=1 hit */ }
#[test] fn strip_volume_prefix_works() { assert_eq!(strip_volume_prefix(r"\VOLUME{01d9-ab}\PROGRAM FILES\X.EXE").as_deref(), Some(r"\PROGRAM FILES\X.EXE")); }
```

- [ ] **Step 2: Copy a fixture**: `Copy-Item C:\Windows\Prefetch\NOTEPAD.EXE-*.pf crates\stale-core\tests\fixtures\prefetch\` (elevated). If none exists, pick any `*.pf`. Run tests, expect failure — `cargo test -p stale-core prefetch`

- [ ] **Step 3: Implement** decompression (FFI signatures: `RtlGetCompressionWorkSpaceSize(u16, *mut u32, *mut u32) -> i32`, `RtlDecompressBufferEx(u16, *mut u8, u32, *const u8, u32, *mut u32, *mut c_void) -> i32`; NTSTATUS 0 = success) and the parser.

- [ ] **Step 4: Run tests, expect pass** — `cargo test -p stale-core prefetch`

- [ ] **Step 5: Commit** — `git commit -m "feat(core): Prefetch parser and provider"`

---

### Task 7: Main-exe detection, shortcuts, services and usage matching

**Files:**
- Create: `crates/stale-core/src/usage/matching.rs`, `crates/stale-core/src/shortcuts.rs`, `crates/stale-core/src/services.rs`
- Modify: `crates/stale-core/src/pathutil.rs`

**Interfaces:**
- Consumes: `Program`, `UsageHit`, `RegistryReader`, `strip_volume_prefix`.
- Produces:
```rust
// pathutil.rs
pub fn norm(p: &Path) -> String;                               // uppercase, backslashes, no trailing separator
pub fn is_shared_root(p: &Path) -> bool;                      // true for drive roots and for Program Files, Program Files (x86), ProgramData, %LOCALAPPDATA%, %LOCALAPPDATA%\Programs, %APPDATA%, Windows, Users\<x> themselves (not their subfolders)
pub fn exe_under(exe: &Path, install: &Path) -> bool;         // case-insensitive; if `exe` starts with "\" (volume stripped) compare against `install` minus its drive
// shortcuts.rs
pub fn start_menu_dirs() -> Vec<PathBuf>;                     // %ProgramData%\Microsoft\Windows\Start Menu\Programs and %APPDATA%\...\Start Menu\Programs
pub fn resolve_lnk(lnk: &Path) -> Option<PathBuf>;            // IShellLinkW + IPersistFile; None unless target ends with .exe
pub fn shortcut_targets(dirs: &[PathBuf]) -> Vec<PathBuf>;    // recursive *.lnk → resolved exe targets, deduped
// services.rs
pub fn service_exes(reg: &dyn RegistryReader) -> HashSet<String>;   // norm() of first_path_in_command(ImagePath) for every HKLM\SYSTEM\CurrentControlSet\Services\*; ImagePath env-expanded, "\??\" and "\SystemRoot" prefixes handled
// matching.rs
pub const EXCLUDED_NAME_PARTS: [&str; 8] = ["update", "updater", "service", "helper", "crash", "report", "daemon", "tray"];
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExeUsage { pub exe_path: PathBuf, pub last_run: DateTime<Utc>, pub source: UsageSource, pub run_count: Option<u32>, pub is_main: bool, pub excluded: Option<ExcludeReason> }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] pub enum ExcludeReason { NamePattern, Service, NotMain }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LastUsed { pub when: Option<DateTime<Utc>>, pub source: Option<UsageSource>, pub confident: bool }   // confident=false only for FileTime
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProgramUsage { pub last_used: LastUsed, pub exes: Vec<ExeUsage> }   // exes sorted by last_run desc
pub struct MatchInput<'a> { pub programs: &'a [Program], pub hits: &'a [UsageHit], pub shortcut_targets: &'a [PathBuf], pub root_exes: &'a HashMap<ProgramId, Vec<(PathBuf, u64)>>, pub service_exes: &'a HashSet<String> }
pub fn main_exes(p: &Program, shortcut_targets: &[PathBuf], root_exes: &[(PathBuf, u64)]) -> Vec<PathBuf>;
   // 1) shortcut targets under install_location; else 2) display_icon if .exe; else 3) largest of root_exes. Empty if install_location is None or is_shared_root.
pub fn exclude_reason(exe: &Path, is_main: bool, service_exes: &HashSet<String>) -> Option<ExcludeReason>;   // NamePattern checked on file stem lowercased, then Service, then NotMain
pub fn match_usage(input: &MatchInput) -> HashMap<ProgramId, ProgramUsage>;
   // hit belongs to program if exe_under(hit.exe_path, install_location) (install_location not shared root), or, when install_location is None, if file name equals display_icon's / uninstall_string's exe file name.
   // UserAssist hits are never excluded (they are user launches). last_used.when = max over non-excluded hits; if the only hits are FileTime → confident=false.
   // Programs with no hits at all get ProgramUsage { last_used: {None,None,true}, exes: [] }.
```

- [ ] **Step 1: Write failing tests in `matching.rs` and `pathutil.rs`**

```rust
#[test] fn shared_roots_are_detected() { assert!(is_shared_root(Path::new(r"C:\Program Files"))); assert!(is_shared_root(Path::new(r"C:\"))); assert!(!is_shared_root(Path::new(r"C:\Program Files\Foo"))); }
#[test] fn exe_under_handles_volume_stripped_paths() { assert!(exe_under(Path::new(r"\PROGRAM FILES\FOO\FOO.EXE"), Path::new(r"C:\Program Files\Foo"))); assert!(!exe_under(Path::new(r"C:\Program Files\Foobar\x.exe"), Path::new(r"C:\Program Files\Foo"))); }
#[test] fn main_exe_prefers_shortcut_then_icon_then_largest() { /* three programs, three outcomes */ }
#[test] fn main_exe_empty_for_shared_root_install_location() { /* install_location = C:\Program Files → [] */ }
#[test] fn updater_hit_does_not_set_last_used_but_appears_in_exes() {
    // program Foo at C:\Apps\Foo, main exe foo.exe; hits: FooUpdater.exe yesterday (Bam), foo.exe 200 days ago (Bam)
    // → last_used.when == 200 days ago, exes.len()==2, exes[0].excluded == Some(NamePattern)
}
#[test] fn service_exe_is_excluded() { /* service_exes contains norm(C:\Apps\Foo\foo.exe) → excluded Some(Service), last_used None */ }
#[test] fn userassist_hit_counts_even_for_non_main_exe() { /* UserAssist hit on C:\Apps\Foo\tools\other.exe → last_used set */ }
#[test] fn filetime_only_gives_unconfident_date() { /* source FileTime → confident == false */ }
#[test] fn shared_root_install_location_claims_nothing() { /* program with install_location C:\Program Files, hit at C:\Program Files\Other\x.exe → no exes */ }
#[test] fn no_install_location_matches_on_display_icon_file_name() { /* display_icon C:\x\bar.exe, hit \VOLUME{..}\X\BAR.EXE → matched */ }
```

- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core matching`

- [ ] **Step 3: Implement** pure functions first, then `shortcuts.rs` (COM init with `CoInitializeEx(COINIT_APARTMENTTHREADED)` on the calling thread; builder's choice to cache targets) and `services.rs`.

- [ ] **Step 4: Run tests, expect pass**; ignored live test `shortcuts_live_resolves_at_least_one_exe`.

- [ ] **Step 5: Commit** — `git commit -m "feat(core): main-exe detection and usage matching"`

---

### Task 8: Sizes, settings, paths and the scan orchestrator

**Files:**
- Create: `crates/stale-core/src/paths.rs`, `crates/stale-core/src/settings.rs`, `crates/stale-core/src/size.rs`, `crates/stale-core/src/scan.rs`, `crates/stale-core/examples/dump.rs`
- Add deps: `cargo add -p stale-core serde_json log`, `cargo add -p stale-core --dev tempfile`

**Interfaces:**
- Produces:
```rust
// paths.rs
pub fn data_dir() -> PathBuf;            // %ProgramData%\Stale (created on first call)
pub fn logs_dir() -> PathBuf;            // data_dir()\logs
pub fn user_profiles_root() -> PathBuf;  // %SystemDrive%\Users
// settings.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[serde(default)]
pub struct Settings { pub show_hidden: bool, pub quarantine_days: u32 }   // defaults false, 30
impl Settings { pub fn load_from(path: &Path) -> Settings; pub fn save_to(&self, path: &Path) -> std::io::Result<()>; pub fn path() -> PathBuf /* data_dir()\settings.json */ }
// size.rs
pub fn dir_size(root: &Path) -> u64;                       // follows no symlinks/junctions; unreadable entries skipped
pub fn list_root_exes(dir: &Path) -> Vec<(PathBuf, u64)>;  // non-recursive *.exe with sizes
// scan.rs
#[derive(Debug)]
pub enum ScanEvent { Programs(Vec<Program>), Usage(HashMap<ProgramId, ProgramUsage>), Sizes(HashMap<ProgramId, u64>), Warning(String), Done }
pub struct ScanSources { pub reg: Arc<dyn RegistryReader>, pub providers: Vec<Box<dyn UsageProvider>>, pub shortcut_targets: Vec<PathBuf>, pub list_root_exes: Box<dyn Fn(&Path) -> Vec<(PathBuf, u64)> + Send>, pub dir_size: Box<dyn Fn(&Path) -> u64 + Send> }
impl ScanSources { pub fn live() -> ScanSources; }         // WinRegistry, [Bam, UserAssist, Prefetch, FileTime providers], real shortcuts and fs
pub fn run_scan(src: ScanSources, tx: Sender<ScanEvent>);  // blocking; emits Programs → Usage → Sizes (in batches of ≤50 programs) → Done; a failing provider emits Warning and continues
```
- The `FileTime` provider (`usage/filetime.rs`, add here): `pub struct FileTimeProvider { pub exes: Vec<PathBuf> }` built from every program's main exes; hit `last_run` = max(modified, accessed) with source `FileTime`.

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn settings_roundtrip_and_defaults() { /* tempfile: missing file → defaults; save then load equals */ }
#[test] fn settings_ignores_unknown_and_missing_fields() { /* write {"show_hidden":true,"future":1} → show_hidden true, quarantine_days 30 */ }
#[test] fn dir_size_sums_files_recursively() { /* tempdir with 2 files of 10 and 20 bytes in nested dirs → 30 */ }
#[test] fn scan_emits_programs_then_usage_then_sizes_then_done() {
    // FakeRegistry with 2 programs, one provider returning 1 hit, closures returning fixed sizes
    // collect events; assert order of discriminants == [Programs, Usage, Sizes, Done] and Sizes has 2 entries
}
#[test] fn scan_continues_when_provider_fails() { /* provider Err("boom") → a Warning event containing "boom", still Done */ }
```

- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core scan`

- [ ] **Step 3: Implement**, plus `examples/dump.rs`: runs `run_scan(ScanSources::live())` on the current thread, prints one line per visible program: `drive | name | last used (source) | size`, sorted oldest-first. This is the manual verification tool for the whole core.

- [ ] **Step 4: Run tests, expect pass**; then run `cargo run -p stale-core --example dump` from an **elevated** shell and confirm your own machine's programs show plausible dates (e.g. an app you opened today shows "today", its updater does not drive the date).

- [ ] **Step 5: Commit** — `git commit -m "feat(core): scan orchestrator, settings and dump example"`

---

### Task 9: GUI shell, worker thread, manifest

**Files:**
- Create: `crates/stale-gui/build.rs`, `crates/stale-gui/stale.exe.manifest`, `crates/stale-gui/src/app.rs`, `crates/stale-gui/src/worker.rs`, `crates/stale-gui/src/strings.rs`, `crates/stale-gui/src/logging.rs`
- Modify: `crates/stale-gui/src/main.rs`, `crates/stale-gui/Cargo.toml`
- Add deps: `cargo add -p stale-gui eframe log simplelog chrono`, `cargo add -p stale-gui --build embed-manifest`

**Interfaces:**
- Consumes: `run_scan`, `ScanSources::live`, `ScanEvent`, `Settings`, `paths::logs_dir`.
- Produces:
```rust
// worker.rs
pub fn spawn_scan() -> std::sync::mpsc::Receiver<ScanEvent>;   // thread named "stale-scan"
// app.rs
pub struct Row { pub program: Program, pub usage: Option<ProgramUsage>, pub size: Option<u64> }
pub struct StaleApp { pub rows: Vec<Row>, pub settings: Settings, pub scanning: bool, pub warnings: Vec<String>, rx: Option<Receiver<ScanEvent>>, /* filters/sort/selection added in Task 10 */ }
impl StaleApp { pub fn new(cc: &eframe::CreationContext) -> Self; pub fn apply_event(&mut self, ev: ScanEvent); }   // apply_event is pure state mutation, testable
impl eframe::App for StaleApp { fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame); }   // drains rx with try_recv each frame; calls ctx.request_repaint() while scanning
// strings.rs: pub const APP_TITLE: &str = "Stale Uninstaller"; pub const LOADING: &str = "Läser installerade program…"; (all other UI text added here as needed)
```
- `main.rs`: `#![windows_subsystem = "windows"]`, init logging to `logs_dir()\stale.log` (simplelog `WriteLogger`, level Info), `eframe::run_native` with window title `APP_TITLE`, initial size 1100×700.
- `build.rs`: `embed_manifest::embed_manifest(new_manifest("Stale").requested_execution_level(RequireAdministrator).dpi_awareness(PerMonitorV2))`.

- [ ] **Step 1: Write failing tests in `app.rs`**

```rust
#[test] fn apply_programs_creates_rows_then_usage_and_sizes_attach_by_id() { /* 2 programs; Usage map for one; Sizes for the other; assert rows[i].usage / .size; Done sets scanning=false */ }
#[test] fn hidden_rows_dropped_unless_setting() { /* one hidden program; settings.show_hidden=false → 1 row; true → 2 rows (re-apply) */ }
```

- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-gui`

- [ ] **Step 3: Implement**; `update()` draws a top panel with `APP_TITLE`, and a central panel showing `LOADING` with a spinner while `rows` is empty and `scanning`, else a plain `egui::Grid`/`ScrollArea` with name and drive per row (the real table comes in Task 10).

- [ ] **Step 4: Run tests, expect pass**; `cargo build -p stale-gui --release`, run `target\release\stale.exe`: UAC prompt appears, window opens, list fills within a few seconds, `%ProgramData%\Stale\logs\stale.log` exists.

- [ ] **Step 5: Commit** — `git commit -m "feat(gui): eframe shell with background scan and admin manifest"`

---

### Task 10: Filter panel, table, sorting, search, formatting

**Files:**
- Create: `crates/stale-gui/src/filter.rs`, `crates/stale-gui/src/format.rs`, `crates/stale-gui/src/ui/mod.rs`, `crates/stale-gui/src/ui/toolbar.rs`, `crates/stale-gui/src/ui/filter_panel.rs`, `crates/stale-gui/src/ui/table.rs`
- Modify: `crates/stale-gui/src/app.rs`, `crates/stale-gui/src/strings.rs`
- Add deps: `cargo add -p stale-gui egui_extras --features datepicker` (for `TableBuilder`; drop the feature if unneeded)

**Interfaces:**
- Produces (`filter.rs`):
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum UsageBucket { All, OverSixMonths, OverOneYear, NeverSeen }   // thresholds: 183 and 365 days; Over* buckets include never-seen rows (my call: they are candidates too)
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum SizeBucket { All, OverOneGb }
#[derive(Clone, Debug, PartialEq)] pub struct Filters { pub usage: UsageBucket, pub drive: Option<char>, pub size: SizeBucket, pub search: String }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum SortKey { Name, Publisher, Drive, Size, Installed, LastUsed }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct Sort { pub key: SortKey, pub ascending: bool }   // default LastUsed ascending (oldest first, never-seen first)
pub fn row_matches(row: &Row, f: &Filters, now: DateTime<Utc>) -> bool;      // search: case-insensitive substring on name or publisher
pub fn visible_rows(rows: &[Row], f: &Filters, s: Sort, now: DateTime<Utc>) -> Vec<usize>;   // filtered indices in sort order; stable, ties by name
pub fn bucket_counts(rows: &[Row], now: DateTime<Utc>) -> BucketCounts;      // struct with all, over_six_months, over_one_year, never_seen, per_drive: BTreeMap<char, usize>, over_one_gb
pub fn is_stale(last_used: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool;   // None or older than 183 days → warning colour
```
- Produces (`format.rs`): `pub fn relative_date(when: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String` → `"Aldrig sedd"`, `"Idag"`, `"Igår"`, `"N dagar sedan"` (<30), `"N mån sedan"` (<36 months, N = days/30), `"N år sedan"`; `pub fn human_size(bytes: Option<u64>) -> String` → `"–"`, `"5 MB"`, `"1,2 GB"` (one decimal for GB, none below, Swedish comma); `pub fn ymd(d: Option<NaiveDate>) -> String` → `"2024-01-20"` or `"–"`.
- `app.rs` gains `filters: Filters`, `sort: Sort`, `selected: Option<ProgramId>`, `checked: HashSet<ProgramId>`.
- UI (`ui/*`): toolbar buttons Avinstallera / Tvingad borttagning / Sök rester / Uppdatera (all but Uppdatera disabled with tooltip "Kommer i nästa steg"; Uppdatera restarts the scan) + search box right-aligned; left `SidePanel` with the three groups and counts (selected row highlighted); central `TableBuilder` with columns ☐ · Namn · Utgivare · Disk · Storlek · Installerad · Senast använd, clickable headers toggling sort, stale rows in `egui::Color32::from_rgb(178, 58, 0)`, drive drawn as a small rounded frame; row click sets `selected`.

- [ ] **Step 1: Write failing tests in `filter.rs` and `format.rs`**

```rust
#[test] fn relative_dates() { /* None→"Aldrig sedd"; now→"Idag"; -1d→"Igår"; -5d→"5 dagar sedan"; -420d→"14 mån sedan"; -1500d→"4 år sedan" */ }
#[test] fn sizes() { assert_eq!(human_size(Some(58*1024*1024)), "58 MB"); assert_eq!(human_size(Some(1_288_490_189)), "1,2 GB"); assert_eq!(human_size(None), "–"); }
#[test] fn default_sort_puts_never_seen_first_then_oldest() { /* rows: today, 400d, None → order [2,1,0] */ }
#[test] fn usage_buckets_include_never_seen() { /* OverSixMonths matches 200d and None, not 10d; NeverSeen matches only None */ }
#[test] fn drive_and_size_and_search_filters_combine_with_and() { /* one row passes all, others fail one each */ }
#[test] fn bucket_counts_match_filters() { /* counts equal number of rows where row_matches with that single bucket */ }
```

- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-gui`

- [ ] **Step 3: Implement** pure modules, then the UI modules.

- [ ] **Step 4: Run tests, expect pass**; run the release exe elevated and check: default order shows never-seen then oldest; clicking "Över 1 år" filters; sorting by Disk groups drives; search narrows live; selecting a row highlights it.

- [ ] **Step 5: Commit** — `git commit -m "feat(gui): filter panel, program table, sorting and search"`

---

### Task 11: Details panel and settings toggle

**Files:**
- Create: `crates/stale-gui/src/ui/details.rs`
- Modify: `crates/stale-gui/src/ui/toolbar.rs`, `crates/stale-gui/src/app.rs`, `crates/stale-gui/src/strings.rs`

**Interfaces:**
- Consumes: `ProgramUsage`, `ExeUsage`, `ExcludeReason`, `Settings`.
- Produces: `pub fn details_panel(ui: &mut egui::Ui, row: &Row)` in a bottom `TopBottomPanel` (height ~160, resizable) showing, for the selected row: Plats, Avinstallerare (uninstall_string), Registernyckel (`HKLM\...` with hive abbreviated), Senast använd as `"{ymd} ({source})"` plus `" – osäker (filtid)"` when `confident == false`, and a table of `exes` with columns Fil · Senast körd · Källa · Räknas (✓, or the Swedish reason: "uppdaterare/tjänst-namn", "tjänst", "inte huvudprogram").
- Toolbar gains a `⚙` menu with checkbox "Visa dolda poster" bound to `settings.show_hidden`; toggling saves settings via `Settings::save_to(Settings::path())` and re-applies row visibility (keep the last `Vec<Program>` in app state so no rescan is needed).
- `pub fn source_label(s: UsageSource) -> &'static str` in `format.rs`: `"Prefetch"`, `"BAM"`, `"UserAssist"`, `"filtid"`.

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn exclude_reason_labels_are_swedish() { /* each ExcludeReason maps to a non-empty &str from strings.rs */ }
#[test] fn toggling_show_hidden_reapplies_without_rescan() { /* app with 1 hidden + 1 visible program; toggle → rows.len() 2; toggle back → 1 */ }
```

- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-gui`

- [ ] **Step 3: Implement**

- [ ] **Step 4: Run tests, expect pass**; `cargo clippy --workspace --all-targets` clean; run the release exe and verify the details panel for a program with an updater shows the updater row marked as not counted.

- [ ] **Step 5: Commit** — `git commit -m "feat(gui): details panel with per-exe usage and hidden-rows setting"`

---

## Self-review notes

- Spec coverage for plan 1 scope: §3 constraints (Tasks 1, 9), §4 window (Tasks 9–11), §5 data sources incl. "what counts as used" and user scope for loaded HKU hives (Tasks 3–8). §6 and §7 (uninstall, leftovers, quarantine) are deliberately plan 2; §9 future-proofing is honoured by keeping `Settings`, `paths` and `Row` generic.
- Names used across tasks: `Program`, `ProgramId`, `RegKeyPath`, `RegistryReader`, `FakeRegistry`, `UsageHit`, `UsageSource`, `UsageProvider`, `ProgramUsage`, `ExeUsage`, `LastUsed`, `ScanEvent`, `ScanSources`, `run_scan`, `Row`, `Filters`, `Sort`, `Settings` — consistent.
- Review Focus 1–5 are pinned by tests in Tasks 3, 3, 6, 7 and 8 respectively.
