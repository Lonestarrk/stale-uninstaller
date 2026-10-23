//! The "Sök rester" view's state (no egui): the space scan's entries with their check marks, the
//! drill-down window's folder, and what a removal from either moves to the quarantine.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use stale_core::fsys::FsReader;
use stale_core::ghosts::RootKind;
use stale_core::leftovers::{Confidence, Leftover, LeftoverKind, LeftoverReport};
use stale_core::pathutil::norm;
use stale_core::space::{removal, removal_items, BlockReason, Child, Removal, SpaceEntry, SpaceKind, WinItem};

use crate::leftover_sel::LeftoverSelection;
use crate::strings::{self, texts, Lang};
use crate::format;

/// The space view's entries; `checked[i]` and `measured[i]` belong to `entries[i]`. Nothing
/// starts checked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpaceSelection {
    pub entries: Vec<SpaceEntry>,
    pub checked: Vec<bool>,
    /// The entry's size has arrived (it may still be unknown).
    pub measured: Vec<bool>,
    /// Each entry's [`norm`]ed path, the sort's tie-breaker.
    keys: Vec<String>,
    /// The row order, until a size changes it.
    order: Option<Vec<usize>>,
}

impl SpaceSelection {
    pub fn new(entries: Vec<SpaceEntry>) -> Self {
        let n = entries.len();
        let keys = entries.iter().map(|e| norm(&e.path)).collect();
        SpaceSelection { entries, checked: vec![false; n], measured: vec![false; n], keys, order: None }
    }

    /// Entry `i`'s measured size; an index out of range is ignored.
    pub fn set_size(&mut self, i: usize, size: Option<u64>, partial: bool) {
        if let Some(e) = self.entries.get_mut(i) {
            e.size = size;
            e.partial = partial;
            self.measured[i] = true;
            self.order = None;
        }
    }

    pub fn measured_count(&self) -> usize {
        self.measured.iter().filter(|m| **m).count()
    }

    /// The row order: largest first, unknown and not yet measured sizes last, then by path.
    /// Sorted again only after a size has changed.
    pub fn order(&mut self) -> &[usize] {
        let (entries, keys) = (&self.entries, &self.keys);
        self.order.get_or_insert_with(|| {
            let mut idx: Vec<usize> = (0..entries.len()).collect();
            idx.sort_by(|&a, &b| {
                let by_size = match (entries[a].size, entries[b].size) {
                    (Some(x), Some(y)) => y.cmp(&x),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => Ordering::Equal,
                };
                by_size.then_with(|| keys[a].cmp(&keys[b]))
            });
            idx
        })
    }

    /// Only entries that go through the quarantine have a check box; Windows' own space hogs
    /// are cleaned through Windows.
    pub fn checkable(&self, i: usize) -> bool {
        self.entries.get(i).is_some_and(|e| removal(&e.kind) == Removal::Quarantine)
    }

    /// A row click or its check box; does nothing for a row without a check box.
    pub fn toggle(&mut self, i: usize) {
        if self.checkable(i) {
            self.checked[i] = !self.checked[i];
        }
    }

    /// Checked entries and the bytes they free (unknown sizes count as 0).
    pub fn summary(&self) -> (usize, u64) {
        self.checked_entries().fold((0, 0), |(n, b), e| (n + 1, b + e.size.unwrap_or(0)))
    }

    fn checked_entries(&self) -> impl Iterator<Item = &SpaceEntry> {
        self.entries.iter().zip(&self.checked).enumerate().filter(|(i, (_, c))| **c && self.checkable(*i)).map(|(_, (e, _))| e)
    }

    /// The checked entries that go through the quarantine, for [`removal_selection_of`] on a
    /// worker.
    pub fn checked_snapshot(&self) -> Vec<SpaceEntry> {
        self.checked_entries().cloned().collect()
    }

    /// What removing the checked entries moves; see [`removal_selection_of`].
    pub fn removal_selection(&self, fs: &dyn FsReader, now: DateTime<Utc>) -> LeftoverSelection {
        removal_selection_of(fs, &self.checked_snapshot(), now)
    }

