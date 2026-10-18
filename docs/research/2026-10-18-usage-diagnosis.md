# "Senast använd" on a real machine: diagnosis (2026-10-18)

The developer's machine (Windows 11, three users' hives loaded, games on `F:`/`H:`) showed
174 of 285 visible programs as "never seen" although BAM, Prefetch and UserAssist had traces for
many of them (Chrome, Steam, Discord, Visual Studio, Battlefield 6). `dump usage <name>` (in
`crates/stale-core/examples/dump.rs`) prints, per matching program, its install location, the
folder usage matching uses, the main exes and the rule that picked them, and every related hit
with what `match_usage` did with it (`usage::matching::explain_match`). Runs were elevated.

Sources on this machine: BAM 152 hits (SIDs `…-1001` = Jimmy 51 values, `…-1002` = Jobb 94,
`S-1-5-18` 4, three `S-1-5-90-*` 1 each; all SIDs are read), Prefetch 261 hits (2 of 261 files
unreadable), UserAssist 309, file times 139.

## Root causes

1. **Matching only used `InstallLocation`.** Many entries have none and name their folder only
   through the uninstaller or the display icon (Steam: `C:\Program Files (x86)\Steam\uninstall.exe`;
   Netbird, Bitwarden, MEGAsync, JetBrains Toolbox). Their hits fell to `no-candidate`, or were
   claimed by file name by another program: every `steam.exe` hit went to the Steam game
   "Dota Underlords", whose uninstall string runs `steam.exe` (as not-main, but its UserAssist
   launches then dated *Dota Underlords*). The leftover scanner already had the right rule
   (`ProgramHint::effective_install_dir`); usage matching now uses it too
   (`usage::matching::usage_install_dir`), for main exes, root-exe listing and hit assignment.
2. **MSI entries with no path at all.** Google Chrome is installed from the enterprise MSI: its
   entry has only `MsiExec.exe /X{E9AB118B-…}`, no `InstallLocation`, `DisplayIcon` or exe
   uninstaller, so nothing could ever match. The only link is the Start Menu shortcut
   `Google Chrome.lnk` → `C:\Program Files\Google\Chrome\Application\chrome.exe`. A program with no
   other folder now uses the folder of a Start Menu shortcut named exactly like it (not in a
   shared root or under `%SystemRoot%`). Only MSI entries get this rule: a first version without
   that limit gave the browser web apps "YouTube", "Messenger" and "WorkoutBuddy" (non-MSI
   entries with a same-named shortcut to `chrome_proxy.exe`/`msedge_proxy.exe`) the browser's
   folder and with it the browser's usage.
3. **Squirrel version folders.** Toggl Track's main exe is the root stub `TogglTrack.exe`
   (largest root exe) while BAM records `app-10.18.1\TogglTrack.exe`, which was "not main".
   Discord's main exe found from the shortcut arguments is pinned to one `app-1.0.9253` folder
   and goes stale on the next update. An `app-<version>` folder directly under the install
   folder is now ignored when comparing an exe with the main exes.
4. **Installers picked as main exe.** Battlefield 6's largest root exe is
   `EAAntiCheat.Installer.exe`, so the BAM hit on `bf6.exe` was "not main". The largest-root-exe
   rule now skips stems containing `installer` or `setup` (they are not excluded from usage).

Suspects ruled out: BAM `\Device\HarddiskVolumeN` paths on other volumes are translated
correctly (`H:\SteamLibrary\steamapps\common\Battlefield 6\bf6.exe` matched its entry); BAM SIDs
other than the current user's are read.

Not fixed here:

- **Visual Studio Professional 2026** stays "never": its hits are `LINK.EXE`, `CVTRES.EXE`,
  `VCTIP.EXE` under `VC\Tools\MSVC\…\bin\Hostx64\x64` (Prefetch, today), all `excluded:NotMain`,
  and file times (`devenv.exe` 2026-10-17) only date a program without any other hit. This is
  the "subcomponents count" rule of plan 4 Task 2.
- Discord under the other user (`C:\Users\Jobb\AppData\Local\Discord`) has no trace at all; its
  "never" is real. Steam games whose folders are gone (Portal 2, Dyson Sphere Program,
  StarCraft II, Battlefield 2042 on an unmounted `E:`) are rightly never seen.
