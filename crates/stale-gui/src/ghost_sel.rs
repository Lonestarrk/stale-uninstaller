//! Which ghost programs the user has checked, and what each would remove (no egui).

use std::path::Path;

use stale_core::ghosts::{to_leftovers, Assessment, Ghost};

use crate::leftover_sel::{LeftoverGroup, LeftoverSelection};

/// The ghosts view's state; `checked[i]` and `leftovers[i]` belong to `ghosts[i]`.
#[derive(Clone, Debug, PartialEq)]
pub struct GhostSelection {
    pub ghosts: Vec<Ghost>,
    pub checked: Vec<bool>,
    /// What each ghost would remove, built once; the details popup edits these check marks.
    pub leftovers: Vec<LeftoverGroup>,
}

impl GhostSelection {
    /// Only [`Assessment::ProbablyUninstalled`] ghosts start checked; leftovers without an
    /// installation and uncertain ones are the user's call. `tasks_root` is where scheduled
    /// tasks live (see [`to_leftovers`]).
    pub fn new(ghosts: Vec<Ghost>, tasks_root: &Path) -> Self {
        let checked = ghosts.iter().map(|g| g.assessment == Assessment::ProbablyUninstalled).collect();
        let reports: Vec<_> = ghosts.iter().map(|g| (g.name.clone(), to_leftovers(g, tasks_root))).collect();
        let leftovers = LeftoverSelection::from_reports(&reports).groups;
        GhostSelection { ghosts, checked, leftovers }
    }

    /// The checked ghosts' groups that have at least one checked item, for the removal.
    pub fn removal_selection(&self) -> LeftoverSelection {
        let groups = self
            .leftovers
            .iter()
            .zip(&self.checked)
            .filter(|(g, c)| **c && g.checked_count() > 0)
            .map(|(g, _)| g.clone())
            .collect();
        LeftoverSelection { groups }
    }

    /// Checked ghosts that remove something, their checked items and the bytes those free.
    pub fn summary(&self) -> (usize, usize, u64) {
        let groups = self.removal_selection().groups;
        (groups.len(), groups.iter().map(LeftoverGroup::checked_count).sum(), groups.iter().map(LeftoverGroup::checked_bytes).sum())
    }

    /// Drops the ghost whose key is `key` and hands it back with its check mark and group
    /// (`None` when there is no such ghost).
    pub fn remove_by_key(&mut self, key: &str) -> Option<(Ghost, bool, LeftoverGroup)> {
        let i = self.ghosts.iter().position(|g| g.key == key)?;
        Some((self.ghosts.remove(i), self.checked.remove(i), self.leftovers.remove(i)))
    }

