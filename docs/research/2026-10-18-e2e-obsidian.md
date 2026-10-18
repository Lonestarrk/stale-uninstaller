# Headless end-to-end test of the uninstall flow on Obsidian (2026-10-18)

`crates/stale-core/examples/e2e.rs` drives the core directly (no GUI, unelevated) against a
per-user program: enumerate, leftover scan, vendor uninstaller (`--vendor-silent`, NSIS `/S`) or
Forced mode, scan again, quarantine the default-checked items, verify, restore, verify, quarantine
again, purge, verify. Target: Obsidian 1.14.4, installed with
`winget install --id Obsidian.Obsidian --scope user` (electron-builder NSIS, per-user).

Obsidian's uninstall key: `HKCU\...\Uninstall\bd400747-f0c1-5638-a859-982036102edf`, no
`InstallLocation` value; files in `%LOCALAPPDATA%\Programs\Obsidian` (not `%LOCALAPPDATA%\Obsidian`),
plus `%LOCALAPPDATA%\obsidian-updater\installer.exe` (325 MB), Start Menu and Desktop shortcuts,
ProgIDs `Markdown`, `PDF`, `Obsidian Base`, `Obsidian Canvas` and extension keys `.md`, `.pdf`,
`.canvas`, `.base` under `HKCU\Software\Classes`. Obsidian was never launched (no `%APPDATA%\obsidian`).

## Unelevated: the locked data folder blocks quarantine (as designed)

Without overrides, `QuarantineOps::live()` creates `%ProgramData%\Stale Uninstaller\quarantine`
locked (`O:<Jimmy>D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)`: unelevated, the owner cannot name
Administrators, and the DACL gives the user nothing). Quarantine then refuses before touching anything:

```
== 5. quarantine
  finished: 0 moved, 6 failures
    HKCU\...\Uninstall\bd400747-...: manifest could not be saved: open C:\ProgramData\Stale Uninstaller\quarantine: Åtkomst nekad. (os error 5)
    ... (same for all 6 items)
FAIL 5 quarantine: 0 moved, 0 keys, 0 values, 6 failures
```

Even in a user-owned data folder, `restore`/`purge` would refuse (`owned_by_admins` = `Some(false)`),
and `create_locked_dir` makes `<id>` folders the unelevated user cannot write into. Hence two
test-only overrides, never set by the application:

- `STALE_E2E_DATA_DIR=<folder>`: `paths::data_dir()` uses it instead of `%ProgramData%\Stale Uninstaller`.
- `STALE_E2E_TRUST_ALL=1` (exactly `1`): `QuarantineOps::live()` trusts every manifest folder and uses
  a mover that creates payload roots and `<id>` folders as plain folders (reparse points still refused).

The example pre-creates `<data dir>\quarantine` as a plain folder (else `paths::quarantine_manifest_dir()`
creates it locked). The runs below use
`STALE_E2E_DATA_DIR=%LOCALAPPDATA%\Temp\stale-e2e-data STALE_E2E_TRUST_ALL=1`.

## Run 1: `e2e Obsidian` (Forced)

