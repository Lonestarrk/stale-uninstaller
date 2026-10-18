# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

- Build: `cargo build` (release: `cargo build -p stale-gui --release` → `target/release/StaleUninstaller.exe`, statically linked CRT)
- Installer (after the release build; Inno Setup 6, `ISCC.exe` is in `%ProgramFiles(x86)%\Inno Setup 6` or `%LOCALAPPDATA%\Programs\Inno Setup 6`): `ISCC.exe /DAppVersion=X.Y.Z installer\stale.iss` → `installer\Output\StaleUninstaller-Setup-X.Y.Z.exe`. The whole release routine (version bump, build, elevated install/uninstall smoke test, tag, GitHub release) is the project skill `.claude/skills/release/SKILL.md`.
- Run: `target\release\StaleUninstaller.exe` — the manifest requires elevation, so `cargo run -p stale-gui` fails with os error 740 from a non-elevated shell; launch the exe directly and accept the UAC prompt.
- Core smoke test against the real machine: `cargo run -p stale-core --example dump` (run elevated to get BAM/Prefetch data; prints one line per program sorted oldest-first).
- Dump modes (`cargo run -p stale-core --example dump -- <mode>`): `dump` (every program), `dump usage <name>`, `dump deps`, `dump provenance <name>`, `dump leftovers <name>`, `dump ghosts`.
- Headless end-to-end uninstall test (removes the program!): `cargo run -p stale-core --features e2e-overrides --example e2e -- Obsidian` with `STALE_E2E_DATA_DIR`/`STALE_E2E_TRUST_ALL=1` set; see `docs/research/2026-10-18-e2e-obsidian.md`.
- Test all: `cargo test --workspace`
- Single test: `cargo test -p stale-core <name>` / `cargo test -p stale-gui <name>`
- Live tests that read the real registry/Prefetch/shell are `#[ignore]`; run with `cargo test -p stale-core -- --ignored <name>` (some need elevation).
- Lint: `cargo clippy --workspace --all-targets`; the code behind the debug-only `e2e-overrides` feature also needs `cargo clippy -p stale-core --all-targets --examples --features e2e-overrides` and `cargo test -p stale-core --features e2e-overrides`
- Do not run `cargo fmt` on the workspace: the core is not rustfmt-formatted and the churn swamps reviews.

## Architecture

Stale Uninstaller is a Windows 10/11 x64 uninstaller (toolchain `stable-x86_64-pc-windows-msvc`) built as a Cargo workspace of two crates:

