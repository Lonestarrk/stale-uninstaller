# "Last used" for every account — Implementation Plan

**Goal:** "Senast använd" reflects use by every account on the computer, not only by the accounts that are signed in right now.

**Architecture:**
- **What is already machine-wide.** Prefetch is machine-wide. BAM (`HKLM\…\bam\State\UserSettings\<SID>`) already covers every SID.
- **What is missing for signed-out accounts.**
  - UserAssist launches, which are read only from HKCU and the hives loaded under `HKU`.
  - Per-user Run values.
  - Per-user Start-menu and Startup shortcuts, which drive main-exe detection and the autostart marker.
- **The new module `offline_hives`.** It makes signed-out accounts' `NTUSER.DAT` readable without ever touching the original. It copies the hive and its transaction logs into a private folder locked to administrators, then loads the copy with `RegLoadAppKeyW` (`KEY_READ`). A read-only `RegistryReader` overlay presents each copy as `HKU\<SID>`, next to the live `HKU` subkeys. It closes the copies and deletes them when the scan ends.
- **What uses the overlay.** Only the usage sources use it: UserAssist and Run values. Shortcut folders come from every profile directory.

**Spec (user decision 2026-10-18):** "senast använd ska gälla för alla användare på datorn, inte bara det kontot som är inloggat."

## Global constraints
- Never open, lock or modify another account's `NTUSER.DAT` or its logs in place.
  - Read them with backup semantics and copy them, together with `.LOG1` and `.LOG2` when present, into `%ProgramData%\Stale Uninstaller\hives\<random>`. That folder is created through `acl` with the admin-only DACL.
  - Load only the copy.
  - The copies are Stale's own scratch files, so delete them outright after unloading, and also at the next start if a crash left them behind.
- Skip a profile whose SID is already loaded under `HKU`; the live hive is used for it. Also skip:
  - profiles under the Windows folder (service profiles)
  - profiles whose directory or `NTUSER.DAT` is missing, unreadable or locked. Do this silently: log at debug level, no user warning.
- The program list is not affected. Other accounts' per-user installs are not added from offline hives, so the overlay is never given to `programs.rs`, the leftover scanners, ghosts or quarantine.
- Every user-visible string goes in both languages (`strings.rs`).
- Tests never load real hives. The overlay and the copy step sit behind seams and fakes. One `#[ignore]` live test runs elevated through the elevated runner.

## Review focus
1. A crash or early return leaves no loaded app key and no undeleted copy beyond the next start's cleanup.
2. The scan cost with many profiles. Each hive is about 5–50 MB, so copy only when the profile exists and is not already loaded. Release every copy before the scan's `Done` event.
3. A user who is signed in but whose hive is not under `HKU` yet (a race at logon). Copying fails because the file is locked, so skip that profile.

### Task 1: Offline user hives for usage

**Files:**
- Create `crates/stale-core/src/offline_hives.rs`.
- Modify `lib.rs`, `scan.rs` (`ScanSources::live`, `gather_usage`), `usage/userassist.rs`, `leftovers/reg_scan.rs` (`run_value_exes` over HKU), `shortcuts.rs` (per-profile Start-menu and Startup folders), `fsys.rs` or `paths.rs` as needed, `main.rs` (start-up cleanup of leftover copies), `README.md`, `CLAUDE.md`.

**Interfaces:**
- `pub struct OfflineHives`. It owns the loaded app keys and the copy folder. `Drop` closes every key and deletes the folder.
  - `pub fn load(profiles: &[(String /*SID*/, PathBuf /*profile dir*/)], loaded_sids: &HashSet<String>, ops: &dyn HiveOps) -> OfflineHives`
  - `pub fn sids(&self) -> Vec<String>`
- `pub trait HiveOps`. Live implementation: copy with backup semantics, `RegLoadAppKeyW`, read through the returned `HKEY`. Fake: an in-memory `FakeRegistry` per SID.
- `pub struct UsersOverlay<'a> { live: &'a dyn RegistryReader, offline: &'a OfflineHives }` implementing `RegistryReader`.
  - `subkeys(HKU root)` returns the live SIDs plus the offline SIDs.
  - `HKU\<offline SID>\…` is read from that copy.
  - Everything else is delegated to `live`.
- `pub fn profile_start_menu_dirs(profiles: &[PathBuf]) -> Vec<PathBuf>` and `profile_startup_dirs`. They return the common folder plus each profile's `AppData\Roaming\Microsoft\Windows\Start Menu\Programs` (and its `Startup`), existing folders only, deduplicated.
- `pub fn clean_stale_copies()`. Called once at start-up after `secure_data_dir()`. It removes `hives\*` left by a crash, never following reparse points.

- [ ] **Step 1: Failing tests**
  - `overlay_lists_live_and_offline_sids`
  - `overlay_reads_offline_userassist`: a UserAssist hit present only in an offline SID's fake hive reaches `gather_usage`.
  - `loaded_sid_is_not_copied`
  - `service_and_missing_profiles_are_skipped`
  - `locked_hive_is_skipped_silently`
  - `drop_closes_keys_and_removes_copies` (fake ops record the calls)
  - `run_values_of_offline_users_mark_autostart`
  - `start_menu_of_every_profile_feeds_main_exe_detection`
  - `program_list_ignores_offline_hives`
  - `#[ignore]` live `offline_hive_copy_loads_and_unloads`: elevated. Uses a profile of another, signed-out account when one exists, otherwise skips.
- [ ] **Step 2: Run, expect failure.**
- [ ] **Step 3: Implement.** README: the "last used" Features line says the date covers every account on the computer.
- [ ] **Step 4: Run all tests and both clippy variants.** Run elevated `dump` before and after through the elevated runner, and report programs whose date changed and why. Check that `%ProgramData%\Stale Uninstaller\hives` is empty after the run.
- [ ] **Step 5: Commit** — `feat: last used covers every account on the computer`