- Entries whose uninstaller is not named like one and lives in a differently named folder
  (Total Commander `totalcmd\tcunin64.exe`, EA app `EAUninstall.exe`, Rustup
  `.cargo\bin\rustup.exe`, OneDrive's versioned `OneDriveSetup.exe`) still have no folder.
- (Fixed in the review round below.) A program without a folder whose uninstaller is named
  `setup.exe` (ASUS Motherboard) claimed every unclaimed `setup.exe` hit by name.

## Counts

| | never | visible programs |
|---|---|---|
| Before (plan baseline) | 173 | 286 |
| Before (this run) | 174 | 285 |
| After the fixes | 157 | 285 |
| After review fixes | 161 | 285 |

No longer "never": Google Chrome, Steam, Battlefield 6, Toggl Track, Netbird, Bitwarden,
MEGAsync, JetBrains Toolbox, Paint.NET, Node.js, WireGuard, Meta Quest Remote Desktop, Fraps,
L-Connect 3, MSI Afterburner, Microsoft SQL Server 2019/2022, RSI Launcher, Synology Assistant.
Newly "never": Dota Underlords, which had been dated by Steam's own launches (UserAssist
`steam.exe`, claimed by its uninstaller's file name; they now go to Steam by location), and the
Chrome web app WorkoutBuddy (not traced; presumably the same with `chrome.exe`).

Review fixes (same day): a folder derived from a nested uninstaller widens to the display
icon's folder (`Application\141.x\Installer\setup.exe` under `Application\chrome.exe`); the
display icon claims its exe by name whenever it lies outside the folder, but never from a
rejected folder (shared root, `%SystemRoot%`, temp/installer caches, user folders); generic
setup/uninstaller names (`setup.exe`, `unins000.exe`, ...) never claim by name. Four programs
became "never" through the last two rules (the first three plus the x64 one checked with
`dump usage`, the x86 one has the same layout): ASUS Motherboard
(display icon `InstallShield Installation Information\{…}\Setup.exe`), SQL Server Management
Studio 18.12.1 (`Package Cache\{…}\SSMS-Setup-ENU.exe`) and the two Visual C++ v14
Redistributables (`Package Cache\{…}\VC_redist.x64.exe`). Their only hits are runs of their
own cached setup (an upgrade on 2026-10-17), which is not use.

## Elevated output before the fixes

### dump usage Chrome

```
warning: Prefetch: 2 av 261 filer kunde inte läsas
hits per source: [("Bam", 152), ("FileTime", 139), ("Prefetch", 261), ("UserAssist", 309)]
=== Google Chrome [LocalMachine\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{E9AB118B-2341-3DD2-BD45-27B55F5F3802}]
install_location: None
display_icon:     None
uninstall_string: Some("MsiExec.exe /X{E9AB118B-2341-3DD2-BD45-27B55F5F3802}")
effective dir:    None
main exes (None): []
exe names under install dirs: 0
--- 0 related hits
```

### dump usage Steam

```
warning: Prefetch: 2 av 261 filer kunde inte läsas
hits per source: [("Bam", 152), ("FileTime", 139), ("Prefetch", 261), ("UserAssist", 309)]
=== Steam [LocalMachine\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Steam]
install_location: None
display_icon:     Some("C:\\Program Files (x86)\\Steam\\uninstall.exe")
uninstall_string: Some("C:\\Program Files (x86)\\Steam\\uninstall.exe")
effective dir:    Some("C:\\Program Files (x86)\\Steam")
main exes (None): []
exe names under install dirs: 28
  Bam 2026-10-18 13:05 C:\Program Files (x86)\Steam\bin\cef\cef.win64\steamwebhelper.exe -> no-candidate
  Bam 2026-10-18 10:09 H:\SteamLibrary\steamapps\common\Battlefield 6\bf6.exe -> assigned-to:Battlefield™ 6
  Bam 2026-10-18 09:55 C:\Program Files (x86)\Steam\steam.exe -> assigned-to:Dota Underlords
  UserAssist 2025-12-16 16:50 C:\Program Files (x86)\Steam\steam.exe x0 -> assigned-to:Dota Underlords
  UserAssist 2025-12-07 12:03 C:\Program Files (x86)\Steam\steam.exe x0 -> assigned-to:Dota Underlords
  UserAssist 2026-06-25 13:58 C:\Program Files (x86)\Steam\steam.exe x0 -> assigned-to:Dota Underlords
  UserAssist 2025-12-19 11:18 C:\Program Files (x86)\Steam\steam.exe x0 -> assigned-to:Dota Underlords
  Prefetch 2026-10-18 09:55 \PROGRAM FILES (X86)\STEAM\STEAM.EXE x10 -> assigned-to:Dota Underlords
  Prefetch 2026-10-18 09:54 \PROGRAM FILES (X86)\STEAM\BIN\CEF\CEF.WIN64\STEAMWEBHELPER.EXE x22 -> drive-less-unmatched
  Prefetch 2026-10-18 13:03 \PROGRAM FILES (X86)\STEAM\BIN\CEF\CEF.WIN64\STEAMWEBHELPER.EXE x265 -> drive-less-unmatched
  FileTime 2026-03-08 01:34 C:\Program Files (x86)\Steam\steamapps\common\DisplayFusion\DisplayFusionSetup.exe -> assigned-to:DisplayFusion
--- 11 related hits
```

### dump usage Discord

```
warning: Prefetch: 2 av 261 filer kunde inte läsas
hits per source: [("Bam", 152), ("FileTime", 139), ("Prefetch", 261), ("UserAssist", 309)]
=== Discord [CurrentUser\Software\Microsoft\Windows\CurrentVersion\Uninstall\Discord]
install_location: Some("C:\\Users\\Jimmy\\AppData\\Local\\Discord")
display_icon:     None
uninstall_string: Some("C:\\Users\\Jimmy\\AppData\\Local\\Discord\\Update.exe --uninstall")
effective dir:    Some("C:\\Users\\Jimmy\\AppData\\Local\\Discord")
main exes (Shortcut): ["C:\\Users\\Jimmy\\AppData\\Local\\Discord\\app-1.0.9253\\Discord.exe"]
exe names under install dirs: 2
  UserAssist 2026-01-01 03:04 C:\Users\Jimmy\AppData\Local\Discord\Update.exe x0 -> counted
  UserAssist 2025-06-14 17:53 C:\Users\Jimmy\AppData\Local\Discord\Update.exe x0 -> counted
  FileTime 2026-08-13 23:37 C:\Users\Jimmy\AppData\Local\Discord\app-1.0.9253\Discord.exe -> counted:main
--- 3 related hits
=== Discord [Users\S-1-5-21-649775681-1884614592-2659330354-1002\Software\Microsoft\Windows\CurrentVersion\Uninstall\Discord]
install_location: Some("C:\\Users\\Jobb\\AppData\\Local\\Discord")
display_icon:     None
uninstall_string: Some("C:\\Users\\Jobb\\AppData\\Local\\Discord\\Update.exe --uninstall")
effective dir:    Some("C:\\Users\\Jobb\\AppData\\Local\\Discord")
main exes (None): []
exe names under install dirs: 2
  UserAssist 2026-01-01 03:04 C:\Users\Jimmy\AppData\Local\Discord\Update.exe x0 -> assigned-to:Discord
  UserAssist 2025-06-14 17:53 C:\Users\Jimmy\AppData\Local\Discord\Update.exe x0 -> assigned-to:Discord
  FileTime 2026-08-13 23:37 C:\Users\Jimmy\AppData\Local\Discord\app-1.0.9253\Discord.exe -> assigned-to:Discord
--- 3 related hits
```

### dump usage Visual Studio Professional

```
warning: Prefetch: 2 av 261 filer kunde inte läsas
hits per source: [("Bam", 152), ("FileTime", 139), ("Prefetch", 261), ("UserAssist", 309)]
=== Visual Studio Professional 2026 [LocalMachine\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\261281d8]
install_location: Some("C:\\Program Files\\Microsoft Visual Studio\\18\\Professional")
display_icon:     Some("C:\\Program Files\\Microsoft Visual Studio\\18\\Professional\\Common7\\IDE\\devenv.exe")
uninstall_string: Some("\"C:\\Program Files (x86)\\Microsoft Visual Studio\\Installer\\setup.exe\" uninstall --installPath \"C:\\Program Files\\Microsoft Visual Studio\\18\\Professional\"")
effective dir:    Some("C:\\Program Files\\Microsoft Visual Studio\\18\\Professional")
main exes (Shortcut): ["C:\\Program Files\\Microsoft Visual Studio\\18\\Professional\\Common7\\IDE\\Blend.exe", "C:\\Program Files\\Microsoft Visual Studio\\18\\Professional\\Common7\\IDE\\devenv.exe"]
exe names under install dirs: 62
  Bam 2026-10-18 00:46 C:\Windows\Microsoft.NET\Framework\v4.0.30319\MSBuild.exe -> no-candidate
  Prefetch 2026-10-18 11:56 \PROGRAM FILES\MICROSOFT VISUAL STUDIO\18\PROFESSIONAL\VC\TOOLS\MSVC\14.50.35717\BIN\HOSTX64\X64\CVTRES.EXE x32 -> excluded:NotMain
  Prefetch 2026-10-18 13:47 \PROGRAM FILES\MICROSOFT VISUAL STUDIO\18\PROFESSIONAL\VC\TOOLS\MSVC\14.50.35717\BIN\HOSTX64\X64\LINK.EXE x99 -> excluded:NotMain
  Prefetch 2026-10-18 00:41 \WINDOWS\WINSXS\WOW64_MSBUILD_B03F5F7F11D50A3A_4.0.15920.100_NONE_FCC3A3BB98E3C413\MSBUILD.EXE x131 -> drive-less-unmatched
  Prefetch 2026-10-18 13:47 \PROGRAM FILES\MICROSOFT VISUAL STUDIO\18\PROFESSIONAL\VC\TOOLS\MSVC\14.50.35717\BIN\HOSTX64\X64\VCTIP.EXE x2 -> excluded:NotMain
  FileTime 2026-10-17 21:48 C:\Program Files\Microsoft Visual Studio\18\Professional\Common7\IDE\Blend.exe -> counted:main
  FileTime 2026-10-17 21:48 C:\Program Files\Microsoft Visual Studio\18\Professional\Common7\IDE\devenv.exe -> counted:main
--- 7 related hits
=== JetBrains ReSharper in Visual Studio Professional 2022 [CurrentUser\Software\Microsoft\Windows\CurrentVersion\Uninstall\{b32acd63-30b2-5789-9b9d-66694b8fe50e}]
install_location: Some("C:\\Users\\Jimmy\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d")
display_icon:     Some("C:\\Users\\Jimmy\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d\\ReSharperPlatformVs.ico")
uninstall_string: Some("\"C:\\Users\\Jimmy\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d\\JetBrains.Platform.Installer.exe\" /HostsToRemove=ReSharperPlatformVs17_dc284e0d")
effective dir:    Some("C:\\Users\\Jimmy\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d")
main exes (LargestRootExe): ["C:\\Users\\Jimmy\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d\\JetBrains.Platform.Installer.exe"]
exe names under install dirs: 41
  Prefetch 2026-10-18 13:35 \PROGRAM FILES\GIT\MINGW64\BIN\GIT-CREDENTIAL-MANAGER.EXE x43 -> assigned-to:Git
  FileTime 2023-09-09 11:42 C:\Users\Jimmy\AppData\Local\JetBrains\Installations\ReSharperPlatformVs17_dc284e0d\JetBrains.Platform.Installer.exe -> counted:main
  FileTime 2026-01-29 09:05 C:\Users\Jobb\AppData\Local\JetBrains\Installations\ReSharperPlatformVs18_261281d8\JetBrains.Platform.Installer.exe -> assigned-to:JetBrains ReSharper in Visual Studio Professional 2026
  FileTime 2025-02-25 12:18 C:\Users\Jobb\AppData\Local\JetBrains\Installations\ReSharperPlatformVs17_dc284e0d\JetBrains.Platform.Installer.exe -> assigned-to:JetBrains ReSharper in Visual Studio Professional 2022
--- 4 related hits
=== JetBrains ReSharper in Visual Studio Professional 2019 [Users\S-1-5-21-649775681-1884614592-2659330354-1002\Software\Microsoft\Windows\CurrentVersion\Uninstall\{3c29bc09-d9e4-5dcd-84e3-d1b53445357e}]
install_location: None
display_icon:     Some("C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs16_60ec3f91\\ReSharperPlatformVs.ico")
uninstall_string: Some("\"C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs16_60ec3f91\\JetBrains.Platform.Installer.exe\" /HostsToRemove=ReSharperPlatformVs16_60ec3f91")
effective dir:    None
main exes (None): []
exe names under install dirs: 0
--- 0 related hits
=== JetBrains ReSharper in Visual Studio Professional 2026 [Users\S-1-5-21-649775681-1884614592-2659330354-1002\Software\Microsoft\Windows\CurrentVersion\Uninstall\{a911525c-d313-5032-932b-2f0c627daeb1}]
install_location: Some("C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs18_261281d8")
display_icon:     Some("C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs18_261281d8\\ReSharperPlatformVs.ico")
uninstall_string: Some("\"C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs18_261281d8\\JetBrains.Platform.Installer.exe\" /HostsToRemove=ReSharperPlatformVs18_261281d8")
effective dir:    Some("C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs18_261281d8")
main exes (LargestRootExe): ["C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs18_261281d8\\JetBrains.Platform.Installer.exe"]
exe names under install dirs: 72
  FileTime 2023-09-09 11:42 C:\Users\Jimmy\AppData\Local\JetBrains\Installations\ReSharperPlatformVs17_dc284e0d\JetBrains.Platform.Installer.exe -> assigned-to:JetBrains ReSharper in Visual Studio Professional 2022
  FileTime 2026-01-29 09:05 C:\Users\Jobb\AppData\Local\JetBrains\Installations\ReSharperPlatformVs18_261281d8\JetBrains.Platform.Installer.exe -> counted:main
  FileTime 2025-02-25 12:18 C:\Users\Jobb\AppData\Local\JetBrains\Installations\ReSharperPlatformVs17_dc284e0d\JetBrains.Platform.Installer.exe -> assigned-to:JetBrains ReSharper in Visual Studio Professional 2022
--- 3 related hits
=== JetBrains ReSharper in Visual Studio Professional 2022 [Users\S-1-5-21-649775681-1884614592-2659330354-1002\Software\Microsoft\Windows\CurrentVersion\Uninstall\{b32acd63-30b2-5789-9b9d-66694b8fe50e}]
install_location: Some("C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d")
display_icon:     Some("C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d\\ReSharperPlatformVs.ico")
uninstall_string: Some("\"C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d\\JetBrains.Platform.Installer.exe\" /HostsToRemove=ReSharperPlatformVs17_dc284e0d")
effective dir:    Some("C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d")
main exes (LargestRootExe): ["C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d\\JetBrains.Platform.Installer.exe"]
exe names under install dirs: 75
  Prefetch 2026-10-18 13:35 \PROGRAM FILES\GIT\MINGW64\BIN\GIT-CREDENTIAL-MANAGER.EXE x43 -> assigned-to:Git
  FileTime 2023-09-09 11:42 C:\Users\Jimmy\AppData\Local\JetBrains\Installations\ReSharperPlatformVs17_dc284e0d\JetBrains.Platform.Installer.exe -> assigned-to:JetBrains ReSharper in Visual Studio Professional 2022
  FileTime 2026-01-29 09:05 C:\Users\Jobb\AppData\Local\JetBrains\Installations\ReSharperPlatformVs18_261281d8\JetBrains.Platform.Installer.exe -> assigned-to:JetBrains ReSharper in Visual Studio Professional 2026
  FileTime 2025-02-25 12:18 C:\Users\Jobb\AppData\Local\JetBrains\Installations\ReSharperPlatformVs17_dc284e0d\JetBrains.Platform.Installer.exe -> counted:main
--- 4 related hits
```

### dump usage Battlefield

```
warning: Prefetch: 2 av 261 filer kunde inte läsas
hits per source: [("Bam", 152), ("FileTime", 139), ("Prefetch", 261), ("UserAssist", 309)]
=== Battlefield™ 6 [LocalMachine\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Steam App 2807960]
install_location: Some("H:\\SteamLibrary\\steamapps\\common\\Battlefield 6")
display_icon:     Some("C:\\Program Files (x86)\\Steam\\steam\\games\\1e5b399eb3bcb274ddec8a4bf2743c6e725aee79.ico")
uninstall_string: Some("\"C:\\Program Files (x86)\\Steam\\steam.exe\" steam://uninstall/2807960")
effective dir:    Some("H:\\SteamLibrary\\steamapps\\common\\Battlefield 6")
main exes (LargestRootExe): ["H:\\SteamLibrary\\steamapps\\common\\Battlefield 6\\EAAntiCheat.Installer.exe"]
exe names under install dirs: 3
  Bam 2026-10-18 09:59 H:\SteamLibrary\steamapps\common\Battlefield 6\EAAntiCheat.GameServiceLauncher.exe -> excluded:NamePattern
  Bam 2026-10-18 10:09 H:\SteamLibrary\steamapps\common\Battlefield 6\bf6.exe -> excluded:NotMain
  FileTime 2026-10-07 19:55 H:\SteamLibrary\steamapps\common\Battlefield 6\EAAntiCheat.Installer.exe -> counted:main
--- 3 related hits
=== Battlefield™ 2042 [LocalMachine\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\{45e281f3-1414-47ea-bb64-4f50d50121f3}]
install_location: Some("E:\\EA Games\\Battlefield 2042")
display_icon:     Some("E:\\EA Games\\Battlefield 2042\\BF2042.exe")
uninstall_string: Some("\"C:\\Program Files\\Common Files\\EAInstaller\\Battlefield 2042\\Cleanup.exe\" uninstall_game -autologging")
effective dir:    Some("E:\\EA Games\\Battlefield 2042")
main exes (DisplayIcon): ["E:\\EA Games\\Battlefield 2042\\BF2042.exe"]
exe names under install dirs: 0
--- 0 related hits
```

## Elevated output after the fixes (excerpts)

### dump usage Chrome

```
=== Google Chrome [LocalMachine\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{E9AB118B-2341-3DD2-BD45-27B55F5F3802}]
install_location: None
display_icon:     None
uninstall_string: Some("MsiExec.exe /X{E9AB118B-2341-3DD2-BD45-27B55F5F3802}")
effective dir:    Some("C:\\Program Files\\Google\\Chrome\\Application")
main exes (Shortcut): ["C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe", "C:\\Program Files\\Google\\Chrome\\Application\\chrome_proxy.exe"]
exe names under install dirs: 10
  Bam 2026-10-17 21:22 C:\Program Files\Google\Chrome\Application\chrome.exe -> counted:main
  Bam 2026-10-17 21:06 C:\Program Files (x86)\Microsoft Visual Studio\Installer\setup.exe -> assigned-to:Microsoft Visual Studio Installer
  Bam 2026-10-18 09:43 C:\Program Files (x86)\Microsoft Visual Studio\Installer.d454a31ec5884299aebc72b9310115fb\setup.exe -> assigned-to:ASUS Motherboard
  Bam 2026-10-16 08:50 C:\Program Files\Google\Chrome\Application\chrome.exe -> counted:main
  UserAssist 2026-01-07 18:47 C:\Program Files\Google\Chrome\Application\chrome.exe x2 -> counted:main
  UserAssist 2026-10-11 10:09 C:\Program Files\Google\Chrome\Application\chrome.exe x2 -> counted:main
  UserAssist 2025-12-19 13:20 C:\Program Files (x86)\Microsoft Visual Studio\Installer\setup.exe x0 -> assigned-to:Microsoft Visual Studio Installer
  UserAssist 2026-08-04 06:11 C:\Program Files\Google\Chrome\Application\chrome.exe x0 -> counted:main
  Prefetch 2026-10-17 21:21 \PROGRAM FILES\GOOGLE\CHROME\APPLICATION\CHROME.EXE x31 -> counted:main
  Prefetch 2026-10-17 21:22 \PROGRAM FILES\GOOGLE\CHROME\APPLICATION\CHROME.EXE x389 -> counted:main
  Prefetch 2026-10-17 21:21 \PROGRAM FILES\GOOGLE\CHROME\APPLICATION\CHROME.EXE x101 -> counted:main
  Prefetch 2026-10-17 21:05 \PROGRAM FILES (X86)\MICROSOFT VISUAL STUDIO\INSTALLER\SETUP.EXE x4 -> assigned-to:Microsoft Visual Studio Installer
  Prefetch 2026-10-17 21:05 \PROGRAM FILES (X86)\MICROSOFT VISUAL STUDIO\INSTALLER.D454A31EC5884299AEBC72B9310115FB\SETUP.EXE x1 -> assigned-to:ASUS Motherboard
  FileTime 2026-10-17 21:05 C:\Program Files (x86)\Microsoft Visual Studio\Installer\setup.exe -> assigned-to:Microsoft Visual Studio Installer
  FileTime 2026-10-13 02:47 C:\Program Files\Google\Chrome\Application\chrome.exe -> superseded
  FileTime 2026-10-13 02:47 C:\Program Files\Google\Chrome\Application\chrome_proxy.exe -> filetime-ignored
--- 16 related hits
```

### dump usage Steam

```
=== Steam [LocalMachine\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Steam]
install_location: None
display_icon:     Some("C:\\Program Files (x86)\\Steam\\uninstall.exe")
uninstall_string: Some("C:\\Program Files (x86)\\Steam\\uninstall.exe")
effective dir:    Some("C:\\Program Files (x86)\\Steam")
main exes (Shortcut): ["C:\\Program Files (x86)\\Steam\\steam.exe"]
exe names under install dirs: 28
  Bam 2026-10-18 13:05 C:\Program Files (x86)\Steam\bin\cef\cef.win64\steamwebhelper.exe -> excluded:NamePattern
  Bam 2026-10-18 10:09 H:\SteamLibrary\steamapps\common\Battlefield 6\bf6.exe -> assigned-to:Battlefield™ 6
  Bam 2026-10-18 09:55 C:\Program Files (x86)\Steam\steam.exe -> counted:main
  UserAssist 2025-12-16 16:50 C:\Program Files (x86)\Steam\steam.exe x0 -> counted:main
  UserAssist 2025-12-07 12:03 C:\Program Files (x86)\Steam\steam.exe x0 -> counted:main
  UserAssist 2026-06-25 13:58 C:\Program Files (x86)\Steam\steam.exe x0 -> counted:main
  UserAssist 2025-12-19 11:18 C:\Program Files (x86)\Steam\steam.exe x0 -> counted:main
  Prefetch 2026-10-18 09:55 \PROGRAM FILES (X86)\STEAM\STEAM.EXE x10 -> counted:main
  Prefetch 2026-10-18 09:54 \PROGRAM FILES (X86)\STEAM\BIN\CEF\CEF.WIN64\STEAMWEBHELPER.EXE x22 -> excluded:NamePattern
  Prefetch 2026-10-18 13:03 \PROGRAM FILES (X86)\STEAM\BIN\CEF\CEF.WIN64\STEAMWEBHELPER.EXE x265 -> excluded:NamePattern
  FileTime 2026-03-08 01:34 C:\Program Files (x86)\Steam\steamapps\common\DisplayFusion\DisplayFusionLauncher.exe -> assigned-to:DisplayFusion
  FileTime 2026-10-15 08:12 H:\SteamLibrary\steamapps\common\Battlefield 6\bf6.exe -> assigned-to:Battlefield™ 6
  FileTime 2026-10-13 19:50 C:\Program Files (x86)\Steam\steam.exe -> superseded
--- 13 related hits
```

### dump usage Battlefield

```
=== Battlefield™ 6 [LocalMachine\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Steam App 2807960]
install_location: Some("H:\\SteamLibrary\\steamapps\\common\\Battlefield 6")
display_icon:     Some("C:\\Program Files (x86)\\Steam\\steam\\games\\1e5b399eb3bcb274ddec8a4bf2743c6e725aee79.ico")
uninstall_string: Some("\"C:\\Program Files (x86)\\Steam\\steam.exe\" steam://uninstall/2807960")
effective dir:    Some("H:\\SteamLibrary\\steamapps\\common\\Battlefield 6")
main exes (LargestRootExe): ["H:\\SteamLibrary\\steamapps\\common\\Battlefield 6\\bf6.exe"]
exe names under install dirs: 3
  Bam 2026-10-18 09:59 H:\SteamLibrary\steamapps\common\Battlefield 6\EAAntiCheat.GameServiceLauncher.exe -> excluded:NamePattern
  Bam 2026-10-18 10:09 H:\SteamLibrary\steamapps\common\Battlefield 6\bf6.exe -> counted:main
  FileTime 2026-10-15 08:12 H:\SteamLibrary\steamapps\common\Battlefield 6\bf6.exe -> superseded
--- 3 related hits
=== Battlefield™ 2042 [LocalMachine\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\{45e281f3-1414-47ea-bb64-4f50d50121f3}]
install_location: Some("E:\\EA Games\\Battlefield 2042")
display_icon:     Some("E:\\EA Games\\Battlefield 2042\\BF2042.exe")
uninstall_string: Some("\"C:\\Program Files\\Common Files\\EAInstaller\\Battlefield 2042\\Cleanup.exe\" uninstall_game -autologging")
effective dir:    Some("E:\\EA Games\\Battlefield 2042")
main exes (DisplayIcon): ["E:\\EA Games\\Battlefield 2042\\BF2042.exe"]
exe names under install dirs: 0
--- 0 related hits
```

### dump usage Toggl

```
=== Toggl Track [Users\S-1-5-21-649775681-1884614592-2659330354-1002\Software\Microsoft\Windows\CurrentVersion\Uninstall\TogglDesktop]
install_location: Some("C:\\Users\\Jobb\\AppData\\Local\\TogglDesktop")
display_icon:     Some("C:\\Users\\Jobb\\AppData\\Local\\TogglDesktop\\TogglDesktop.exe")
uninstall_string: Some("\"C:\\Users\\Jobb\\AppData\\Local\\TogglDesktop\\uninstall.exe\"")
effective dir:    Some("C:\\Users\\Jobb\\AppData\\Local\\TogglDesktop")
main exes (DisplayIcon): ["C:\\Users\\Jobb\\AppData\\Local\\TogglDesktop\\TogglDesktop.exe"]
exe names under install dirs: 2
  FileTime 2023-06-22 11:49 C:\Users\Jobb\AppData\Local\TogglDesktop\TogglDesktop.exe -> counted:main
--- 1 related hits
=== Toggl Track [Users\S-1-5-21-649775681-1884614592-2659330354-1002\Software\Microsoft\Windows\CurrentVersion\Uninstall\TogglTrack]
install_location: Some("C:\\Users\\Jobb\\AppData\\Local\\TogglTrack")
display_icon:     Some("C:\\Users\\Jobb\\AppData\\Local\\TogglTrack\\app.ico")
uninstall_string: Some("\"C:\\Users\\Jobb\\AppData\\Local\\TogglTrack\\Update.exe\" --uninstall")
effective dir:    Some("C:\\Users\\Jobb\\AppData\\Local\\TogglTrack")
main exes (LargestRootExe): ["C:\\Users\\Jobb\\AppData\\Local\\TogglTrack\\TogglTrack.exe"]
exe names under install dirs: 2
  Bam 2026-10-18 13:57 C:\Users\Jobb\AppData\Local\TogglTrack\app-10.18.1\TogglTrack.exe -> counted:main
  UserAssist 2026-01-01 03:04 C:\Users\Jimmy\AppData\Local\Discord\Update.exe x0 -> assigned-to:Discord
  UserAssist 2025-06-14 17:53 C:\Users\Jimmy\AppData\Local\Discord\Update.exe x0 -> assigned-to:Discord
  FileTime 2026-08-27 00:14 C:\Users\Jobb\AppData\Local\TogglTrack\TogglTrack.exe -> filetime-ignored
--- 4 related hits
```

### dump usage Visual Studio Professional

```
=== Visual Studio Professional 2026 [LocalMachine\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\261281d8]
install_location: Some("C:\\Program Files\\Microsoft Visual Studio\\18\\Professional")
display_icon:     Some("C:\\Program Files\\Microsoft Visual Studio\\18\\Professional\\Common7\\IDE\\devenv.exe")
uninstall_string: Some("\"C:\\Program Files (x86)\\Microsoft Visual Studio\\Installer\\setup.exe\" uninstall --installPath \"C:\\Program Files\\Microsoft Visual Studio\\18\\Professional\"")
effective dir:    Some("C:\\Program Files\\Microsoft Visual Studio\\18\\Professional")
main exes (Shortcut): ["C:\\Program Files\\Microsoft Visual Studio\\18\\Professional\\Common7\\IDE\\Blend.exe", "C:\\Program Files\\Microsoft Visual Studio\\18\\Professional\\Common7\\IDE\\devenv.exe"]
exe names under install dirs: 62
  Bam 2026-10-18 00:46 C:\Windows\Microsoft.NET\Framework\v4.0.30319\MSBuild.exe -> no-candidate
  Prefetch 2026-10-18 11:56 \PROGRAM FILES\MICROSOFT VISUAL STUDIO\18\PROFESSIONAL\VC\TOOLS\MSVC\14.50.35717\BIN\HOSTX64\X64\CVTRES.EXE x32 -> excluded:NotMain
  Prefetch 2026-10-18 13:56 \PROGRAM FILES\MICROSOFT VISUAL STUDIO\18\PROFESSIONAL\VC\TOOLS\MSVC\14.50.35717\BIN\HOSTX64\X64\LINK.EXE x101 -> excluded:NotMain
  Prefetch 2026-10-18 00:41 \WINDOWS\WINSXS\WOW64_MSBUILD_B03F5F7F11D50A3A_4.0.15920.100_NONE_FCC3A3BB98E3C413\MSBUILD.EXE x131 -> drive-less-unmatched
  Prefetch 2026-10-18 13:47 \PROGRAM FILES\MICROSOFT VISUAL STUDIO\18\PROFESSIONAL\VC\TOOLS\MSVC\14.50.35717\BIN\HOSTX64\X64\VCTIP.EXE x2 -> excluded:NotMain
  FileTime 2026-10-17 21:48 C:\Program Files\Microsoft Visual Studio\18\Professional\Common7\IDE\Blend.exe -> filetime-ignored
  FileTime 2026-10-17 21:48 C:\Program Files\Microsoft Visual Studio\18\Professional\Common7\IDE\devenv.exe -> filetime-ignored
--- 7 related hits
=== JetBrains ReSharper in Visual Studio Professional 2022 [CurrentUser\Software\Microsoft\Windows\CurrentVersion\Uninstall\{b32acd63-30b2-5789-9b9d-66694b8fe50e}]
install_location: Some("C:\\Users\\Jimmy\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d")
display_icon:     Some("C:\\Users\\Jimmy\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d\\ReSharperPlatformVs.ico")
uninstall_string: Some("\"C:\\Users\\Jimmy\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d\\JetBrains.Platform.Installer.exe\" /HostsToRemove=ReSharperPlatformVs17_dc284e0d")
effective dir:    Some("C:\\Users\\Jimmy\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d")
main exes (LargestRootExe): ["C:\\Users\\Jimmy\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d\\JetLauncher64c.exe"]
exe names under install dirs: 41
  Prefetch 2026-10-18 13:35 \PROGRAM FILES\GIT\MINGW64\BIN\GIT-CREDENTIAL-MANAGER.EXE x43 -> assigned-to:Git
  FileTime 2023-09-09 11:42 C:\Users\Jimmy\AppData\Local\JetBrains\Installations\ReSharperPlatformVs17_dc284e0d\JetLauncher64c.exe -> counted:main
  FileTime 2025-02-25 12:18 C:\Users\Jobb\AppData\Local\JetBrains\Installations\ReSharperPlatformVs17_dc284e0d\JetLauncher64c.exe -> assigned-to:JetBrains ReSharper in Visual Studio Professional 2022
--- 3 related hits
=== JetBrains ReSharper in Visual Studio Professional 2019 [Users\S-1-5-21-649775681-1884614592-2659330354-1002\Software\Microsoft\Windows\CurrentVersion\Uninstall\{3c29bc09-d9e4-5dcd-84e3-d1b53445357e}]
install_location: None
display_icon:     Some("C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs16_60ec3f91\\ReSharperPlatformVs.ico")
uninstall_string: Some("\"C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs16_60ec3f91\\JetBrains.Platform.Installer.exe\" /HostsToRemove=ReSharperPlatformVs16_60ec3f91")
effective dir:    None
main exes (None): []
exe names under install dirs: 0
--- 0 related hits
=== JetBrains ReSharper in Visual Studio Professional 2026 [Users\S-1-5-21-649775681-1884614592-2659330354-1002\Software\Microsoft\Windows\CurrentVersion\Uninstall\{a911525c-d313-5032-932b-2f0c627daeb1}]
install_location: Some("C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs18_261281d8")
display_icon:     Some("C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs18_261281d8\\ReSharperPlatformVs.ico")
uninstall_string: Some("\"C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs18_261281d8\\JetBrains.Platform.Installer.exe\" /HostsToRemove=ReSharperPlatformVs18_261281d8")
effective dir:    Some("C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs18_261281d8")
main exes (LargestRootExe): ["C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs18_261281d8\\ReSharper.Backend64c.exe"]
exe names under install dirs: 72
  FileTime 2023-09-09 11:42 C:\Users\Jimmy\AppData\Local\JetBrains\Installations\ReSharperPlatformVs17_dc284e0d\JetLauncher64c.exe -> assigned-to:JetBrains ReSharper in Visual Studio Professional 2022
  FileTime 2026-01-29 09:05 C:\Users\Jobb\AppData\Local\JetBrains\Installations\ReSharperPlatformVs18_261281d8\ReSharper.Backend64c.exe -> counted:main
  FileTime 2025-02-25 12:18 C:\Users\Jobb\AppData\Local\JetBrains\Installations\ReSharperPlatformVs17_dc284e0d\JetLauncher64c.exe -> assigned-to:JetBrains ReSharper in Visual Studio Professional 2022
--- 3 related hits
=== JetBrains ReSharper in Visual Studio Professional 2022 [Users\S-1-5-21-649775681-1884614592-2659330354-1002\Software\Microsoft\Windows\CurrentVersion\Uninstall\{b32acd63-30b2-5789-9b9d-66694b8fe50e}]
install_location: Some("C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d")
display_icon:     Some("C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d\\ReSharperPlatformVs.ico")
uninstall_string: Some("\"C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d\\JetBrains.Platform.Installer.exe\" /HostsToRemove=ReSharperPlatformVs17_dc284e0d")
effective dir:    Some("C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d")
main exes (LargestRootExe): ["C:\\Users\\Jobb\\AppData\\Local\\JetBrains\\Installations\\ReSharperPlatformVs17_dc284e0d\\JetLauncher64c.exe"]
exe names under install dirs: 75
  Prefetch 2026-10-18 13:35 \PROGRAM FILES\GIT\MINGW64\BIN\GIT-CREDENTIAL-MANAGER.EXE x43 -> assigned-to:Git
  FileTime 2023-09-09 11:42 C:\Users\Jimmy\AppData\Local\JetBrains\Installations\ReSharperPlatformVs17_dc284e0d\JetLauncher64c.exe -> assigned-to:JetBrains ReSharper in Visual Studio Professional 2022
  FileTime 2025-02-25 12:18 C:\Users\Jobb\AppData\Local\JetBrains\Installations\ReSharperPlatformVs17_dc284e0d\JetLauncher64c.exe -> counted:main
--- 3 related hits
```

