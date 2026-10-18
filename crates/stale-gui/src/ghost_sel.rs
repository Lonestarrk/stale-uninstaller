//! Which ghost programs the user has checked (no egui).

use stale_core::ghosts::{to_leftovers, Assessment, Ghost};
use stale_core::leftovers::LeftoverReport;

/// The ghosts view's state; `checked[i]` belongs to `ghosts[i]`.
#[derive(Clone, Debug, PartialEq)]
pub struct GhostSelection {
    pub ghosts: Vec<Ghost>,
    pub checked: Vec<bool>,
}

impl GhostSelection {
    /// Only [`Assessment::ProbablyUninstalled`] ghosts start checked; leftovers without an
    /// installation and uncertain ones are the user's call.
    pub fn new(ghosts: Vec<Ghost>) -> Self {
        let checked = ghosts.iter().map(|g| g.assessment == Assessment::ProbablyUninstalled).collect();
        GhostSelection { ghosts, checked }
    }

    /// `(name, leftovers)` of each checked ghost, for the leftovers view.
    pub fn checked_reports(&self) -> Vec<(String, LeftoverReport)> {
        let tasks_root = stale_core::paths::tasks_root();
        self.checked_ghosts().map(|g| (g.name.clone(), to_leftovers(g, &tasks_root))).collect()
    }

    /// Checked count, total count and the checked ghosts' folder bytes.
    pub fn summary(&self) -> (usize, usize, u64) {
        let (count, bytes) = self.checked_ghosts().fold((0, 0), |(n, b), g| (n + 1, b + g.total_size));
        (count, self.ghosts.len(), bytes)
    }

    /// Drops the ghost whose key is `key` (no-op when there is none).
    pub fn remove_by_key(&mut self, key: &str) {
        if let Some(i) = self.ghosts.iter().position(|g| g.key == key) {
            self.ghosts.remove(i);
            self.checked.remove(i);
        }
    }

    fn checked_ghosts(&self) -> impl Iterator<Item = &Ghost> {
        self.ghosts.iter().zip(&self.checked).filter(|(_, c)| **c).map(|(g, _)| g)
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
                Evidence::RegKeyOrphan { key: RegKeyPath { hive: Hive::LocalMachine, path: format!(r"SOFTWARE\{name}") } },
            ],
            assessment,
            total_size: size,
            last_used: None,
            reasons: Vec::new(),
            note: None,
        }
    }

    fn selection() -> GhostSelection {
        GhostSelection::new(vec![
            ghost("Alpha", Assessment::ProbablyUninstalled, 100),
            ghost("Beta", Assessment::Uncertain, 50),
            ghost("Gamma", Assessment::LeftoversOnly, 7),
        ])
    }

    #[test]
    fn only_probably_uninstalled_is_prechecked() {
        let sel = selection();
        assert_eq!(sel.checked, vec![true, false, false]);
        assert_eq!(sel.summary(), (1, 3, 100));
    }

    #[test]
    fn checked_reports_convert_only_checked() {
        let mut sel = selection();
        sel.checked[1] = false;
        let reports = sel.checked_reports();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].0, "Alpha");
        let paths: Vec<String> = reports[0]
            .1
            .items
            .iter()
            .map(|l| match &l.kind {
                LeftoverKind::Dir { path, .. } => path.display().to_string(),
                LeftoverKind::RegKey { key } => key.path.clone(),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(paths, vec![r"C:\Program Files\Alpha".to_string(), r"SOFTWARE\Alpha".to_string()]);
        assert_eq!(sel.summary(), (1, 3, 100));
    }

    #[test]
    fn remove_by_key() {
        let mut sel = selection();
        sel.checked[0] = false;
        sel.checked[2] = true;
        sel.remove_by_key("beta");
        assert_eq!(sel.ghosts.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), vec!["Alpha", "Gamma"]);
        assert_eq!(sel.checked, vec![false, true], "check marks stay with their ghosts");
        sel.remove_by_key("missing");
        assert_eq!(sel.ghosts.len(), 2);
    }
}
