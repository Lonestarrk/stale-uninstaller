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
- **The largest entries were user data**, not leftovers: `H:\Dropbox` and `H:\Projects`, listed
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
