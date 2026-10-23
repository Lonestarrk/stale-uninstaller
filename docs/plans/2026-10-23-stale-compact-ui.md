# Compact lists, larger UI and direct ghost removal — Implementation Plan

**Goal:** Make ghosts and leftovers quick to remove:
- one compact row per program
- details in a popup on double-click
- removal straight from the ghosts view
- the whole UI 50 % larger by default

**User decisions (2026-10-23):**
- The UI is too small.
  - Buttons should be larger.
  - The UI should be 50 % larger overall by default.
- Listing every leftover under every program in the ghosts and leftovers views is too much.
  - Show one row per program instead.
  - A double-click on a row opens its leftovers in a popup.
- Removing things is hard. Removal should happen from the ghosts view, with a confirmation, without going through the separate leftovers list first.

The approved proposal:
1. **Scale.**
   - 150 % by default.
   - The settings menu offers 100/125/150/175/200 %, saved in `settings.json`.
   - Ctrl + / Ctrl − also changes it and is saved.
2. **Ghost rows.**
   - One row per ghost: checkbox, name, publisher, assessment, size, last used, Behåll.
   - A row click toggles its checkbox, as in the program list.
3. **Popup.** A double-click opens a popup with the program's leftovers, each with its own checkbox.
4. **Removal.**
   - "Ta bort markerade" is in the ghosts footer.
   - It asks for confirmation first.
   - Everything still goes to the quarantine.
5. **Leftovers view.** It gets the same row-and-popup layout.
6. **Own folders.** Stale's own folders are never listed as a ghost or as leftovers.

## Global constraints
- Every user-visible text goes in `strings.rs` in Swedish and English. Tests never switch the global language.
- Removal still goes only through the quarantine flow: `request_removal` → the optional drive/space window → `confirm_removal`. No new delete path.
- The pre-check rules are unchanged:
  - Low-confidence leftovers are never pre-checked.
  - Only ProbablyUninstalled ghosts start checked.
- Pure selection logic stays in `ghost_sel.rs` and `leftover_sel.rs` (no egui) and is unit-tested.
- Do not run `cargo fmt`.
- Update the README Features list in the same commit as the feature it describes.

## Review focus
1. **Double-click.** The two clicks of a double-click must leave the row's checkbox as it was and must open the popup.
2. **A ghost with no removable leftovers** (for example only usage traces).
   - It can be checked but adds nothing to the removal.
   - The confirmation counts real items only.
   - "Ta bort markerade" is disabled when there are none.
3. **Popup choices.**
   - Edits in a popup survive closing it, and they are what gets removed.
   - Toggling a row's checkbox does not throw away its popup choices.
4. **Saved scale.**
   - The scale survives a restart.
   - A corrupt or out-of-range value falls back to 150 %.
5. **Stale's own folders.** Its data folder, quarantine roots and install folder never show up, not even as uncertain.

### Task 1: Stale's own folders are never ghosts or leftovers (core)

**The live finding:**
- The ghost scan lists "Stale Uninstaller" as ProbablyUninstalled, and it starts checked.
- Its evidence:
  - `Folder ProgramData C:\ProgramData\Stale Uninstaller`
  - a BAM trace of `C:\Program Files\Stale Uninstaller\StaleUninstaller.exe`
- Removing it would quarantine Stale's own data folder.

**Files:**
- `stale-core/src/ghosts/{mod,folders,group}.rs`
- `leftovers/fs_scan.rs`
- `paths.rs` as needed

**What to drop:**
- Any evidence or leftover whose path equals or lies below one of Stale's own folders:
  - `paths::data_dir()`
  - each `<drive>:\Stale Uninstaller` quarantine payload root
  - the running exe's folder, from `std::env::current_exe()`'s parent. Put it behind a seam for tests.
  - `%ProgramFiles%\Stale Uninstaller`
- Usage traces of `StaleUninstaller.exe`.
- Registry keys under `Software\Stale Uninstaller`.

A ghost with no evidence left disappears.

**Tests:**
- `own_data_dir_is_never_ghost_evidence`
- `own_install_dir_and_exe_traces_are_dropped`
- `own_folders_are_never_leftovers`

**Live check:** run `dump ghosts` elevated. "Stale Uninstaller" must be gone.

**Commit:** `fix(core): Stale's own folders are never ghosts or leftovers`

### Task 2: UI scale, 150 % by default

**Files:**
- `stale-core/src/settings.rs`: add `ui_scale: Option<f32>`.
  - Missing, not finite, or outside 0.75..=3.0 → `None`.
  - A serde default keeps old files loading.
- `stale-gui`: `app.rs`, `main.rs`, `ui/toolbar.rs`, `strings.rs`

**Interfaces (stale-gui):**
- `pub const DEFAULT_UI_SCALE: f32 = 1.5`
- `pub const UI_SCALES: [f32; 5] = [1.0, 1.25, 1.5, 1.75, 2.0]`
- `StaleApp::ui_scale(&self) -> f32`
- `StaleApp::set_ui_scale_at(&mut self, scale: f32, path: &Path)`, which saves

