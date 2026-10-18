# Stale Uninstaller – Plan 4: Trustworthy "Last Used", Provenance, Fixes and First Release

**Goal:** "Senast använd" is right on real machines (incl. subcomponent usage like Visual Studio via link.exe), the details panel tells where a program came from, the parked plan-2/3 defects are fixed, and v0.1.0 ships as an Inno Setup installer on GitHub Releases with a release routine documented as a project skill.

**Architecture:** Core changes stay in `stale-core` (usage matching, a new `provenance` module, path/ACL fixes); GUI changes are confined to the program table, details panel and queue dialog. The installer is an Inno Setup script in `installer/`, built from the release exe; `StaleUninstaller.exe --uninstall-cleanup` handles Stale's own data at uninstall time.

**Tech Stack:** as before; Inno Setup 6 (`JRSoftware.InnoSetup` via winget), `gh` CLI for the release.

**Spec:** `docs/design/2026-10-18-plan4-design.md` §3 (last used), §8 (provenance), §10 (installer/release), §11 (parked fixes). §4–§7, §9 and §12–§13 (folder explainer, junk finder, space view, Docker/WSL, their performance and views) are **plan 5**.

## Global Constraints

- Windows 10/11 x64; `stale-core` never depends on egui; OS access behind traits/closures; tests never touch the real registry/disk outside tempdirs.
- Elevated verification on the developer's machine runs through the scheduled task `StaleDevElevated` (drop a `.ps1` into `.superpowers/elevated/jobs/`, `Start-ScheduledTask -TaskName StaleDevElevated`, read `.superpowers/elevated/log.txt`; `lib.ps1` has `Start-Stale`, `Save-Shot`, `Click-At`, `Stop-Stale`).
- All UI strings Swedish in `strings.rs`. README Features updated in the same commit as any user-visible change (CLAUDE.md "Conventions").
- License MIT. Docs tool-neutral (`docs/design`, `docs/plans`, `docs/research`).
- Commit per task; trailer lines from the session reminder.

## Review Focus