    /// `listed` (a [`removal_selection_of`] of an earlier snapshot) cut down to the entries that
    /// are still checked now: what "Ta bort" removes. An entry checked after the listing is not
    /// added; the user has not seen it in the confirmation.
    pub fn still_checked(&self, listed: &LeftoverSelection) -> LeftoverSelection {
        let names: std::collections::HashSet<String> = self.checked_entries().map(|e| e.path.display().to_string()).collect();
        LeftoverSelection { groups: listed.groups.iter().filter(|g| names.contains(&g.program_name)).cloned().collect() }
    }
}

/// What removing `entries` moves, one group per entry named by its path: the entry itself, or a
/// temp folder's old files ([`removal_items`]). Windows-cleanup entries yield nothing. Reads the
/// file system for temp folders, so it runs on a worker.
pub fn removal_selection_of(fs: &dyn FsReader, entries: &[SpaceEntry], now: DateTime<Utc>) -> LeftoverSelection {
    let reports: Vec<(String, LeftoverReport)> = entries
        .iter()
        .filter(|e| removal(&e.kind) == Removal::Quarantine)
        .map(|e| (e.path.display().to_string(), LeftoverReport { items: removal_items(fs, e, now) }))
        .filter(|(_, r)| !r.items.is_empty())
        .collect();
    LeftoverSelection::from_reports(&reports)
}

/// What one row of the space view shows.
#[derive(Clone, Debug, PartialEq)]
pub struct SpaceRow {
    pub name: String,
    /// The folder the entry is in.
    pub location: String,
    pub kind: &'static str,
    pub size: String,
    /// A check box; otherwise the row offers "Öppna Windows rensning".
    pub checkbox: bool,
    /// The catalog sentence, the ambiguity note or "Okänd – avgör själv".
    pub explanation: Option<String>,
}

/// Entry `entry`'s row in `lang`; `measured` says whether its size has arrived. Ambiguity notes
/// are worded in the current language.
pub fn space_row_in(lang: Lang, entry: &SpaceEntry, measured: bool) -> SpaceRow {
    let s = texts(lang);
    let name = entry.path.file_name().map_or_else(|| entry.path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let location = entry.path.parent().map(|p| p.display().to_string()).unwrap_or_default();
    let kind = match entry.kind {
        SpaceKind::Orphan(RootKind::ProgramFiles) => s.space_kind_program_files,
        SpaceKind::Orphan(RootKind::ProgramData) => s.space_kind_program_data,
        SpaceKind::Orphan(RootKind::AppData | RootKind::LocalPrograms) => s.space_kind_appdata,
        SpaceKind::DriveRoot if entry.is_dir => s.space_kind_unknown_folder,
        SpaceKind::DriveRoot => s.space_kind_unknown_file,
        SpaceKind::Windows(_) => s.space_kind_windows,
    };
    let size = if !measured {
        s.space_pending.to_string()
    } else {
        size_text_in(lang, entry.size, entry.partial)
    };
    let explanation = match entry.kind {
        SpaceKind::Windows(item) => Some(windows_text_in(lang, item, entry.modified)),
        SpaceKind::DriveRoot => Some(s.space_unknown_decide.to_string()),
        SpaceKind::Orphan(_) => entry.note.map(|n| crate::ui::leftovers_view::note_text(n).to_string()),
    };
    SpaceRow { name, location, kind, size, checkbox: removal(&entry.kind) == Removal::Quarantine, explanation }
}

/// A size, "minst …" when it is a lower bound, "–" when unknown.
pub fn size_text_in(lang: Lang, size: Option<u64>, partial: bool) -> String {
    let text = format::human_size_in(lang, size);
    if partial && size.is_some() { strings::at_least_in(lang, &text) } else { text }
}

/// The catalog sentence of a Windows space hog; `Windows.old` names the date it was saved.
pub fn windows_text_in(lang: Lang, item: WinItem, modified: Option<DateTime<Utc>>) -> String {
    let s = texts(lang);
    match item {
        WinItem::WindowsOld => {
            let saved = modified.map(|m| format::ymd(Some(format::local_date(m))));
            strings::windows_old_text_in(lang, saved.as_deref())
        }
        WinItem::WindowsBt => s.win_bt.to_string(),
        WinItem::WindowsWs => s.win_ws.to_string(),
        WinItem::WindowsTmp => s.win_tmp.to_string(),
        WinItem::UpdateDownload => s.win_update.to_string(),
        WinItem::WindowsTemp => s.win_temp.to_string(),
        WinItem::UserTemp => s.win_user_temp.to_string(),
        WinItem::MemoryDump => s.win_memory_dump.to_string(),
        WinItem::Minidumps => s.win_minidumps.to_string(),
        WinItem::LiveKernelReports => s.win_live_kernel.to_string(),
    }
}

/// Why a drill-down row has no check box, in `lang`.
pub fn block_text_in(lang: Lang, reason: BlockReason) -> &'static str {
    let s = texts(lang);
    match reason {
        BlockReason::System => s.block_system,
        BlockReason::InstalledProgram => s.block_installed,
        BlockReason::OtherProfileRoot => s.block_profile,
        BlockReason::Own => s.block_own,
        BlockReason::Kept => s.block_kept,
    }
}

