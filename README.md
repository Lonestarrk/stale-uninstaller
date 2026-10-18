# Stale Uninstaller

A Windows uninstaller that tells you **when you last used each program**, so you can find the ones that just take up space, remove them, and clean up what they leave behind.

Think Revo Uninstaller, but sorted by "last used" and showing which drive everything lives on.

> Status: early development. The program list with last-used dates works. Uninstalling, leftover cleanup and quarantine are being built (see [Roadmap](#roadmap)). The user interface is in Swedish.

## What it does

- **Lists every classic installed program** (from the Windows Uninstall registry keys) with publisher, version, size, install date and **drive letter**.
- **Shows when you last used it.** The date comes from Windows' own execution traces: Prefetch, Background Activity Moderator (BAM) and UserAssist. Only *your own launches of the main program* count. Updaters, background services, tray helpers and uninstallers never make a program look "used". If no trace exists, the file's modification time is shown as a low-confidence fallback.
- **Filters** for "not used in 6 months", "not used in a year", "never seen", per drive, and "over 1 GB". Sortable table, search, and a details panel that lists every executable with its last run and the source of the date.
- **Honest about uncertainty.** Each date shows which source it came from, and rows are marked "pending" until the scan has finished rather than showing a wrong value.

Planned (in progress):

- Queue several programs and uninstall them in one go, with an optional System Restore point.
- Find leftovers in the file system, registry, services, scheduled tasks and shortcuts, grouped by confidence. Anything that might be your own data is unchecked by default.
- **Quarantine instead of delete.** Removed files move to a per-drive quarantine folder and registry keys are exported first, so everything can be undone. Empty the quarantine when you need the space.
- Find "ghost" programs that are already uninstalled but left files and registry entries behind.

## Requirements

- Windows 10 or 11, 64-bit.
- Administrator rights (the program asks for elevation on start; Prefetch, BAM and parts of the registry need it).

## Building

Rust (stable, MSVC toolchain) and the Visual Studio "Desktop development with C++" workload are required.

```powershell
cargo build -p stale-gui --release
.\target\release\StaleUninstaller.exe
```

The release binary is a single statically linked exe with no runtime dependencies.

```powershell
cargo test --workspace          # unit tests (no registry or disk access)
cargo clippy --workspace --all-targets
cargo run -p stale-core --example dump   # prints the program list to the console; run elevated for full data
```

## How "last used" is decided

1. The program's **main executable** is identified: the Start Menu shortcut target, or the exe named in a `--processStart` argument (Squirrel-style apps like Discord), or the display icon, or the largest exe in the install folder.
2. Execution traces are collected from Prefetch (`C:\Windows\Prefetch`, including the Windows 11 24H2 format), BAM (`HKLM\SYSTEM\...\bam\State\UserSettings`) and UserAssist (`HKCU\...\Explorer\UserAssist`, including Start Menu shortcut launches).
3. A trace counts only if it belongs to a main executable, or is a UserAssist entry (a user launch). Executables whose name contains `update`, `service`, `helper`, `crash`, `report`, `daemon`, `tray` or `unins`, or that are registered as a Windows service, are excluded.
4. The newest counted trace is the date. If there is none, the main exe's modification time is used and marked as uncertain.

## Project layout

- `crates/stale-core` – all logic, no UI. Registry, file system and Windows APIs sit behind traits so the tests run against fakes.
- `crates/stale-gui` – the window, built with [egui](https://github.com/emilk/egui)/eframe.
- `docs/superpowers/specs` – the design document (Swedish).
- `docs/superpowers/plans` – implementation plans.
- `docs/research` – research notes.

## Roadmap

| Version | Scope | Status |
|---|---|---|
| 1, plan 1 | Program list with last-used, filters, details | Done |
| 1, plan 2 | Uninstall queue, leftover scan, quarantine with undo | In progress |
| 1, plan 3 | Ghost programs (uninstalled but leftovers remain) | Planned |
| 2 | Junk finder: temp folders, caches, AppData growth, with plain-language explanations and a "keep" list | Planned |
| 3 | Microsoft Store apps | Planned |

## License

Not yet decided.