    /// Puts a ghost taken out by [`Self::remove_by_key`] back at row `at` (or last).
    pub fn insert(&mut self, at: usize, row: (Ghost, bool, LeftoverGroup)) {
        let at = at.min(self.ghosts.len());
        self.ghosts.insert(at, row.0);
        self.checked.insert(at, row.1);
        self.leftovers.insert(at, row.2);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use stale_core::ghosts::{Evidence, RootKind};
    use stale_core::leftovers::LeftoverKind;
    use stale_core::{Hive, RegKeyPath};

    use super::*;

    /// A ghost named `name` with one Program Files folder of `size` bytes and one orphan key.
    pub(crate) fn ghost(name: &str, assessment: Assessment, size: u64) -> Ghost {
        Ghost {
            name: name.to_string(),
            key: name.to_lowercase(),
            publisher: None,
            evidence: vec![
                Evidence::Folder {
                    path: PathBuf::from(format!(r"C:\Program Files\{name}")),
                    size: Some(size),
                    modified: None,
                    root_kind: RootKind::ProgramFiles,
                    holds_user_data: false,
                },
                Evidence::RegKeyOrphan { key: RegKeyPath { hive: Hive::LocalMachine, path: format!(r"SOFTWARE\{name}") }, shared: false },
            ],
            assessment,
            total_size: size,
            last_used: None,
            reasons: Vec::new(),
            note: None,
        }
    }

    /// A selection over `ghosts` with a fixed tasks folder, so no test reads the live one.
    pub(crate) fn selection_of(ghosts: Vec<Ghost>) -> GhostSelection {
        GhostSelection::new(ghosts, Path::new(r"C:\Windows\System32\Tasks"))
    }

    fn selection() -> GhostSelection {
        selection_of(vec![
            ghost("Alpha", Assessment::ProbablyUninstalled, 100),
            ghost("Beta", Assessment::Uncertain, 50),
            ghost("Gamma", Assessment::LeftoversOnly, 7),
        ])
    }

    fn paths(group: &LeftoverGroup) -> Vec<String> {
        group
            .items
            .iter()
            .map(|l| match &l.kind {
                LeftoverKind::Dir { path, .. } => path.display().to_string(),
                LeftoverKind::RegKey { key } => key.path.clone(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn only_probably_uninstalled_is_prechecked() {
        let sel = selection();
        assert_eq!(sel.checked, vec![true, false, false]);
        assert_eq!(sel.summary(), (1, 2, 100));
    }

    #[test]
    fn ghost_selection_builds_one_group_per_ghost() {
        let sel = selection();
        assert_eq!(sel.leftovers.len(), 3);
        let names: Vec<&str> = sel.leftovers.iter().map(|g| g.program_name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "Beta", "Gamma"]);
        assert_eq!(paths(&sel.leftovers[0]), vec![r"C:\Program Files\Alpha".to_string(), r"SOFTWARE\Alpha".to_string()]);
    }

    #[test]
    fn removal_selection_skips_unchecked_and_empty() {
        let mut sel = selection();
        sel.checked[2] = true;
        // Gamma has nothing ticked in its popup: it adds nothing.
        sel.leftovers[2].checked.iter_mut().for_each(|c| *c = false);
        let groups = sel.removal_selection().groups;
        assert_eq!(groups.iter().map(|g| g.program_name.as_str()).collect::<Vec<_>>(), vec!["Alpha"]);
        assert_eq!(sel.summary(), (1, 2, 100));
        sel.checked[0] = false;
        assert_eq!(sel.summary(), (0, 0, 0));
        assert!(sel.removal_selection().groups.is_empty());
    }

    #[test]
    fn usage_only_ghost_adds_nothing() {
        let mut g = ghost("Trace", Assessment::LeftoversOnly, 0);
        g.evidence = vec![Evidence::UsageTrace {
            exe: PathBuf::from(r"C:\Trace	race.exe"),
            last_run: chrono::DateTime::from_timestamp(1_792_000_000, 0).unwrap(),
            source: stale_core::usage::UsageSource::Prefetch,
        }];
        let mut sel = selection_of(vec![g]);
        assert!(sel.leftovers[0].items.is_empty());
        sel.checked[0] = true;
        assert_eq!(sel.summary(), (0, 0, 0));
        assert!(sel.removal_selection().groups.is_empty());
    }

    #[test]
    fn popup_edits_survive_and_are_removed() {
        let mut sel = selection();
        // The popup unticks Alpha's registry key; unticking and re-ticking the ghost keeps that.
        sel.leftovers[0].checked[1] = false;
        sel.checked[0] = false;
        sel.checked[0] = true;
        let groups = sel.removal_selection().groups;
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].checked, vec![true, false]);
        let items = sel.removal_selection().checked_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].1.len(), 1, "only the folder is removed");
    }

    #[test]
    fn remove_by_key_keeps_the_vectors_aligned() {
        let mut sel = selection();
        sel.checked[0] = false;
        sel.checked[2] = true;
        let row = sel.remove_by_key("beta").expect("found");
        assert_eq!(row.0.name, "Beta");
        assert_eq!(sel.ghosts.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), vec!["Alpha", "Gamma"]);
        assert_eq!(sel.checked, vec![false, true], "check marks stay with their ghosts");
        assert_eq!(sel.leftovers.iter().map(|g| g.program_name.as_str()).collect::<Vec<_>>(), vec!["Alpha", "Gamma"]);
        assert!(sel.remove_by_key("missing").is_none());
        assert_eq!(sel.ghosts.len(), 2);
        sel.insert(1, row);
        assert_eq!(sel.ghosts.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), vec!["Alpha", "Beta", "Gamma"]);
        assert_eq!(sel.leftovers[1].program_name, "Beta");
    }
}
