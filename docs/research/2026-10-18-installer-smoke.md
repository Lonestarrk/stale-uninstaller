# Installer smoke test, 0.1.0 (2026-10-18)

An install, launch and uninstall of the setup build on the development machine (Windows 11 Pro
10.0.26200, x64), run from an elevated PowerShell job. Run twice: first with a one-step cleanup,
then again after the cleanup was split into a deciding step and an acting step (below). This log
records the second run; the first gave the same results apart from the uninstall log lines.

## Build

- Inno Setup 6.7.3, installed with
  `winget install --id JRSoftware.InnoSetup -e --accept-package-agreements --accept-source-agreements --disable-interactivity`
  (exit code 0). winget put it in `%LOCALAPPDATA%\Programs\Inno Setup 6`, not in
  `%ProgramFiles(x86)%`, although the job ran elevated.
- `cargo build -p stale-gui --release`: `StaleUninstaller.exe`, 12 894 720 bytes, file and product
  version 0.1.0 in its version resource.
- `ISCC.exe /DAppVersion=0.1.0 installer\stale.iss`: compiled without warnings.
- `installer\Output\StaleUninstaller-Setup-0.1.0.exe`: 5 892 812 bytes,
  SHA-256 `74BAE1FCD10AFCC432FBD9DDD21ECCCDDBE0273D8C55F9025E271AF6887290E9`.

## Why the cleanup has two steps

Inno Setup calls `InitializeUninstall` before its own "Are you sure you want to completely remove…"
confirmation. A cleanup that acted there could empty the quarantine and then see the user decline
the uninstall. So `InitializeUninstall` only runs `--uninstall-cleanup --ask`, which decides
(exit code 10 remove the data folder, 11 empty the quarantine then remove, 12 keep everything,
2 cancel) and changes nothing. `CurUninstallStepChanged(usUninstall)`, which comes after the
confirmation and before any file is removed, runs `--uninstall-cleanup --apply remove|purge` for
10 or 11.

## Protecting the real data folder

`%ProgramData%\Stale Uninstaller` on this machine is in real use, so before anything else the job:

1. Counted the quarantine manifests (`quarantine\*\manifest.json`, trusted or not): 0. With any
   manifest present the uninstall half would have been skipped.
2. Backed the folder up with `robocopy /E /B /COPY:DATSO /DCOPY:DAT /XJ` (exit code 1 = files
   copied; 4 folders and 1 file, 1.5 kB) and saved the folder's own SDDL.

The folder held no `settings.json` or `important.json` at the time (only the log and empty
folders), so there was nothing of those two to restore.

## Steps and results

| Step | Result |
|---|---|
| Before | No `%ProgramFiles%\Stale Uninstaller`, no uninstall entry. |
| `StaleUninstaller-Setup-0.1.0.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=…` | Exit code 0. `StaleUninstaller.exe` and `unins000.exe` in `C:\Program Files\Stale Uninstaller`; uninstall key `{006E9CB5-0F91-495D-A821-CB149DD3217F}_is1` with DisplayName "Stale Uninstaller", DisplayVersion 0.1.0 and the exe as DisplayIcon; Start-menu shortcut present; installed exe version 0.1.0. |
| Launch the installed exe | Main window found within the timeout. The screenshot shows the program list (287 programs) in Swedish, the Windows display language. Stopped afterwards. |
| Run the installed exe with `--uninstall-cleanup --ask --silent` directly | Exit code 10 (no quarantine: remove), and the data folder still existed afterwards: the deciding step changed nothing. |
| `unins000.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=…` | Exit code 0. The uninstall log shows `Stale cleanup --ask exit code: 10`, then `Stale cleanup --apply remove exit code: 0`, then "Starting the uninstallation process" and the file deletions, so the acting step ran after the confirmation point and while the exe was still there. Afterwards the program folder, the uninstall key, the Start-menu shortcut and `%ProgramData%\Stale Uninstaller` were all gone, and no `C:\Stale Uninstaller` payload folder existed. |
| Restore | `robocopy` back with the same flags (exit code 1), the saved SDDL set on the folder again (identical afterwards), all 4 entries back, none missing. |

## Not covered here

- The Yes/No/Cancel question when the quarantine holds something: a message box cannot be
  answered from the headless job. The decision (`cleanup_action`), the exit-code mapping, the
  deciding step not touching anything, and `--apply remove` keeping a quarantine that appeared
  after the question are covered by unit tests, and the folder removal by a test that plants a
  junction pointing outside the folder and checks that its target survives.
- The interactive setup wizard, its language choice (English or Swedish by the Windows language)
  and Inno's interactive confirmation.

## Final build (new exe manifest and icon)

A third run of the same job against the final setup, after the exe manifest and icon changed.

- Setup: `installer\Output\StaleUninstaller-Setup-0.1.0.exe`, 5 861 074 bytes,
  SHA-256 `803CD099DFE8B9AC58764EB808481C536DF102D5977293EACFA0A89FD5C05362` (checked against the
  expected value before anything ran).
- Data folder protection as before: 0 quarantine manifests, backup with
  `robocopy /E /B /COPY:DATSO /DCOPY:DAT /XJ` (exit code 1, 4 entries) and the folder's SDDL saved.

| Step | Result |
|---|---|
| `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=…` | Exit code 0. Exe, `unins000.exe`, uninstall key (DisplayVersion 0.1.0) and Start-menu shortcut present. |
| Launch the installed exe | Started elevated with no side-by-side error (no SideBySide events in the Application log), main window found, program list of 287 programs shown. The title-bar icon is the new one: a dark rounded square with a light trash bin and an amber clock badge. Screenshot `.superpowers/elevated/shots/installer-final.png`. |
| `ExtractAssociatedIcon` on the installed exe | 32x32 icon, saved as `.superpowers/elevated/shots/installer-final-icon.png`; it shows the same new icon. |
| `--uninstall-cleanup --ask --silent` | Exit code 10 (no manifests); the data folder still existed afterwards. |
| `unins000.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=…` | Exit code 0. Log: `--ask` exit code 10, then `--apply remove` exit code 0, then the uninstall and the file deletions. Afterwards the program folder, the uninstall key, the Start-menu shortcut and `%ProgramData%\Stale Uninstaller` were gone, and no `C:\Stale Uninstaller` folder existed. |
| Restore | `robocopy` back with the same flags (exit code 1), SDDL identical, 4 of 4 entries back. The folder held no `settings.json` or `important.json`, so there was nothing of those to restore. |

## Version 0.1.1

Setup `StaleUninstaller-Setup-0.1.1.exe`, 5 859 420 bytes, SHA-256
`9D71A5F04CCF5A27A32262F856A132C14368C15459E673C887FC388E3AF1B20F`. Same procedure as the final
0.1.0 build: no quarantine manifests, data folder backed up and restored (4 of 4 entries, SDDL
identical).

- Silent install: exit 0; uninstall entry shows DisplayVersion 0.1.1; installed exe version 0.1.1.
- Elevated launch: main window shown, no side-by-side events, new icon in the title bar.
- `--uninstall-cleanup --ask --silent`: exit 10, data folder untouched.
- Silent uninstall: exit 0; log shows `--ask` exit 10, `--apply remove` exit 0, then file deletions;
  program folder, uninstall key, shortcut and `%ProgramData%\Stale Uninstaller` gone.
