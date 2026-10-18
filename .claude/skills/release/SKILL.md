---
name: release
description: Use when building the Stale Uninstaller installer or publishing a GitHub release
---

# Release routine

Builds `StaleUninstaller-Setup-X.Y.Z.exe` with Inno Setup, smoke-tests it elevated, then tags and
publishes the release on `Lonestarrk/stale-uninstaller`. Work on `main` after the release work is
merged. `X.Y.Z` below is the new version.

## 1. Version

- Set `version = "X.Y.Z"` in `crates/stale-core/Cargo.toml` and `crates/stale-gui/Cargo.toml`
  (the exe's version resource comes from the stale-gui version through `build.rs`; the exe
  manifest carries no version).
- Update the lockfile to the new versions: `cargo update -w`.
- Commit both Cargo.toml files and `Cargo.lock` together: `chore: version X.Y.Z`.

## 2. Test and build

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets
cargo build -p stale-gui --release
```

`build.rs` embeds `crates/stale-gui/stale.ico` as the exe icon and the setup uses it too
(`SetupIconFile`). It is generated: only if the design changes, edit and rerun
`crates/stale-gui/assets/make-icon.ps1`, look at its previews and commit the regenerated files.

## 3. Installer

Inno Setup 6 is needed once. Install it with
`winget install --id JRSoftware.InnoSetup -e --accept-package-agreements --accept-source-agreements --disable-interactivity`.
Depending on how winget runs it, `ISCC.exe` lands in `%ProgramFiles(x86)%\Inno Setup 6` or
`%LOCALAPPDATA%\Programs\Inno Setup 6`.

```powershell
& "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" /DAppVersion=X.Y.Z installer\stale.iss
# or: & "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe" /DAppVersion=X.Y.Z installer\stale.iss
Get-FileHash installer\Output\StaleUninstaller-Setup-X.Y.Z.exe
```

The setup lands in `installer\Output\` (gitignored). Note its SHA-256 for the release notes.
`stale.iss` stops with an error when `/DAppVersion` differs from the first three parts of the
version of `target\release\StaleUninstaller.exe`, so rebuild the exe after the version bump.

## 4. Elevated smoke test

Run it as a job through the elevated runner: drop a `.ps1` into `.superpowers/elevated/jobs/`,
run `schtasks /run /tn StaleDevElevated`, wait for a new `=== end` line in
`.superpowers/elevated/log.txt`. `lib.ps1` has `Save-Shot`/`Stop-Stale` and the window finder
`[G]::MainWindow(pid)`. Posted mouse clicks do not reach egui: launch and screenshot only.

`%ProgramData%\Stale Uninstaller` on the dev machine is real data. The job must:

1. Count the quarantine manifests (`%ProgramData%\Stale Uninstaller\quarantine\*\manifest.json`).
   If there are any, skip the launch (step 4) and the uninstall (step 5): the user's quarantine
   must not be touched, and starting Stale runs the automatic purge of old quarantines.
2. Back up the data folder: `robocopy <data> .superpowers\elevated\backup-<timestamp> /E /B /COPY:DATSO /DCOPY:DAT /XJ`,
   and save the folder's own SDDL (`(Get-Acl <data>).Sddl`).
3. Install silently: `StaleUninstaller-Setup-X.Y.Z.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=<file>`;
   check exit code 0, `%ProgramFiles%\Stale Uninstaller\StaleUninstaller.exe`, the uninstall key
   `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{006E9CB5-0F91-495D-A821-CB149DD3217F}_is1`
   and the Start-menu shortcut.
4. Only when step 1 found no manifests: launch the installed exe, find its window, `Save-Shot`,
   stop it. Look at the screenshot.
5. Uninstall (only when step 1 found no manifests):
   `%ProgramFiles%\Stale Uninstaller\unins000.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=<file>`;
   check `%ProgramFiles%\Stale Uninstaller`, the uninstall key, the shortcut and
   `%ProgramData%\Stale Uninstaller` are gone, and that the uninstall log has both phases:
   `Stale cleanup --ask exit code: 10` and `Stale cleanup --apply remove exit code: 0`.
6. Restore the backup with the same robocopy flags, put the saved SDDL back with `Set-Acl`, and
   check that `settings.json` and `important.json` are back.

Record the steps, exit codes, sizes and the setup's SHA-256 in
`docs/research/<date>-installer-smoke.md` (plain language).

## 5. Tag and publish

```powershell
git tag vX.Y.Z
git push origin vX.Y.Z
gh release create vX.Y.Z installer\Output\StaleUninstaller-Setup-X.Y.Z.exe --title "Stale Uninstaller X.Y.Z" --notes-file <notes.md>
```

The notes file (kept out of the repo, e.g. in `.superpowers/`) holds:

- the features of this release (from the README's Features list),
- "The installer is not code-signed yet, so Windows SmartScreen may warn on first run: choose
  More info → Run anyway.",
- the SHA-256 of the setup file.

The README's Download section points to the Releases page, so it needs no change per release.

## Uninstall behaviour (what the smoke test exercises)

Inno calls `InitializeUninstall` before its own "Are you sure…?" confirmation, so the cleanup runs
in two phases (`installer/stale.iss`):

1. `InitializeUninstall` runs `StaleUninstaller.exe --uninstall-cleanup --ask` (plus `--silent` for a
   silent uninstall). It only decides and changes nothing on disk. Exit codes: 10 = remove the data
   folder (no quarantine), 11 = purge then remove (the user chose Yes), 12 = keep everything (No, or
   silent with a quarantine), 2 = Cancel, which stops the uninstall. If the exe cannot run, 12.
2. `CurUninstallStepChanged(usUninstall)`, after the confirmation and before files are removed,
   runs `--uninstall-cleanup --apply remove|purge` for 10 or 11. It re-checks the quarantine: a
   quarantine that appeared since phase 1 is kept under `remove`, never purged without consent.
   It exits 0 when done and 1 when something could not be purged or removed.

A quarantine folder Stale cannot trust (not owned by Administrators, or unreadable) counts as a
quarantine too: it is never deleted, and while one exists the data folder is kept.