**Behaviour:**
- **At start-up:** call `ctx.set_zoom_factor(scale)`.
- **Settings menu:** a submenu "Storlek" / "Size" with radio items 100 % … 200 %.
- **Keyboard zoom:** egui's built-in Ctrl + / − / 0 changes `ctx.zoom_factor()`.
  - The new value is saved, rounded to 2 decimals.
  - Save once per change, not every frame.
- **Start window:** about 1500×950 points before zoom, so the 150 % layout fits what the old size showed. egui keeps it within the monitor.

**Tests:**
- `ui_scale_defaults_to_150_percent`
- `out_of_range_or_corrupt_scale_falls_back`
- `set_ui_scale_saves`
- `old_settings_file_without_scale_loads`

**Commit:** `feat(gui): UI scale setting, 150 % by default`

### Task 3: Leftover popup and compact leftovers view

**Files:**
- new `stale-gui/src/ui/leftover_popup.rs`
- `ui/leftovers_view.rs`
- `leftover_sel.rs`
- `strings.rs`

**Interfaces:**
- `LeftoverGroup::toggle_all(&mut self)`
  - If any item is checked, uncheck all.
  - Otherwise check the `default_checked` items. If none is default-checked, check nothing.
- `LeftoverGroup::checked_count(&self) -> usize`
- `LeftoverGroup::checked_bytes(&self) -> u64`
- `leftover_popup::show(ctx: &egui::Context, id: egui::Id, title: &str, info: &[String], group: &mut LeftoverGroup, editable: bool) -> bool`
  - Returns false when the user closed it.
  - The `info` lines (warnings, for example) come first.
  - Then the files, registry and other sections with item checkboxes. Move today's `group_body` and `item_row` here.
  - The popup scrolls and is at most about 70 % of the screen height.
  - It has a Close button.

**Leftovers view:**
- **One row per group:**
  - a checkbox, checked when any item is checked
  - the name
  - "N av M" from `strings::items_checked(n, m)`
  - the size
- **Clicks:**
  - A row click calls `toggle_all`.
  - A double-click opens the popup for that group (`app.leftover_popup: Option<usize>`).
- **Unchanged:** the footer and the removal flow.

**Tests:**
- `toggle_all_unchecks_then_restores_defaults`
- `toggle_all_never_checks_low_items`
- `group_counts_and_bytes`
- `double_click_leaves_row_check_unchanged`: a pure helper in the style of `table.rs`'s `toggle_checked` that handles both click and double-click.

**Commit:** `feat(gui): compact leftovers list with a details popup`

### Task 4: Compact ghosts view with direct removal

**Files:**
- `ghost_sel.rs`
- `ui/ghosts_view.rs`
- `app.rs`
- `strings.rs`
- `README.md`

**Interfaces:**
- `GhostSelection { ghosts, checked, leftovers: Vec<LeftoverGroup> }`
  - `leftovers[i]` is built once in `new` from `to_leftovers(&ghosts[i], &tasks_root)`.
  - `tasks_root` comes in as a parameter, `new(ghosts, tasks_root: &Path)`, so tests need no live path.
  - `remove_by_key` keeps the three vectors aligned.
- `GhostSelection::removal_selection(&self) -> LeftoverSelection`: the checked ghosts' groups that have at least one checked item.
- `summary()` now returns `(ghosts_checked, items_checked, bytes)` over those groups.
- `StaleApp::remove_checked_ghosts(&mut self)`:
  1. Sets `leftovers = removal_selection()`, `leftovers_origin = View::Ghosts` and `view = View::Leftovers`.
  2. Calls `request_removal(live…)`.

  The leftovers view then shows the drive window, the progress and the summary as today, and "Tillbaka" returns to the ghosts view.

**View:**
- **Sections:** as today.
- **One row per ghost:**
  - checkbox, name, publisher (weak), assessment badge, size, last used
  - Behåll on the right
  - An uncertain ghost's reasons become a "⚠" with hover text in the row, not a separate line.
- **Clicks:**
  - A row click toggles the checkbox.
  - A double-click opens `leftover_popup`:
    - title: the ghost's name
    - `info`: the uncertainty text, plus the evidence lines that are not leftovers (usage traces)
- **Footer:**
  - The `ghost_summary` text.
  - A "Ta bort markerade…" / "Remove checked…" button, enabled when items_checked > 0 and no removal is running.
  - It opens a confirmation window:
    - Text: "Flytta {n} rester från {p} program ({size}) till karantänen? Du kan återställa dem från Karantän.", with an English equivalent.
    - Buttons: Avbryt / Ta bort. Ta bort calls `remove_checked_ghosts`.
  - "Visa rester för markerade" goes away.
- **README:** the ghosts feature line says that removal goes straight from the ghosts view and that a double-click shows the details.

**Tests:**
- `ghost_selection_builds_one_group_per_ghost`
- `removal_selection_skips_unchecked_and_empty`
- `usage_only_ghost_adds_nothing`
- `popup_edits_survive_and_are_removed`
- `remove_checked_ghosts_opens_leftovers_with_origin_ghosts`, in the `app.rs` tests. Use a fake volume check if `request_removal` needs one.
- Update the existing ghosts tests.

**Commit:** `feat(gui): compact ghost list, details popup and direct removal`
