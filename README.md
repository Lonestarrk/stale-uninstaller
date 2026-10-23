# Stale Uninstaller

A Windows uninstaller that tells you **when you last used each program**, so you can find the ones that just take up space, remove them, and clean up what they leave behind. The user interface is available in English and Swedish.

## Download

Download the latest installer from the [Releases page](https://github.com/Lonestarrk/stale-uninstaller/releases). The installer is not code-signed yet, so Windows SmartScreen may warn on first run.

## Features

- **Program list with "last used".** Every classic installed program with publisher, version, size, install date and **drive letter**, sorted so the programs you have not touched for the longest come first. The date comes from Windows' own execution traces (Prefetch, BAM, UserAssist) and covers every account on the computer, signed in or not. Launches of the program count, or of any of its own tools (shown as "via link.exe", and marked "autostart" when the program was started automatically at logon or by a scheduled task); updaters, background services, tray helpers and uninstallers never make a program look "used". Every date shows its source.
- **Filters and search.** "Not used in 6 months", "not used in a year", "never seen", per drive, "over 1 GB". Sortable table and a details panel listing every executable with its last run.
- **Where it came from.** The details panel shows who signed the program, its description, install date and source (Steam, Microsoft Store, winget, MSI, driver, preinstalled …).
- **Uninstall queue.** Tick several programs and uninstall them in one go, with an optional System Restore point. Each program's own uninstaller runs first; a cancelled or failed uninstall is detected and offered for retry or forced removal.
- **Dependency warning.** Runtimes, SDKs and developer tools such as the Visual C++ redistributables, .NET, Java, Python and Git are marked, and uninstalling one needs an extra confirmation.
- **Leftover cleanup.** After uninstalling, Stale finds what was left behind in the file system, registry, services, scheduled tasks and shortcuts, grouped by confidence. Anything that might be your own data is unchecked by default. Each program is one row you click to tick or untick; double-click it to see and pick the individual items.
- **Quarantine instead of delete.** Removed files move to a quarantine folder on the same drive and registry keys are exported first, so everything can be undone with one click. Empty the quarantine when you need the space, or let it clean itself after 30 days.
- **Ghost programs.** Finds programs that are already uninstalled but left folders, registry keys, dangling autostart, file-type, service, task or shortcut references, or execution traces behind. The evidence is grouped per program and rated "probably uninstalled", "leftovers only" or "uncertain". Uncertain ones, such as data that may belong to an installed program or another user, are never pre-checked. Each ghost is one compact row; a double-click shows its details and lets you untick single items, and the checked ones are removed straight from the ghosts view (moved to the quarantine, so they can be restored).
- **Keep list.** Mark any folder, key or program as "Keep" and it is left alone in every future scan.
- **Standalone leftover search** by name for programs that are no longer in the list.
- **Safety first.** Runs as administrator and treats everything it touches with suspicion: system tools are loaded only from System32, quarantine folders are locked down to administrators, junctions are refused, and nothing is deleted outright.
- **English and Swedish.** Follows the Windows display language on first start; switch any time in the settings menu.
- **Adjustable size.** The interface opens at 150 %; pick 100-200 % in the settings menu or zoom with Ctrl +, Ctrl - and Ctrl 0. The choice is remembered.
- **Installer that cleans up after itself.** Uninstalling Stale removes its data too; if the quarantine still holds anything, you choose whether to empty it, keep it, or cancel the uninstall.

## Future features

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

The release binary is a single statically linked exe. `cargo test --workspace` runs the unit tests without changing the registry or files outside temporary folders.

To build the installer, install [Inno Setup 6](https://jrsoftware.org/isinfo.php) and run its compiler on the script after the release build:

```powershell
& "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" /DAppVersion=0.1.0 installer\stale.iss
```

The setup is written to `installer\Output\StaleUninstaller-Setup-0.1.0.exe`.

## License

MIT, see [LICENSE](LICENSE).
