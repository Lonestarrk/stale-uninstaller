# Stale Uninstaller

A Windows uninstaller that tells you **when you last used each program**, so you can find the ones that just take up space, remove them, and clean up what they leave behind. The user interface is in Swedish.

## Features

- **Program list with "last used".** Every classic installed program with publisher, version, size, install date and **drive letter**, sorted so the programs you have not touched for the longest come first. The date comes from Windows' own execution traces (Prefetch, BAM, UserAssist). Your own launches of the program count, or of any of its own tools (shown as "via link.exe"); updaters, background services, tray helpers and uninstallers never make a program look "used". Every date shows its source.
- **Filters and search.** "Not used in 6 months", "not used in a year", "never seen", per drive, "over 1 GB". Sortable table and a details panel listing every executable with its last run.
- **Uninstall queue.** Tick several programs and uninstall them in one go, with an optional System Restore point. Each program's own uninstaller runs first; a cancelled or failed uninstall is detected and offered for retry or forced removal.
- **Leftover cleanup.** After uninstalling, Stale finds what was left behind in the file system, registry, services, scheduled tasks and shortcuts, grouped by confidence. Anything that might be your own data is unchecked by default.
- **Quarantine instead of delete.** Removed files move to a quarantine folder on the same drive and registry keys are exported first, so everything can be undone with one click. Empty the quarantine when you need the space, or let it clean itself after 30 days.
- **Ghost programs.** Finds programs that are already uninstalled but left folders, registry keys, dangling autostart, file-type, service, task or shortcut references, or execution traces behind. The evidence is grouped per program and rated "probably uninstalled", "leftovers only" or "uncertain". Uncertain ones, such as data that may belong to an installed program or another user, are never pre-checked.
- **Keep list.** Mark any folder, key or program as "Behåll" and it is left alone in every future scan.
- **Standalone leftover search** by name for programs that are no longer in the list.
- **Safety first.** Runs as administrator and treats everything it touches with suspicion: system tools are loaded only from System32, quarantine folders are locked down to administrators, junctions are refused, and nothing is deleted outright.

## Future features

- Installer and a downloadable release.
- Junk finder: Windows temp folders, update caches and growing AppData folders, explained in plain language with a safety rating, sharing the keep list.
- Microsoft Store apps.
- Elevation on demand instead of at start-up.
- Parallel scanning and an NTFS fast path for folder sizes.

## Requirements and building

Windows 10 or 11, 64-bit. Rust (stable, MSVC) and the Visual Studio "Desktop development with C++" workload.

```powershell
cargo build -p stale-gui --release
.\target\release\StaleUninstaller.exe
```

The release binary is a single statically linked exe. `cargo test --workspace` runs the unit tests without touching the registry or disk.

## License

MIT, see [LICENSE](LICENSE).