```
== 1. enumerate programs
  name: Obsidian
  key: HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\bd400747-f0c1-5638-a859-982036102edf (scope CurrentUser)
  install_location: None
  uninstall_string: Some("\"C:\\Users\\Jimmy\\AppData\\Local\\Programs\\Obsidian\\Uninstall Obsidian.exe\" /currentuser")
  => 1 enumerate: PASS Obsidian

== 2. leftover scan (before uninstall)
  [x] High   RegKey   HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\bd400747-f0c1-5638-a859-982036102edf | - | 
  [x] High   RegKey   HKCU\Software\Classes\Markdown | - | 
  [x] High   RegKey   HKCU\Software\Classes\Obsidian Base | - | 
  [x] High   RegKey   HKCU\Software\Classes\Obsidian Canvas | - | 
  [x] High   RegKey   HKCU\Software\Classes\PDF | - | 
  [x] Medium Dir      C:\Users\Jimmy\AppData\Local\Programs\Obsidian | 387.6 MB | 
  --- 5 High, 1 Medium, 0 Low
  => 2 scan before: PASS 6 items

== plan
  3. Forced mode: vendor uninstaller skipped
  4. scan again; 5. quarantine the default-checked items; 6. verify;
  7. restore + verify; 8. quarantine again + purge + verify
  starting in 3 s ...
...
== 5. quarantine
  finished: 1 moved, 0 failures
  manifest 20261018-080034-ee18 for "Obsidian"
  manifest path: C:\Users\Jimmy\AppData\Local\Temp\stale-e2e-data\quarantine\20261018-080034-ee18\manifest.json
  moved (1):
    C:\Users\Jimmy\AppData\Local\Programs\Obsidian -> C:\Stale Uninstaller\quarantine\20261018-080034-ee18\C\Users\Jimmy\AppData\Local\Programs\Obsidian (dir, 387.6 MB)
  keys (5):
    HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\bd400747-f0c1-5638-a859-982036102edf -> C:\Users\Jimmy\AppData\Local\Temp\stale-e2e-data\quarantine\20261018-080034-ee18\key-0.reg
    HKCU\Software\Classes\Markdown -> C:\Users\Jimmy\AppData\Local\Temp\stale-e2e-data\quarantine\20261018-080034-ee18\key-1.reg
    HKCU\Software\Classes\Obsidian Base -> C:\Users\Jimmy\AppData\Local\Temp\stale-e2e-data\quarantine\20261018-080034-ee18\key-2.reg
    HKCU\Software\Classes\Obsidian Canvas -> C:\Users\Jimmy\AppData\Local\Temp\stale-e2e-data\quarantine\20261018-080034-ee18\key-3.reg
    HKCU\Software\Classes\PDF -> C:\Users\Jimmy\AppData\Local\Temp\stale-e2e-data\quarantine\20261018-080034-ee18\key-4.reg
  values (0):
  services: 0, tasks: 0, pending: 0
  failures (0):
  size on C: 387.6 MB
  => 5 quarantine: PASS 1 moved, 5 keys, 0 values, 0 failures
== 6. verify quarantine
  => 6 verify quarantine: PASS 0 problems

== 7. restore
  outcome: Restored
  => 7 restore: PASS 0 problems

== 8. quarantine again + purge
  6 of 6 items still exist
  finished: 1 moved, 0 failures
  moved (1):
    C:\Users\Jimmy\AppData\Local\Programs\Obsidian -> C:\Stale Uninstaller\quarantine\20261018-080034-9081\C\Users\Jimmy\AppData\Local\Programs\Obsidian (dir, 387.6 MB)
  keys (5):
  values (0):
  services: 0, tasks: 0, pending: 0
  failures (0):
  size on C: 387.6 MB
  purge: Ok(())
  => 8 quarantine+purge: PASS 0 problems

== result
PASS 1 enumerate: Obsidian
PASS 2 scan before: 6 items
PASS 3 vendor: skipped (Forced)
PASS 4 scan after: 6 items, 6 default-checked
PASS 5 quarantine: 1 moved, 5 keys, 0 values, 0 failures
PASS 6 verify quarantine: 0 problems
PASS 7 restore: 0 problems
PASS 8 quarantine+purge: 0 problems
```

After run 1: `winget list --id Obsidian.Obsidian` → "No installed package found";
`%LOCALAPPDATA%\Programs\Obsidian` gone, the four ProgIDs gone. Left behind (not found by the
scanner): Start Menu and Desktop `Obsidian.lnk`, `%LOCALAPPDATA%\obsidian-updater`, the extension
keys `.md`/`.pdf`/`.canvas`/`.base`. Obsidian was then reinstalled with winget.

## Run 2: `e2e Obsidian --vendor-silent`