1. **A program whose only recent runs are of a non-main, non-excluded exe** (Visual Studio: `link.exe` under `…\VC\Tools\MSVC\…\bin\Hostx64\x64\`): shows "Idag (via link.exe)", not "Aldrig sedd". Pinned in Task 2.
2. **An updater still never makes a program look used** (Chrome's `GoogleUpdate.exe` / `updater.exe`): the subcomponent rule must keep the name/service exclusions. Pinned in Task 2.
3. **A program installed under a Steam library on another drive** (`H:\SteamLibrary\steamapps\common\Game\game.exe`) with a BAM hit: matched to the Steam-registered game entry. Pinned in Task 1 (diagnosed live) and its fix test.
4. **Uninstalling Stale with items still in quarantine**: the user is asked; nothing is deleted silently and nothing admin-locked is stranded without being told. Pinned in Task 6.
5. **A signed exe whose signature does not verify** (tampered/expired): provenance shows "ogiltig signatur", never the signer name as if trusted. Pinned in Task 4.

---

### Task 1: Usage diagnostics and the live matching fix

**Files:** Modify `crates/stale-core/examples/dump.rs`, `crates/stale-core/src/usage/matching.rs` (and whichever of `usage/{bam,prefetch,userassist}.rs`, `programs.rs`, `scan.rs` the diagnosis implicates).

**Interfaces:**
- Produces: `dump usage <name-substring>` prints, for each matching program: install_location, effective install dir, main exes (with which rule picked them), every `UsageHit` from every provider whose exe file name or path relates to the program (path under install dir, OR file name equal to any exe found under the install dir up to depth 3), and for each hit whether `match_usage` assigned it and, if not, why (`no-candidate`, `excluded:<reason>`, `drive-less-unmatched`, `assigned-to:<other program>`).
- Produces: `pub fn explain_match(input: &MatchInput, program: &ProgramId) -> Vec<HitExplanation>` in `matching.rs` (`HitExplanation { hit: UsageHit, outcome: String }`) used by the dump mode — keeps the diagnosis logic testable.

- [ ] **Step 1: Write failing unit tests for `explain_match`**: `explains_hit_outside_install_dir_as_no_candidate`, `explains_excluded_updater`, `explains_hit_assigned_to_other_program`.
- [ ] **Step 2: Run, expect failure** — `cargo test -p stale-core explain_match`
- [ ] **Step 3: Implement `explain_match` and the `dump usage` mode.**
- [ ] **Step 4: Run elevated** via `StaleDevElevated`: `dump usage Chrome`, `dump usage Steam`, `dump usage Discord`, `dump usage "Visual Studio Professional"`, `dump usage Battlefield`. Paste the outputs into `docs/research/2026-10-18-usage-diagnosis.md` with a one-paragraph root-cause statement per program.
- [ ] **Step 5: Fix the root causes** found in Step 4, each with a regression test built from the observed data (fake providers/registry reproducing the exact paths seen). Expected suspects to confirm or rule out: Chrome's Uninstall entry has no `InstallLocation` and an icon in `…\Application\chrome.exe` while the uninstall string points into a versioned `Installer\setup.exe`; Steam games registered with `InstallLocation` on another volume while BAM uses `\Device\HarddiskVolumeN` paths; Discord's Squirrel `app-x.y.z` folders; BAM SIDs other than the current user's.
- [ ] **Step 6: Re-run elevated `dump`** and record before/after counts of "never" (baseline 173/286) in the research note.
- [ ] **Step 7: Commit** — `fix(core): usage matching on real machines, with dump usage diagnostics`

---

### Task 2: Subcomponents count as use

**Files:** Modify `crates/stale-core/src/usage/matching.rs`, `crates/stale-gui/src/format.rs`, `crates/stale-gui/src/ui/table.rs`, `crates/stale-gui/src/ui/details.rs`, `crates/stale-gui/src/strings.rs`, `README.md`.

**Interfaces:**
- Consumes: `ExeUsage`, `ExcludeReason`, `LastUsed`, `exclude_reason`.
- Produces: `LastUsed` gains `pub via: Option<String>` (serde default) — the exe file name when the date came from a non-main exe, else `None`. `exclude_reason(exe, is_main, service_exes)` no longer returns `NotMain`; non-main exes that are not name-excluded and not services count. `ExcludeReason::NotMain` stays in the enum for deserialisation but is never produced (doc comment). `format::last_used_label(when, via, now) -> String` → `"Idag (via link.exe)"` when `via` is `Some`.

- [ ] **Step 1: Write failing tests** in `matching.rs`: `non_main_exe_counts_and_sets_via` (BAM hit on `…\VC\Tools\MSVC\14.50\bin\Hostx64\x64\link.exe` under a VS install location, no main-exe hit → `when` = that hit, `via = Some("link.exe")`); `main_exe_hit_wins_without_via` (main exe yesterday, other exe today → `when` = today, `via = Some(...)`; main exe today → `via = None`); `updater_still_excluded` (Review Focus 2: `GoogleUpdate.exe`, `updater.exe` under Chrome's dir → not counted); `service_exe_still_excluded`. In `format.rs`: `last_used_label_shows_via`.
- [ ] **Step 2: Run, expect failure** — `cargo test --workspace via`
- [ ] **Step 3: Implement**; table "Senast använd" column uses `last_used_label`; details panel's exe list shows "räknas" for non-main exes; README Features line for "last used" mentions subcomponents ("…or any of its own tools, shown as 'via link.exe'").
- [ ] **Step 4: Run tests, expect pass**; elevated `dump usage "Visual Studio Professional"` shows `via link.exe` with today's date.
- [ ] **Step 5: Commit** — `feat: subcomponent usage counts, shown as "via <exe>"`

---

### Task 3: Dependency warning

**Files:** Create `crates/stale-core/src/dependencies.rs`; modify `lib.rs`, `crates/stale-gui/src/{app.rs,strings.rs}`, `ui/{table.rs,uninstall_dialog.rs}`, `README.md`.

**Interfaces:**
- Produces: `pub fn is_shared_dependency(p: &Program) -> Option<&'static str>` — returns a reason key when the program is a runtime/SDK others rely on: display name matches (case-insensitive) one of `Microsoft Visual C++ * Redistributable`, `Microsoft .NET *Runtime*`, `Microsoft .NET SDK*`, `Microsoft Windows Desktop Runtime*`, `Microsoft ASP.NET Core*`, `Windows Software Development Kit*`, `Windows SDK*`, `Microsoft Edge WebView2 Runtime`, `Java*`/`*JDK*`/`*JRE*`, `Python * (64-bit)`/`Python *`, `Node.js`, `Git`, `Visual Studio Build Tools*`, `Microsoft Visual Studio Installer`, `DirectX*`, `Microsoft XNA*`, `PhysX*`, `Vulkan Run Time*`, `OpenAL`; reason keys `"runtime"`, `"sdk"`, `"toolchain"`.
- GUI: such rows show a small badge `strings::DEPENDENCY_BADGE` ("används av andra program"); `open_queue_dialog_for` marks them, and the summary dialog shows `strings::DEPENDENCY_WARNING` ("Detta program används troligen av andra program. Avinstallera bara om du vet att inget behöver det.") with a per-item checkbox "Jag förstår" that must be ticked before Starta is enabled.

- [ ] **Step 1: Write failing tests**: `vc_redist_is_runtime`, `dotnet_sdk_is_sdk`, `git_and_python_are_toolchains`, `ordinary_program_is_not_dependency`; GUI `starta_disabled_until_dependency_acknowledged`.
- [ ] **Step 2: Run, expect failure.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run tests, expect pass.**
- [ ] **Step 5: Commit** — `feat: warn before uninstalling runtimes, SDKs and toolchains`

---

### Task 4: Provenance in the details panel

**Files:** Create `crates/stale-core/src/provenance.rs`; modify `lib.rs`, `crates/stale-gui/src/{worker.rs,app.rs,strings.rs}`, `ui/details.rs`, `README.md`. Windows features as needed (`Win32_Security_WinTrust`, `Win32_Security_Cryptography`, `Win32_Storage_FileSystem` for version info) — add with `cargo add -p stale-core windows --features <existing…>,<new>`.

**Interfaces:**
```rust
#[derive(Clone, Debug, PartialEq)] pub enum Signature { Valid { signer: String }, Invalid { signer: Option<String>, reason: String }, Unsigned, Unknown }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum InstallSource { Winget, MicrosoftStore, Steam, Epic, Gog, EaApp, Ubisoft, Driver, Oem, Msi, Unknown }
#[derive(Clone, Debug, PartialEq)] pub struct Provenance { pub signature: Signature, pub description: Option<String>, pub company: Option<String>, pub install_date: Option<NaiveDate>, pub source: InstallSource }
pub trait ProvenanceProbe: Send + Sync { fn signature(&self, exe: &Path) -> Signature; fn version_info(&self, exe: &Path) -> (Option<String> /*FileDescription*/, Option<String> /*CompanyName*/); }
pub struct WinProbe; // WinVerifyTrust(WINTRUST_ACTION_GENERIC_VERIFY_V2, revocation off, UI none) for validity; CryptQueryObject + CertGetNameStringW(CERT_NAME_SIMPLE_DISPLAY_TYPE) for the signer; GetFileVersionInfoSizeW/GetFileVersionInfoW/VerQueryValueW (\VarFileInfo\Translation then \StringFileInfo\<lang><cp>\FileDescription|CompanyName)
pub fn install_source(p: &Program, reg: &dyn RegistryReader) -> InstallSource; // path under a Steam library (`steamapps\common`) → Steam; `Epic Games` → Epic; `GOG Galaxy\Games`/`GOG Games` → Gog; `EA Games`/`Origin Games` → EaApp; `Ubisoft Game Launcher\games` → Ubisoft; `WindowsApps` → MicrosoftStore; `WindowsInstaller=1` → Msi; uninstall key `InstallSource` containing `\WinGet\` or `winget` → Winget; publisher matches a known OEM list (ASUS, ASUSTeK, Dell, HP, Lenovo, Acer, MSI, Micro-Star, Gigabyte, Razer) and installed within 30 days of the OS install date → Oem; display name containing `Driver`/`drivrutin` or publisher in a driver-vendor list (NVIDIA, AMD, Advanced Micro Devices, Intel, Realtek, Creative) → Driver; else Unknown
pub fn provenance(p: &Program, main_exe: Option<&Path>, probe: &dyn ProvenanceProbe, reg: &dyn RegistryReader) -> Provenance;
```
- GUI: when a row is selected, `worker::spawn_provenance(program, main_exe)` computes it off the UI thread (WinVerifyTrust can take a second) and caches per `ProgramId`; the details panel shows rows `Signerad av` (`"{signer} ✓"` / `"ogiltig signatur: {reason}"` / `"osignerad"`), `Beskrivning`, `Tillverkare`, `Installerad`, `Källa` (Swedish label per `InstallSource`).

- [ ] **Step 1: Write failing tests** with a fake probe and `FakeRegistry`: `steam_library_path_is_steam`, `winget_install_source_is_winget`, `oem_publisher_near_os_install_is_oem`, `invalid_signature_never_shows_as_valid` (Review Focus 5: `Invalid { signer: Some("X") }` → details text contains "ogiltig", not "X ✓"); `#[ignore]` live `win_probe_reads_notepad` (`C:\Windows\System32\notepad.exe` → Valid with "Microsoft" in signer, description non-empty) — run once.
- [ ] **Step 2: Run, expect failure.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run tests, expect pass**; elevated GUI check: open Stale, select Visual Studio Professional, `Save-Shot`, confirm the provenance rows in the screenshot.
- [ ] **Step 5: Commit** — `feat: program provenance (signature, source, description) in details`

---

### Task 5: Parked plan-2/3 fixes

**Files:** as each item requires (`pathutil.rs`, `proc.rs`, `ghosts/{mod,refs,registry}.rs`, `acl.rs`, `keep.rs`, `leftovers/mod.rs`, `ui/uninstall_dialog.rs`).

One commit per numbered item; each with its test:
1. `resolve_unquoted_path` requires a file (`exists && !is_dir`) — test `directory_prefix_is_not_accepted`.
2. Tests that mutate `SystemRoot` hold a crate-wide `static ENV_LOCK: Mutex<()>`, and every test reading `SystemRoot`/`ProgramData` through env takes it — test file compiles and the suite passes 10 times in a row (`for i in 1..10 { cargo test -p stale-core }` reported).
3. Live targets normalised with `GetLongPathNameW` (falls back to the input) before `norm` — test via an injected `long_path: &dyn Fn(&Path) -> PathBuf`: `short_path_live_target_marks_long_folder_in_use`.
4. Planted settings/keep files: `take_over_file` opens with `FILE_SHARE_READ` only; if neither take-over nor move-aside succeeds, the file is ignored (defaults used) and a warning is logged — test with `DirOps`: `untakeable_settings_file_is_ignored`.
5. Live-target gaps: kept registry keys still contribute live targets; `Services\*\Parameters\ServiceDll` is a live target; `rundll32 <dll>,…` Run values add the DLL as a live target — tests `service_dll_is_live`, `rundll32_dll_is_live`, `kept_key_target_is_live`.
6. `pathutil::shared_folders`/`users_root` use `paths::windows_dir()`/`program_data_dir()` (API) — test `shared_folders_ignore_env_override` (under `ENV_LOCK`).
7. Over-broad removals: a task with several commands where only one is dead → `Low` with note `"partly-live"`; a CLSID with one live server → not listed; a vendor key whose subkeys include an installed program's name → Low — tests `task_with_live_command_is_low`, `clsid_with_live_server_not_listed`.
8. `format_location`/`parse_location` use explicit hive names (`HKLM`, `HKCU`, `HKU`) instead of `Debug`, with backward-compatible parsing of the old form — test `location_roundtrip_and_legacy_form`.
9. Other profiles' `AppData\Local\Temp` rejected by `in_temp_or_installer_cache` — test `other_profile_temp_is_rejected`.
10. Keep list load caps the file at 1 MiB (larger → defaults + warning) — test `oversized_keep_list_is_ignored`.
11. Queue dialog renders the OtherUser uninstaller path with normal (not weak) text and a warning colour — no unit test; elevated `Save-Shot` not required.

---

### Task 6: Installer, `--uninstall-cleanup`, release routine

**Files:** Create `installer/stale.iss`, `.claude/skills/release/SKILL.md`; modify `crates/stale-gui/src/main.rs`, `crates/stale-gui/src/strings.rs`, `crates/stale-core/src/quarantine/ops.rs` (a `purge_all`/`summary` helper if missing), `README.md` (download section), `CLAUDE.md` (release command line).

**Interfaces:**
- `StaleUninstaller.exe --uninstall-cleanup`: no main window; lists trusted manifests; if none → removes `%ProgramData%\Stale Uninstaller` (after `secure_dir` checks, never following reparse points) and exits 0. If some → `MessageBoxW` (Yes/No/Cancel) with `strings::uninstall_cleanup_prompt(total_bytes)` ("Karantänen innehåller {size}. Ja = töm den och ta bort Stales data. Nej = behåll karantänen och datan. Avbryt = avbryt avinstallationen."): Yes → `purge` every manifest then remove the data dir, exit 0; No → exit 0 leaving everything; Cancel → exit 2.
- `installer/stale.iss`: `AppName=Stale Uninstaller`, `AppVersion` from a `/DAppVersion=` define, `DefaultDirName={autopf}\Stale Uninstaller`, `PrivilegesRequired=admin`, `ArchitecturesAllowed=x64compatible`, `ArchitecturesInstallIn64BitMode=x64compatible`, `OutputBaseFilename=StaleUninstaller-Setup-{#AppVersion}`, `LicenseFile=..\LICENSE`, Start-menu shortcut, optional desktop shortcut task, `UninstallDisplayIcon={app}\StaleUninstaller.exe`; `[Code]` `InitializeUninstall` runs `{app}\StaleUninstaller.exe --uninstall-cleanup` and returns `False` when the exit code is 2 (Review Focus 4).
- `.claude/skills/release/SKILL.md` (`name: release`, description: "Use when building the Stale Uninstaller installer or publishing a GitHub release"): steps — bump `version` in both crate manifests; `cargo test --workspace`; `cargo build -p stale-gui --release`; `"%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe" /DAppVersion=X.Y.Z installer\stale.iss`; elevated install/uninstall smoke test through `StaleDevElevated` (install silently `/VERYSILENT /SUPPRESSMSGBOXES`, launch, `Save-Shot`, uninstall `/VERYSILENT`, check `%ProgramFiles%\Stale Uninstaller` gone); `git tag vX.Y.Z && git push origin vX.Y.Z`; `gh release create vX.Y.Z installer\Output\StaleUninstaller-Setup-X.Y.Z.exe --title … --notes-file …` (notes: features, SmartScreen note for unsigned builds, SHA-256 of the setup); README download section points to the Releases page.

- [ ] **Step 1: Write failing tests** for the cleanup decision as a pure function: `cleanup_action(manifests: usize, choice: Option<Choice>) -> CleanupAction` (`RemoveDataDir`, `PurgeThenRemove`, `KeepAll`, `AbortUninstall`) — `no_manifests_removes_data_dir`, `yes_purges_then_removes`, `no_keeps_all`, `cancel_aborts`.
- [ ] **Step 2: Run, expect failure.**
- [ ] **Step 3: Implement** `--uninstall-cleanup`, the `.iss`, the skill, README download section ("Download the latest installer from the Releases page. The installer is not code-signed yet, so Windows SmartScreen may warn on first run.").
- [ ] **Step 4: Install Inno Setup** via the elevated runner (`winget install --id JRSoftware.InnoSetup -e --accept-package-agreements --accept-source-agreements --disable-interactivity`), build `installer\Output\StaleUninstaller-Setup-0.1.0.exe`, run the elevated smoke test from the skill (install, launch + `Save-Shot`, uninstall with an empty quarantine, verify the program folder and `%ProgramData%\Stale Uninstaller` are gone). Record the log in `docs/research/2026-10-18-installer-smoke.md`.
- [ ] **Step 5: Commit** — `feat: Inno Setup installer, uninstall cleanup and release routine`
- [ ] **Step 6 (controller, after the final review and merge):** follow the skill to tag `v0.1.0` and publish the GitHub release with the setup attached.

---

## Self-review notes

- Spec coverage: design §3.1 (Task 1), §3.2 (Task 2), §3.3 (Task 3), §8 (Task 4), §10 (Task 6), §11 (Task 5). §4–§7, §9, §12–§13 → plan 5 (stated in the header).
- Names consistent: `LastUsed.via`, `format::last_used_label`, `explain_match`, `HitExplanation`, `is_shared_dependency`, `Provenance`, `Signature`, `InstallSource`, `ProvenanceProbe`, `WinProbe`, `install_source`, `provenance`, `cleanup_action`, `CleanupAction`.
- Review Focus 1–5 pinned in Tasks 2, 2, 1, 6, 4.