- `crates/stale-core` (lib): all logic, no egui. OS access is behind traits/closures so tests use fakes: `registry::RegistryReader` (`FakeRegistry`/`WinRegistry`), `devicepath::DeviceMap`, and the closures in `scan::ScanSources`. `scan::run_scan` streams `ScanEvent`s (`Programs → Warning* → Usage → UnmatchedHits → Sizes* → Done`) over an mpsc channel.
  - "Senast använd" sources: `usage/{userassist,bam,prefetch,filetime}.rs`; `usage/matching.rs` decides each program's main exe (shortcut target → `--processStart` argument → display icon → largest root exe), excludes updater/service/uninstaller exes, and treats file mtime as a pure fallback.
  - `tests/fixtures/prefetch/*.pf` is a real Windows 11 (format v31) Prefetch file used by the parser tests.
  - Leftovers (`leftovers/`): `ProgramHint` (install dir derived from `InstallLocation`, else the uninstaller's folder, never Temp/installer caches/user folders), scanners for folders (`fs_scan`), registry (`reg_scan`), services/tasks/shortcuts (`other_scan`), merged by `scan::scan_leftovers`. Every item carries a `Confidence` (High/Medium/Low) and an optional note key (`user-folder`, `suffix-match`, `used-by-other-program`, …) that the GUI maps to the user's language; Low is never pre-checked.
  - Uninstall (`uninstall/`): `runner` (job-object wait for the vendor uninstaller, `still_installed` check, MSI 1602/1603 → not uninstalled), `restore_point` (srclient), `orchestrator::run_queue` streaming `UninstallEvent`s.
  - Quarantine (`quarantine/`): per-volume payload under `<drive>:\Stale Uninstaller\quarantine\<id>`, manifests under `%ProgramData%\Stale Uninstaller\quarantine\<id>`; registry keys are exported with `reg.exe` before deletion; every destructive step is recorded as pending first; restore refuses occupied originals; cleanup never deletes unknown payload. `acl.rs` locks folders with a protected DACL through a pinned handle; `store::owned_by_admins` + reparse checks decide which manifest folders are trusted.
  - Ghosts (`ghosts/`): evidence collectors (`folders`, `registry`, `refs`), `group` (grouping, `Assessment` + reasons, `to_leftovers`), `scan::scan_ghosts`. `SKIP_FOLDERS`/`CONTAINER_FOLDERS`/`LAUNCHER_FOLDERS` hold the name-based rules; expect to tune them against real machines (`dump ghosts`). The reference collectors also report the existing targets they read (`LiveRefs`); a ghost folder holding one is Uncertain (`in-use`). Only ProbablyUninstalled ghosts are pre-checked.
  - `keep.rs`: the persistent keep list (`%ProgramData%\Stale Uninstaller\important.json`) consulted by both the leftover and ghost scanners. `session.rs`: interactive-session user vs token user (the "elevated under another account" warning).
  - `provenance.rs`: the details panel's signature (`WinVerifyTrust`, no revocation/network; falls back to the local system catalogs), version info and install source (library folder → MSI → winget → OEM → driver, first match wins) of a program's main exe (`ProgramUsage::main_exe`). `dump provenance <name>` prints it.
  - Everything the elevated process runs or trusts is pinned: tools come from `proc::system32_tool`, bare uninstaller names resolve to System32, paths are re-checked for reparse points before ACL writes and moves.
- `crates/stale-gui` (bin `StaleUninstaller`): eframe 0.37 front end. The `App` trait here is `logic()` (channel draining) + `ui()` (drawing), not `update()`. Unit tests live in the lib target (`src/lib.rs`) because the embedded admin manifest blocks the bin's test harness. All user-visible text lives in `src/strings.rs` in Swedish and English: fixed texts are fields of the `Strings` struct (`SV` and `EN`, read through `t()`), texts with values are functions that match on `lang()`, so every new text needs both languages or it does not compile. The language is a process-wide atomic (`set_lang`), set at start from `settings.json` or the Windows display language; tests must not switch it (use the `_in(lang, …)` variants). The core sends no sentences: warnings and error reasons are typed values or codes (`scan::ScanWarning`, `quarantine::codes`) that the GUI words, and the GUI keeps its own warnings as `warning::Warning` values worded when drawn; `filter.rs`/`format.rs`/`flow.rs`/`leftover_sel.rs`/`ghost_sel.rs` are pure and tested. Views: program list, leftovers, quarantine, ghosts (`View` enum); every long operation runs on a named worker thread in `worker.rs` and reports over an mpsc channel.

`StaleUninstaller.exe --uninstall-cleanup` is run by the installer's uninstaller (`installer/stale.iss`) in two phases, with no window and no single-instance check: `--ask [--silent]` in `InitializeUninstall` (before Inno's confirmation) only decides, asking Yes/No/Cancel when there are trusted quarantines, and exits 10 remove / 11 purge / 12 keep / 2 cancel; `--apply remove|purge` in `CurUninstallStepChanged(usUninstall)` does the work through `stale_core::selfclean::apply`, re-checking the quarantine and removing the data folder without following links.

Persistent data goes under `%ProgramData%\Stale Uninstaller\` (settings.json, important.json, logs\stale.log, quarantine\). The `e2e-overrides` cargo feature (debug builds only; `compile_error!` in release) enables the `STALE_E2E_*` env overrides used by `examples/e2e.rs` — never enable it for `stale-gui`. Add dependencies with `cargo add -p <crate> <dep>`. Design spec is in `docs/design/`, implementation plans in `docs/plans/`; research notes and e2e logs in `docs/research/`.

## Conventions

- Keep `README.md` up to date: whenever a change adds, removes or changes a user-visible feature, update its Features / Future features lists in the same commit. The README holds only a short description, features, future features, build instructions and the license — no status, plans or roadmap tables.
- Documentation is tool-neutral: design docs in `docs/design/`, implementation plans in `docs/plans/`, research and test logs in `docs/research/`. `.superpowers/` is local scratch and gitignored.
