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
- Lint: `cargo clippy --workspace --all-targets`
- Do not run `cargo fmt` on the workspace: the core is not rustfmt-formatted and the churn swamps reviews.

## Architecture

Stale Uninstaller is a Windows 10/11 x64 uninstaller (toolchain `stable-x86_64-pc-windows-msvc`) built as a Cargo workspace of two crates:

- `crates/stale-core` (lib): all logic, no egui. OS access is behind traits/closures so tests use fakes: `registry::RegistryReader` (`FakeRegistry`/`WinRegistry`), `devicepath::DeviceMap`, and the closures in `scan::ScanSources`. `scan::run_scan` streams `ScanEvent`s (`Programs → Warning* → Usage → UnmatchedHits → Sizes* → Done`) over an mpsc channel.
  - "Senast använd" sources: `usage/{userassist,bam,prefetch,filetime}.rs`; `usage/matching.rs` decides each program's main exe (shortcut target → `--processStart` argument → display icon → largest root exe), excludes updater/service/uninstaller exes, and treats file mtime as a pure fallback.
  - `tests/fixtures/prefetch/*.pf` is a real Windows 11 (format v31) Prefetch file used by the parser tests.
- `crates/stale-gui` (bin `StaleUninstaller`): eframe 0.37 front end. The `App` trait here is `logic()` (channel draining) + `ui()` (drawing), not `update()`. Unit tests live in the lib target (`src/lib.rs`) because the embedded admin manifest blocks the bin's test harness. All user-visible strings are Swedish and live in `src/strings.rs`; `filter.rs`/`format.rs` are pure and tested.

Persistent data goes under `%ProgramData%\Stale\` (settings.json, logs\stale.log). Add dependencies with `cargo add -p <crate> <dep>`. Design spec and plans are in `docs/superpowers/`.
