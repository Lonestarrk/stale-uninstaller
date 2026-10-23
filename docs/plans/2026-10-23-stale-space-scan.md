# "Sök rester" as an automatic space scan — Implementation Plan

**Goal:** "Sök rester" scans the computer on its own, with no name to type. It lists, sorted by size, the folders and files that no installed program owns, plus the big Windows space hogs such as `Windows.old`. Any entry can be opened to go deeper and find a single big file to remove.

**Spec (user decisions, 2026-10-23):**
- "Sök rester" was meant to scan the drives for files that belong to no identifiable program. It lists them with name, location and size. It was never meant to need a search field.
- The user misses seeing `Windows.old` and similar folders that take up a lot of unnecessary space.
  - Show such a folder as one row, like a file.
  - Its contents do not matter.
- Approved scope (playback answered "kör så"):
  - **Places to scan:**
    - the top level of every drive
    - Program Files and Program Files (x86)
    - ProgramData
    - AppData of every account
  - **Windows space hogs** are listed even though Windows owns them: `Windows.old`, `$Windows.~BT`, Windows Update downloads, crash dumps (`MEMORY.DMP`) and old temp. `Windows.old` and its kind get a button that opens Windows' own cleanup, never the quarantine.
  - **Layout** as in the compact lists: click to check, double-click for details, removal to the quarantine.
  - The old manual search may stay as a small link.
- User addition: "att man kan gå djupare på en om man vill för att hitta kanske en stor fil som kan tas bort".
  - Any entry can be opened.
  - Its contents are listed largest first.
  - You can go further down.
  - Single files or folders found there can be removed through the quarantine.

Background: `docs/design/2026-10-18-plan4-design.md` §6 (the junk finder categories, the explanation per entry, the actions through Windows). Folder ownership reuses `ghosts::folders` (`classify`, `InstalledIndex`, keep list).

## Global constraints
- **Removal** goes only through the existing quarantine flow, the same flow the leftovers view uses (`LeftoverSelection` → `request_removal` → `confirm_removal`). Windows-owned entries (`Windows.old`, `$Windows.~BT`, `SoftwareDistribution\Download`, `C:\Windows\Temp`) are never quarantined. They offer "Öppna Windows rensning" instead. That button opens `ms-settings:storagesense` through `explorer.exe` from `proc::system32_tool`, and it is shown with a one-line explanation.
- **Nothing is pre-checked.** Every row is the user's call. Drive-root folders and drill-down entries say "Okänd – avgör själv" when no catalog text applies.
- **Never listed, never offered:**
  - Stale's own folders (`own::OwnPaths`) and the keep list.
  - Drive roots themselves, `Windows`, `Program Files*`, `ProgramData`, `Users`.
  - Profile folders themselves.
  - `$Recycle.Bin`, `System Volume Information`, `Recovery`, `Boot`, `EFI`.
  - `pagefile.sys`, `hiberfil.sys`, `swapfile.sys`, `DumpStack.log*`.
  - Folders that an installed program's `InstallLocation` lies in or below.
  - Reparse points: links are never followed or listed.
- **Drill-down shows everything** below the opened entry. Removal from it is refused for `Windows` and `Program Files*` paths, other users' profile roots, and paths that an installed program owns. Show a one-line reason instead of a checkbox.
- **Timing.** Sizes are measured on a worker. The list appears at once and its sizes fill in as they come (the `Sizes*` event pattern from `scan::run_scan`). The scan must never block the UI.
- **Text and tests.**
  - All text goes in SV and EN.
  - Pure logic stays egui-free and unit-tested with `FakeFs`.
  - No test touches the real disk outside tempdirs.
  - Do not run `cargo fmt`.
  - Update the README Features line in the same commit as the feature.

## Review focus
1. **User data on a second drive.** Example: `D:\Foton`, 200 GB. It appears as "Okänd – avgör själv", never checked. Removing it takes an explicit check plus the confirmation.
2. **A huge drive.** Example: 2 TB with millions of files. The list appears within seconds, sizes stream in, and the UI stays responsive. Leaving the view or re-scanning cancels the old measurement.
3. **Unreadable folders and in-use files.** Example: `Windows.old` partly owned by TrustedInstaller.
   - The size is whatever was readable, marked as "minst" ("at least").
   - Removing an in-use file fails per item in the quarantine summary, as today.
4. **Drill-down into a folder with 100k entries.** Show only the 200 largest children plus a "… N till" ("… N more") line, so the list stays usable.
5. **The same folder from two sources.** Example: an orphan in ProgramData that is also a ghost. It is listed once.

### Task 1: The space scan (core)
**Files:**
- Create `stale-core/src/space/{mod,catalog,scan}.rs`.
- Modify `lib.rs`.

**Interfaces:**
- `pub enum SpaceKind { Orphan(RootKind), DriveRoot, Windows(WinItem) }`
- `pub enum WinItem { WindowsOld, WindowsBt, WindowsWs, UpdateDownload, WindowsTemp, UserTemp, MemoryDump, Minidumps, LiveKernelReports }`
- `pub struct SpaceEntry { pub path: PathBuf, pub is_dir: bool, pub kind: SpaceKind, pub size: Option<u64>, pub partial: bool, pub modified: Option<DateTime<Utc>> }`
- `pub fn removal(kind: &SpaceKind) -> Removal`, where `Removal` is `Quarantine` or `WindowsCleanup`.
  - Every `Windows(_)` item maps to `WindowsCleanup`, except these, which map to `Quarantine`:
    - `UserTemp`: files older than 7 days, as one entry per profile temp folder
    - `MemoryDump`
    - `Minidumps`
    - `LiveKernelReports`