/// The drill-down window: the opened entry, the folder shown and its children.
#[derive(Clone, Debug, PartialEq)]
pub struct BrowseState {
    /// The opened entry, the breadcrumb's first step.
    pub root: PathBuf,
    /// The folder shown.
    pub dir: PathBuf,
    /// `None` while the folder is being read.
    pub children: Option<Vec<Child>>,
    /// How many children were left out.
    pub more: usize,
    pub checked: Vec<bool>,
}

impl BrowseState {
    pub fn new(root: PathBuf) -> Self {
        BrowseState { dir: root.clone(), root, children: None, more: 0, checked: Vec::new() }
    }

    /// The breadcrumb from the opened entry down to the shown folder: (label, folder).
    pub fn crumbs(&self) -> Vec<(String, PathBuf)> {
        let mut out = vec![(self.root.display().to_string(), self.root.clone())];
        let Ok(rest) = self.dir.strip_prefix(&self.root) else { return out };
        let mut at = self.root.clone();
        for c in rest.components() {
            at.push(c);
            out.push((c.as_os_str().to_string_lossy().into_owned(), at.clone()));
        }
        out
    }

    /// Shows `dir` (read again); the check marks are dropped.
    pub fn go_to(&mut self, dir: PathBuf) {
        self.dir = dir;
        self.children = None;
        self.more = 0;
        self.checked.clear();
    }

    /// The listing of `dir` arrived; ignored (false) when another folder is shown by now.
    pub fn set_children(&mut self, dir: &Path, children: Vec<Child>, more: usize) -> bool {
        if norm(dir) != norm(&self.dir) {
            return false;
        }
        self.checked = vec![false; children.len()];
        self.children = Some(children);
        self.more = more;
        true
    }

    /// A row click or its check box; blocked children have none.
    pub fn toggle(&mut self, i: usize) {
        let open = self.children.as_ref().and_then(|c| c.get(i)).is_some_and(|c| c.blocked.is_none());
        if open {
            self.checked[i] = !self.checked[i];
        }
    }

    fn checked_children(&self) -> impl Iterator<Item = &Child> {
        self.children.iter().flatten().zip(&self.checked).filter(|(c, on)| **on && c.blocked.is_none()).map(|(c, _)| c)
    }

    /// Checked children and the bytes they free.
    pub fn summary(&self) -> (usize, u64) {
        self.checked_children().fold((0, 0), |(n, b), c| (n + 1, b + c.size.unwrap_or(0)))
    }

