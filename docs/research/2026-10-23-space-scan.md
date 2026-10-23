# "Sök rester" as a space scan on a real machine (2026-10-23)

The space scan (`stale_core::space`) lists what no installed program owns: unknown folders and
files of at least 100 MB at the top of each fixed drive, orphaned folders in Program Files,
ProgramData and every account's AppData, and the Windows space hogs (`Windows.old`, the
`$Windows.~*` upgrade folders, update downloads, `%SystemRoot%\Temp`, crash dumps, each profile's
temp files older than a week). `dump space` (in `crates/stale-core/examples/dump.rs`) lists the
entries, measures every one of them and prints the 40 largest with their kind; nothing is
deleted. Runs were elevated.

## Core run (before the GUI)

Machine: the developer's Windows 11 computer with the fixed drives `C:`, `E:`, `F:` and `H:`.

- **Timing.** 448 entries were listed in 0.5 s; measuring all of them took 42 s in total (one
  thread, as `dump space` measures). The GUI shows the list as soon as it is listed and streams
  the sizes from four measuring threads.
- **The largest entries were user data**, not leftovers: user data folders on `H:`, listed
  as unknown drive-root folders ("Okänd – avgör själv"). This is review focus 1 in practice:
  they are never pre-checked and need an explicit check plus the confirmation.
- **`F:\$WINDOWS.~TMP` was 7.7 GB.** A first version listed it as an unknown folder. Upgrade
  folders on any drive are now Windows items (`WinItem::WindowsTmp`, with `$Windows.~BT` and
  `$Windows.~WS`): no check box, only "Öppna Windows rensning", and their insides are blocked in
  the drill-down.
- **Developer tool caches under AppData** were among the large orphans: JetBrains folders,
  `ms-playwright` and VS Code folders. Folders that look like an installed program's (for
  example `Code` next to "Microsoft Visual Studio Code") carry the ghost scan's ambiguity notes,
  which the view shows on the row.

## Run after the fixes

A second elevated `dump space` ran after the review fixes. It listed 445 entries in 0.7 s and finished in 9.5 s, because the file system cache was warm from the first run. `F:\$Windows.~TMP` is now a Windows item.

The 20 largest entries:

| Size | Kind | Path |
|---|---|---|
| 681 279 MB | DriveRoot | `H:\<sync folder>` |
| 290 286 MB | DriveRoot | `H:\<user folder 1>` |
| 93 665 MB | DriveRoot | `H:\<user folder 2>` |
| 80 388 MB | DriveRoot | `H:\<projects>` |
| 30 775 MB | DriveRoot | `F:\<downloads>` |
| 9 627 MB | DriveRoot | `F:\<downloads 2>` |
| 7 717 MB | Windows(WindowsTmp) | `F:\$Windows.~TMP` |
| 3 981 MB | DriveRoot | `C:\AMD` |
| 3 312 MB | DriveRoot | `H:\<CI runner>` |
| 2 661 MB | Orphan(AppData) | `C:\Users\<account 1>\AppData\Local\ms-playwright` |
| 2 019 MB | Orphan(AppData) | `C:\Users\<account 2>\AppData\Local\ms-playwright` |
| 1 668 MB (at least) | Orphan(AppData) | `C:\Users\<account 2>\AppData\Roaming\Code` |
| 1 345 MB | Orphan(AppData) | `C:\Users\<account 1>\AppData\Roaming\RenPy` |
| 1 191 MB | Orphan(AppData) | `C:\Users\<account 2>\AppData\Local\JetBrains\dotMemory` |
| 1 130 MB | Orphan(AppData) | `C:\Users\<account 2>\AppData\Local\cache` |
| 789 MB | Orphan(ProgramFiles) | `C:\Program Files (x86)\Google\GoogleUpdater` |
| 750 MB | Orphan(AppData) | `C:\Users\<account 2>\AppData\Local\AzureFunctionsTools` |
| 688 MB | Orphan(AppData) | `C:\Users\<account 1>\AppData\Local\PD Launcher` |
| 668 MB | Orphan(AppData) | `C:\Users\<account 2>\AppData\Local\JetBrains\Rider2024.3` |
| 640 MB | Orphan(AppData) | `C:\Users\<account 2>\AppData\Local\JetBrains\Rider2023.3` |

What the list shows:
- **Drive-root rows are mostly the user's own data.** That is why nothing is pre-checked.
- **`C:\AMD` is the extracted AMD driver installer.** It is a typical safe-to-remove leftover, but Stale lists it as unknown and leaves the decision to the user.
- **`GoogleUpdater` and `Roaming\Code` carry the ambiguity notes.** `GoogleUpdater` is "sibling-owned", because Chrome is installed. `Roaming\Code` is "word of an installed name".
