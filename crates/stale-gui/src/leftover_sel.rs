//! Which leftovers the user has checked, grouped per program (no egui).

use stale_core::leftovers::{Leftover, LeftoverReport};

/// The leftovers view's state: one group per program.
#[derive(Clone, Debug, PartialEq)]
pub struct LeftoverSelection {
    pub groups: Vec<LeftoverGroup>,
}

/// One program's leftovers; `checked[i]` belongs to `items[i]`.
#[derive(Clone, Debug, PartialEq)]
pub struct LeftoverGroup {
    pub program_name: String,
    pub items: Vec<Leftover>,
    pub checked: Vec<bool>,
}

impl LeftoverSelection {
    /// One group per report, each item checked unless its confidence is `Low`.
    pub fn from_reports(reports: &[(String, LeftoverReport)]) -> Self {
        let groups = reports
            .iter()
            .map(|(name, report)| LeftoverGroup {
                program_name: name.clone(),
                items: report.items.clone(),
                checked: report.items.iter().map(Leftover::default_checked).collect(),
            })
            .collect();
        LeftoverSelection { groups }
    }

    /// "Markera säkra": checks High and Medium items, unchecks Low ones.
    pub fn mark_safe(&mut self) {
        for g in &mut self.groups {
            g.checked = g.items.iter().map(Leftover::default_checked).collect();
        }
    }

    /// The checked items per program, leaving out programs with none checked.
    pub fn checked_items(&self) -> Vec<(String, Vec<Leftover>)> {
        self.groups
            .iter()
            .map(|g| (g.program_name.clone(), g.checked_iter().cloned().collect::<Vec<_>>()))
            .filter(|(_, items)| !items.is_empty())
            .collect()
    }

    /// Checked count, total count and the bytes the checked items free.
    pub fn summary(&self) -> (usize, usize, u64) {
        let checked = self.groups.iter().flat_map(LeftoverGroup::checked_iter);
        let (count, bytes) = checked.fold((0, 0), |(n, b), l| (n + 1, b + l.size().unwrap_or(0)));
        let total = self.groups.iter().map(|g| g.items.len()).sum();
        (count, total, bytes)
    }
}

impl LeftoverGroup {
    fn checked_iter(&self) -> impl Iterator<Item = &Leftover> {
        self.items.iter().zip(&self.checked).filter(|(_, c)| **c).map(|(l, _)| l)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use stale_core::leftovers::{Confidence, LeftoverKind};
    use stale_core::{Hive, RegKeyPath};

    use super::*;

    fn dir(path: &str, confidence: Confidence, size: u64) -> Leftover {
        Leftover { kind: LeftoverKind::Dir { path: PathBuf::from(path), size: Some(size) }, confidence, note: None }
    }

    fn key(path: &str, confidence: Confidence) -> Leftover {
        let key = RegKeyPath { hive: Hive::LocalMachine, path: path.to_string() };
        Leftover { kind: LeftoverKind::RegKey { key }, confidence, note: None }
    }

    fn report(items: Vec<Leftover>) -> LeftoverReport {
        LeftoverReport { items }
    }

    fn selection() -> LeftoverSelection {
        LeftoverSelection::from_reports(&[
            ("A".to_string(), report(vec![dir(r"C:\A", Confidence::High, 100), dir(r"C:\Users\x\A", Confidence::Low, 50)])),
            ("B".to_string(), report(vec![key(r"Software\B", Confidence::Medium), dir(r"C:\B", Confidence::Medium, 7)])),
        ])
    }

    #[test]
    fn defaults_follow_confidence() {
        let sel = selection();
        assert_eq!(sel.groups.len(), 2);
        assert_eq!(sel.groups[0].program_name, "A");
        assert_eq!(sel.groups[0].checked, vec![true, false]);
        assert_eq!(sel.groups[1].program_name, "B");
        assert_eq!(sel.groups[1].checked, vec![true, true]);
        assert_eq!(sel.groups[1].items.len(), 2);
    }

    #[test]
    fn mark_safe_sets_high_medium_true_low_false() {
        let mut sel = selection();
        sel.groups[0].checked = vec![false, true];
        sel.groups[1].checked = vec![false, false];
        sel.mark_safe();
        assert_eq!(sel.groups[0].checked, vec![true, false]);
        assert_eq!(sel.groups[1].checked, vec![true, true]);
    }

    #[test]
    fn summary_counts_checked_bytes() {
        let mut sel = selection();
        // Checked: A's High dir (100), B's key (no size) and B's dir (7).
        assert_eq!(sel.summary(), (3, 4, 107));
        sel.groups[0].checked[1] = true;
        assert_eq!(sel.summary(), (4, 4, 157));
        sel.groups[1].checked = vec![false, false];
        assert_eq!(sel.summary(), (2, 4, 150));
    }

    #[test]
    fn checked_items_groups_by_program() {
        let mut sel = selection();
        sel.groups[1].checked = vec![false, true];
        assert_eq!(
            sel.checked_items(),
            vec![
                ("A".to_string(), vec![dir(r"C:\A", Confidence::High, 100)]),
                ("B".to_string(), vec![dir(r"C:\B", Confidence::Medium, 7)]),
            ]
        );
        sel.groups[0].checked = vec![false, false];
        assert_eq!(sel.checked_items(), vec![("B".to_string(), vec![dir(r"C:\B", Confidence::Medium, 7)])]);
    }
}