- `pub fn list_entries(fs: &dyn FsReader, drives: &[PathBuf], roots: &FsScanRoots, installed: &InstalledIndex, keep: &KeepList, own: &OwnPaths) -> Vec<SpaceEntry>`
  - Sizes are `None`.
  - Orphans come from `ghosts::folders::orphan_folders`, but without measuring. Add a variant or a flag, so sizes are not measured twice.
  - Drive-root folders and files are those that are not in the never-listed set and not owned. Drive-root files are listed only when they are at least 100 MB.
  - Catalog paths are listed when they exist.
  - Duplicates are removed by `norm`.
- `pub fn measure(fs, entry: &SpaceEntry) -> (Option<u64>, bool /*partial*/)`
- `pub fn fixed_drives() -> Vec<PathBuf>`, live: `GetLogicalDrives` + `GetDriveTypeW == DRIVE_FIXED`.

**Tests:**
- `drive_root_lists_unknown_folders_and_big_files_only`
- `system_and_own_folders_are_never_listed`
- `installed_program_folder_on_a_drive_root_is_not_listed`
- `windows_old_is_listed_as_windows_cleanup`
- `user_temp_and_dumps_are_quarantinable`
- `orphan_seen_twice_is_listed_once`
- `unreadable_subfolder_marks_size_partial`

**Commit:** `feat(core): space scan of unowned folders and Windows space hogs`

### Task 2: Drill-down listing (core)
**Files:**
- Create `stale-core/src/space/browse.rs`.

**Interfaces:**
- `pub struct Child { pub path: PathBuf, pub is_dir: bool, pub size: Option<u64>, pub blocked: Option<BlockReason> }`
- `pub enum BlockReason { System, InstalledProgram, OtherProfileRoot, Own }`
- `pub fn children(fs: &dyn FsReader, dir: &Path, installed: &InstalledIndex, own: &OwnPaths, limit: usize) -> (Vec<Child>, usize /*more*/)`
  - The direct children, largest first.
  - Folders are measured.
  - Links are skipped.
- `pub const BROWSE_LIMIT: usize = 200`

**Tests:**
- `children_sorted_largest_first_with_limit_and_more_count`
- `system_installed_and_own_children_are_blocked`
- `links_are_skipped`

**Commit:** `feat(core): drill-down listing for the space scan`

### Task 3: The "Sök rester" view (GUI)
**Files:**
- `app.rs`: `View::Space`, the state and the removal origin.
- `worker.rs`: `spawn_space_scan`, which streams `SpaceEvent::{Entries, Size(idx, size, partial), Done}` and can be cancelled through a drop flag.
- `ui/space_view.rs` (new): the view, which replaces opening `search_window` from the toolbar button.
- `ui/space_browse.rs` (new): the drill-down window.
- `ui/toolbar.rs`, `strings.rs`, `README.md`.

**View:**
- **Header:** the title "Sök rester", Uppdatera, and a small link "Sök efter ett programs rester…". The link opens today's `search_window`.
- **One compact row per entry, sorted by size (largest first, unknown last):**
  - a checkbox, only for `Quarantine` entries
  - the name, with the location in a weak colour
  - a kind label: "Rest i Program Files" / "Okänd mapp" / "Windows"
  - the size, or "minst X" when partial
  - for `WindowsCleanup` entries: the explanation sentence and the "Öppna Windows rensning" button instead of a checkbox
- **Clicks:** a click toggles the row. A double-click opens `space_browse` for that entry.
- **Footer:** "N markerade · X", plus "Ta bort markerade…". The button opens the same confirmation as the ghosts view, then removes through the leftovers flow with `leftovers_origin = View::Space`.
- **`space_browse` window:**
  - a breadcrumb from the entry down to the current folder; a click goes back up
  - the `children` rows: a checkbox (none on blocked rows, which show their reason as hover text), name, size
  - a double-click on a folder goes into it
  - the "… N till" line
  - "Öppna i Utforskaren" (`explorer.exe /select,` through `proc::system32_tool`)
  - "Ta bort markerade…", which uses the same confirmation and flow
- **Catalog sentences** (SV/EN) for each `WinItem`. For `Windows.old` the sentence names the creation date: "Den förra Windows-installationen, sparad {datum}. Behövs bara om du vill gå tillbaka."

**Tests:**
- `space_rows_sort_by_size_unknown_last`
- `windows_cleanup_rows_have_no_checkbox`
- `removal_from_space_uses_leftovers_flow_with_origin_space`
- `browse_checked_children_become_file_and_dir_leftovers`
- `cancelled_scan_drops_late_sizes`

**Live check:** run `dump space` (add the mode to `examples/dump.rs`) elevated, and record the top 20 entries and the run time in `docs/research/2026-10-23-space-scan.md`.

**Commit:** `feat(gui): Sök rester scans for unowned folders and space hogs`
