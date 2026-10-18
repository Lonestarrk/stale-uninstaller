# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

- Build: `cargo build` (release: `cargo build -p stale-gui --release` → `target/release/StaleUninstaller.exe`, statically linked CRT)
- Run: `target\release\StaleUninstaller.exe` — the manifest requires elevation, so `cargo run -p stale-gui` fails with os error 740 from a non-elevated shell; launch the exe directly and accept the UAC prompt.
- Core smoke test against the real machine: `cargo run -p stale-core --example dump` (run elevated to get BAM/Prefetch data; prints one line per program sorted oldest-first).
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
  - Leftovers (`leftovers/`): `ProgramHint` (install dir derived from `InstallLocation`, else the uninstaller's folder, never Temp/installer caches/user folders), scanners for folders (`fs_scan`), registry (`reg_scan`), services/tasks/shortcuts (`other_scan`), merged by `scan::scan_leftovers`. Every item carries a `Confidence` (High/Medium/Low) and an optional note key (`user-folder`, `suffix-match`, `used-by-other-program`, …) that the GUI maps to Swedish; Low is never pre-checked.
  - Uninstall (`uninstall/`): `runner` (job-object wait for the vendor uninstaller, `still_installed` check, MSI 1602/1603 → not uninstalled), `restore_point` (srclient), `orchestrator::run_queue` streaming `UninstallEvent`s.
  - Quarantine (`quarantine/`): per-volume payload under `<drive>:\Stale Uninstaller\quarantine\<id>`, manifests under `%ProgramData%\Stale Uninstaller\quarantine\<id>`; registry keys are exported with `reg.exe` before deletion; every destructive step is recorded as pending first; restore refuses occupied originals; cleanup never deletes unknown payload. `acl.rs` locks folders with a protected DACL through a pinned handle; `store::owned_by_admins` + reparse checks decide which manifest folders are trusted.
  - Ghosts (`ghosts/`): evidence collectors (`folders`, `registry`, `refs`), `group` (grouping, `Assessment` + reasons, `to_leftovers`), `scan::scan_ghosts`. `SKIP_FOLDERS`/`CONTAINER_FOLDERS` hold the name-based skip rules; expect to tune them against real machines (`dump ghosts`).
  - `keep.rs`: the persistent keep list (`%ProgramData%\Stale Uninstaller\important.json`) consulted by both the leftover and ghost scanners. `session.rs`: interactive-session user vs token user (the "elevated under another account" warning).
  - Everything the elevated process runs or trusts is pinned: tools come from `proc::system32_tool`, bare uninstaller names resolve to System32, paths are re-checked for reparse points before ACL writes and moves.
- `crates/stale-gui` (bin `StaleUninstaller`): eframe 0.37 front end. The `App` trait here is `logic()` (channel draining) + `ui()` (drawing), not `update()`. Unit tests live in the lib target (`src/lib.rs`) because the embedded admin manifest blocks the bin's test harness. All user-visible strings are Swedish and live in `src/strings.rs`; `filter.rs`/`format.rs`/`flow.rs`/`leftover_sel.rs`/`ghost_sel.rs` are pure and tested. Views: program list, leftovers, quarantine, ghosts (`View` enum); every long operation runs on a named worker thread in `worker.rs` and reports over an mpsc channel.

Persistent data goes under `%ProgramData%\Stale Uninstaller\` (settings.json, important.json, logs\stale.log, quarantine\). The `e2e-overrides` cargo feature (debug builds only; `compile_error!` in release) enables the `STALE_E2E_*` env overrides used by `examples/e2e.rs` — never enable it for `stale-gui`. Add dependencies with `cargo add -p <crate> <dep>`. Design spec and plans are in `docs/superpowers/`; research notes and e2e logs in `docs/research/`.
