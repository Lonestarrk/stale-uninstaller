# Installer smoke test, 0.1.0 (2026-10-18)

An install, launch and uninstall of the first setup build on the development machine (Windows 11
Pro 10.0.26200, x64), run from an elevated PowerShell job.

## Build

- Inno Setup 6.7.3, installed with
  `winget install --id JRSoftware.InnoSetup -e --accept-package-agreements --accept-source-agreements --disable-interactivity`
  (exit code 0). winget put it in `%LOCALAPPDATA%\Programs\Inno Setup 6`, not in
  `%ProgramFiles(x86)%`, although the job ran elevated.
- `cargo build -p stale-gui --release`: `StaleUninstaller.exe`, 12 893 184 bytes, file and product
  version 0.1.0 in its version resource.
- `ISCC.exe /DAppVersion=0.1.0 installer\stale.iss`: compiled without warnings.
- `installer\Output\StaleUninstaller-Setup-0.1.0.exe`: 5 893 278 bytes,
  SHA-256 `9CA9DF4F7282DA5648E343A8A9668E56930B4FC8ED3CA8FCECC78A60945B0ABC`.

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
| Launch the installed exe | Main window found within the timeout. The screenshot shows the program list loading (287 programs, sizes filling in), Swedish UI as the Windows display language. Stopped afterwards. |
| `unins000.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=…` | Exit code 0. The uninstall log shows `Stale cleanup exit code: 0` (the `--uninstall-cleanup --silent` step found no quarantine). Afterwards the program folder, the uninstall key, the Start-menu shortcut and `%ProgramData%\Stale Uninstaller` were all gone, and no `C:\Stale Uninstaller` payload folder existed. |
| Restore | `robocopy` back with the same flags (exit code 1), the saved SDDL set on the folder again (identical afterwards), all 4 entries back, none missing. |

## Not covered here

- The Yes/No/Cancel question when the quarantine holds something: a message box cannot be
  answered from the headless job. The decision (`cleanup_action`) and the purge/keep/abort paths
  of `selfclean::run` are covered by unit tests, and the folder removal by a test that plants a
  junction pointing outside the folder and checks that its target survives.
- The interactive setup wizard and its language choice (English or Swedish by the Windows language).