    /// The checked children as one group (named by the folder) of file and folder leftovers.
    pub fn removal_selection(&self) -> LeftoverSelection {
        let items: Vec<Leftover> = self
            .checked_children()
            .map(|c| {
                let (path, size) = (c.path.clone(), c.size);
                let kind = if c.is_dir { LeftoverKind::Dir { path, size } } else { LeftoverKind::File { path, size } };
                Leftover { kind, confidence: Confidence::Medium, note: None }
            })
            .collect();
        if items.is_empty() {
            return LeftoverSelection { groups: Vec::new() };
        }
        LeftoverSelection::from_reports(&[(self.dir.display().to_string(), LeftoverReport { items })])
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use stale_core::fsys::FakeFs;

    pub(crate) fn entry(path: &str, kind: SpaceKind, size: Option<u64>) -> SpaceEntry {
        SpaceEntry { path: PathBuf::from(path), is_dir: true, kind, size, partial: false, modified: None, note: None }
    }

    fn sized(entries: Vec<SpaceEntry>) -> SpaceSelection {
        let mut sel = SpaceSelection::new(entries.iter().map(|e| SpaceEntry { size: None, ..e.clone() }).collect());
        for (i, e) in entries.iter().enumerate() {
            sel.set_size(i, e.size, false);
        }
        sel
    }

    #[test]
    fn space_rows_sort_by_size_unknown_last() {
        let mut sel = sized(vec![
            entry(r"D:\Small", SpaceKind::DriveRoot, Some(10)),
            entry(r"D:\Unknown", SpaceKind::DriveRoot, None),
            entry(r"D:\Big", SpaceKind::DriveRoot, Some(500)),
            entry(r"D:\Also unknown", SpaceKind::DriveRoot, None),
            entry(r"D:\Mid", SpaceKind::DriveRoot, Some(100)),
        ]);
        assert_eq!(sel.order(), [2, 4, 0, 3, 1]);
        // A size arriving later moves its row up.
        sel.set_size(1, Some(1000), true);
        assert_eq!(sel.order(), [1, 2, 4, 0, 3], "the cached order is sorted again");
        assert!(sel.entries[1].partial);
        assert_eq!(sel.measured_count(), 5);
        sel.set_size(99, Some(1), false);
        assert_eq!(sel.entries.len(), 5, "an unknown index is ignored");
    }

    #[test]
    fn windows_cleanup_rows_have_no_checkbox() {
        let old = SpaceEntry {
            modified: DateTime::from_timestamp(1_790_000_000, 0),
            ..entry(r"C:\Windows.old", SpaceKind::Windows(WinItem::WindowsOld), Some(9))
        };
        let mut sel = sized(vec![old.clone(), entry(r"C:\Users\A\AppData\Local\Temp", SpaceKind::Windows(WinItem::UserTemp), Some(1))]);
        assert!(!sel.checkable(0) && sel.checkable(1));
        sel.toggle(0);
        sel.toggle(1);
        assert_eq!(sel.checked, vec![false, true]);
        let row = space_row_in(Lang::Sv, &old, true);
        assert!(!row.checkbox);
        assert_eq!(row.kind, "Windows");
        assert_eq!((row.name.as_str(), row.location.as_str()), ("Windows.old", r"C:\"));
        let date = format::ymd(Some(format::local_date(old.modified.unwrap())));
        assert_eq!(row.explanation, Some(format!("Den förra Windows-installationen, sparad {date}. Behövs bara om du vill gå tillbaka.")));
        let en = space_row_in(Lang::En, &old, true);
        assert_eq!(en.explanation, Some(format!("The previous Windows installation, saved {date}. Only needed to go back to it.")));
        for item in [WinItem::WindowsBt, WinItem::WindowsWs, WinItem::WindowsTmp, WinItem::UpdateDownload, WinItem::WindowsTemp] {
            assert!(!space_row_in(Lang::Sv, &entry(r"C:\x", SpaceKind::Windows(item), None), true).checkbox, "{item:?}");
        }
        for item in [WinItem::UserTemp, WinItem::MemoryDump, WinItem::Minidumps, WinItem::LiveKernelReports] {
            assert!(space_row_in(Lang::Sv, &entry(r"C:\x", SpaceKind::Windows(item), None), true).checkbox, "{item:?}");
        }
    }

    #[test]
    fn rows_name_kind_size_and_explanation() {
        let mut foton = entry(r"D:\Foton", SpaceKind::DriveRoot, Some(3 * 1024 * 1024 * 1024));
        let row = space_row_in(Lang::Sv, &foton, false);
        assert_eq!(row.size, "…", "not measured yet");
        assert_eq!((row.kind, row.explanation.as_deref()), ("Okänd mapp", Some("Okänd – avgör själv")));
        assert!(row.checkbox);
        foton.partial = true;
        assert_eq!(space_row_in(Lang::Sv, &foton, true).size, "minst 3,0 GB");
        assert_eq!(space_row_in(Lang::En, &foton, true).size, "at least 3.0 GB");
        assert_eq!(space_row_in(Lang::En, &foton, true).explanation.as_deref(), Some("Unknown – your call"));
        foton.size = None;
        assert_eq!(space_row_in(Lang::Sv, &foton, true).size, "–");
        let iso = SpaceEntry { is_dir: false, ..entry(r"D:\big.iso", SpaceKind::DriveRoot, Some(1)) };
        assert_eq!(space_row_in(Lang::En, &iso, true).kind, "Unknown file");
        let orphan = entry(r"C:\Program Files\Gone", SpaceKind::Orphan(RootKind::ProgramFiles), Some(1));
        let row = space_row_in(Lang::Sv, &orphan, true);
        assert_eq!((row.kind, row.explanation), ("Rest i Program Files", None));
        let noted = SpaceEntry { note: Some(stale_core::ghosts::group::REASON_SIBLING_OWNED), ..orphan };
        assert_eq!(space_row_in(Lang::Sv, &noted, true).explanation.as_deref(), Some(strings::t().note_sibling_owned));
        assert_eq!(space_row_in(Lang::Sv, &entry(r"C:\ProgramData\X", SpaceKind::Orphan(RootKind::ProgramData), None), true).kind, "Rest i ProgramData");
    }

    #[test]
    fn checked_entries_become_one_group_each() {
        let fs = FakeFs::new().dir(r"D:\Foton", 9).file(r"D:\big.iso", 5);
        let iso = SpaceEntry { is_dir: false, ..entry(r"D:\big.iso", SpaceKind::DriveRoot, Some(5)) };
        let mut sel = sized(vec![entry(r"D:\Foton", SpaceKind::DriveRoot, Some(9)), iso, entry(r"C:\Windows.old", SpaceKind::Windows(WinItem::WindowsOld), Some(7))]);
        assert!(sel.removal_selection(&fs, Utc::now()).groups.is_empty(), "nothing is pre-checked");
        sel.toggle(0);
        sel.toggle(1);
        // Even a forced check mark on a Windows entry removes nothing.
        sel.checked[2] = true;
        assert_eq!(sel.summary(), (2, 14));
        let groups = sel.removal_selection(&fs, Utc::now()).groups;
        let got: Vec<(String, Vec<LeftoverKind>, Vec<bool>)> =
            groups.into_iter().map(|g| (g.program_name, g.items.into_iter().map(|l| l.kind).collect(), g.checked)).collect();
        assert_eq!(
            got,
            vec![
                (r"D:\Foton".to_string(), vec![LeftoverKind::Dir { path: PathBuf::from(r"D:\Foton"), size: Some(9) }], vec![true]),
                (r"D:\big.iso".to_string(), vec![LeftoverKind::File { path: PathBuf::from(r"D:\big.iso"), size: Some(5) }], vec![true]),
            ]
        );
    }

    #[test]
    fn confirmation_removes_only_entries_still_checked() {
        let fs = FakeFs::new().dir(r"D:\Foton", 9).dir(r"D:\ISO", 5).dir(r"D:\Other", 1);
        let mut sel = sized(vec![
            entry(r"D:\Foton", SpaceKind::DriveRoot, Some(9)),
            entry(r"D:\ISO", SpaceKind::DriveRoot, Some(5)),
            entry(r"D:\Other", SpaceKind::DriveRoot, Some(1)),
        ]);
        sel.toggle(0);
        sel.toggle(1);
        let listed = removal_selection_of(&fs, &sel.checked_snapshot(), Utc::now());
        assert_eq!(listed.groups.len(), 2);
        // While the confirmation is open: Foton is unchecked, Other is checked.
        sel.toggle(0);
        sel.toggle(2);
        let names: Vec<String> = sel.still_checked(&listed).groups.into_iter().map(|g| g.program_name).collect();
        assert_eq!(names, vec![r"D:\ISO".to_string()], "unchecked is not removed, unseen is not added");
    }

    fn child(path: &str, is_dir: bool, size: Option<u64>, blocked: Option<BlockReason>) -> Child {
        Child { path: PathBuf::from(path), is_dir, size, partial: false, blocked }
    }

    #[test]
    fn browse_checked_children_become_file_and_dir_leftovers() {
        let mut b = BrowseState::new(PathBuf::from(r"D:\Data"));
        assert!(b.children.is_none(), "loading");
        b.set_children(
            Path::new(r"D:\Data"),
            vec![
                child(r"D:\Data\Big", true, Some(500), None),
                child(r"D:\Data\movie.mkv", false, Some(100), None),
                child(r"D:\Data\Apps", true, Some(50), Some(BlockReason::InstalledProgram)),
                child(r"D:\Data\tiny.txt", false, Some(1), None),
            ],
            3,
        );
        b.toggle(0);
        b.toggle(1);
        b.toggle(2);
        assert_eq!(b.checked, vec![true, true, false, false], "a blocked child cannot be checked");
        b.checked[2] = true;
        assert_eq!(b.summary(), (2, 600), "nor counted or removed when forced");
        let sel = b.removal_selection();
        assert_eq!(sel.groups.len(), 1);
        let g = &sel.groups[0];
        assert_eq!(g.program_name, r"D:\Data");
        assert_eq!(
            g.items.iter().map(|l| (l.kind.clone(), l.confidence)).collect::<Vec<_>>(),
            vec![
                (LeftoverKind::Dir { path: PathBuf::from(r"D:\Data\Big"), size: Some(500) }, Confidence::Medium),
                (LeftoverKind::File { path: PathBuf::from(r"D:\Data\movie.mkv"), size: Some(100) }, Confidence::Medium),
            ]
        );
        assert_eq!(g.checked, vec![true, true]);
        b.checked = vec![false; 4];
        assert!(b.removal_selection().groups.is_empty());
    }

    #[test]
    fn browse_breadcrumb_and_stale_listings() {
        let mut b = BrowseState::new(PathBuf::from(r"D:\Data"));
        b.go_to(PathBuf::from(r"D:\Data\Sub\Deeper"));
        let crumbs = b.crumbs();
        assert_eq!(
            crumbs,
            vec![
                (r"D:\Data".to_string(), PathBuf::from(r"D:\Data")),
                ("Sub".to_string(), PathBuf::from(r"D:\Data\Sub")),
                ("Deeper".to_string(), PathBuf::from(r"D:\Data\Sub\Deeper")),
            ]
        );
        // The listing of the folder the user just left is dropped.
        assert!(!b.set_children(Path::new(r"D:\Data"), vec![child(r"D:\Data\x", false, Some(1), None)], 0));
        assert!(b.children.is_none());
        assert!(b.set_children(Path::new(r"d:\data\sub\deeper"), vec![child(r"D:\Data\Sub\Deeper\x", false, Some(1), None)], 0));
        assert_eq!(b.checked, vec![false]);
        b.toggle(0);
        b.go_to(crumbs[1].1.clone());
        assert!(b.children.is_none() && b.checked.is_empty() && b.more == 0);
    }

    #[test]
    fn block_reasons_are_worded_in_both_languages() {
        assert_eq!(block_text_in(Lang::Sv, BlockReason::InstalledProgram), "Ett installerat program finns här");
        assert_eq!(block_text_in(Lang::En, BlockReason::Own), "Stale Uninstaller's own folder");
        assert_eq!(block_text_in(Lang::Sv, BlockReason::Kept), "På listan över behållna – tas inte bort här");
        assert_eq!(strings::more_children_in(Lang::Sv, 312), "… 312 till");
        assert_eq!(strings::more_children_in(Lang::En, 1), "… 1 more");
        assert_eq!(strings::space_remove_confirm_in(Lang::Sv, 2, "3 MB"), "Flytta 2 mappar och filer (3 MB) till karantänen? Du kan återställa dem från Karantän.");
        assert_eq!(strings::space_summary_in(Lang::En, 1, "5 MB"), "1 checked · 5 MB");
    }
}