```
== 1. enumerate programs
  name: Obsidian
  key: HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\bd400747-f0c1-5638-a859-982036102edf (scope CurrentUser)
  install_location: None
  uninstall_string: Some("\"C:\\Users\\Jimmy\\AppData\\Local\\Programs\\Obsidian\\Uninstall Obsidian.exe\" /currentuser")
  => 1 enumerate: PASS Obsidian

== 2. leftover scan (before uninstall)
  [x] High   RegKey   HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\bd400747-f0c1-5638-a859-982036102edf | - | 
  [x] High   RegKey   HKCU\Software\Classes\Markdown | - | 
  [x] High   RegKey   HKCU\Software\Classes\Obsidian Base | - | 
  [x] High   RegKey   HKCU\Software\Classes\Obsidian Canvas | - | 
  [x] High   RegKey   HKCU\Software\Classes\PDF | - | 
  [x] Medium Dir      C:\Users\Jimmy\AppData\Local\Programs\Obsidian | 387.6 MB | 
  --- 5 High, 1 Medium, 0 Low
  => 2 scan before: PASS 6 items

== plan
  3. run vendor uninstaller: Some(["C:\\Users\\Jimmy\\AppData\\Local\\Programs\\Obsidian\\Uninstall Obsidian.exe", "/currentuser", "/S"])
  4. scan again; 5. quarantine the default-checked items; 6. verify;
  7. restore + verify; 8. quarantine again + purge + verify
  starting in 3 s ...

== 3. vendor uninstaller
  exit: Ok(0); Uninstalled
  => 3 vendor: PASS Uninstalled, exit Ok(0)

== 4. leftover scan (after uninstall)
  --- 0 High, 0 Medium, 0 Low
  => 4 scan after: FAIL 0 items, 0 default-checked

== result
PASS 1 enumerate: Obsidian
PASS 2 scan before: 6 items
PASS 3 vendor: Uninstalled, exit Ok(0)
FAIL 4 scan after: 0 items, 0 default-checked
```

The NSIS uninstaller (`Uninstall Obsidian.exe /currentuser /S`, waited for through the Job Object)
exited 0 and removed the key, the install folder, both shortcuts and the four ProgIDs;
`still_installed` → false. The second scan found nothing, so steps 5–8 had nothing to work on and
step 4 is marked FAIL (the example requires something to quarantine). The quarantine path itself
was exercised in run 1.

After run 2: `winget list --id Obsidian.Obsidian` → "No installed package found"; no uninstall key,
no `%LOCALAPPDATA%\Programs\Obsidian`, no shortcuts, no ProgIDs. Still on disk:
`%LOCALAPPDATA%\obsidian-updater\installer.exe` (325 MB) and `HKCU\Software\Classes\.md`, `.pdf`,
`.canvas`, `.base` (default values naming the removed ProgIDs).

## Results

| Step | Run 1 (Forced) | Run 2 (vendor `/S`) |
|---|---|---|
| 1 enumerate (HKCU only) | PASS | PASS |
| 2 scan before | PASS (6 items) | PASS (6 items) |
| 3 vendor uninstaller | PASS (skipped) | PASS (Uninstalled, exit 0) |
| 4 scan after | PASS (6 default-checked) | FAIL (0 items: scanner gaps below) |
| 5 quarantine | PASS (1 dir 387.6 MB, 5 keys, 0 failures) | not run |
| 6 verify quarantine | PASS | not run |
| 7 restore + verify | PASS (Restored, manifest and payload folders gone) | not run |
| 8 quarantine again + purge + verify | PASS | not run |

## Scanner gaps found

1. No `InstallLocation` (common for electron-builder per-user installs) → `install_dir()` in
   `leftovers/other_scan.rs:13` returns `None`, so shortcuts, services and tasks are never found,
   although the folder is known from `DisplayIcon`/`UninstallString` (the registry scan already
   falls back to exe names, `reg_scan.rs:64`). The install folder itself is only found by the name
   match (Medium, `fs_scan.rs:119`) instead of High. Forced mode therefore left both `Obsidian.lnk`.
2. `%LOCALAPPDATA%\obsidian-updater` (electron-updater cache, 325 MB) is not matched:
   `name_matches_exact` (`fs_scan.rs:119`) does not accept `<name>-updater`.
3. Extension keys whose default value names a ProgID that was removed (`.canvas` → `Obsidian Canvas`)
   are not reported (`reg_scan.rs:222` only matches keys whose command refers to the program).
