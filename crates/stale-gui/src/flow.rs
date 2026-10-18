//! Progress of a running uninstall queue, folded from `UninstallEvent`s (no egui).

use stale_core::leftovers::LeftoverReport;
use stale_core::uninstall::{RestorePointStatus, UninstallEvent, VendorResult};
use stale_core::ProgramId;

use crate::warning::{Msg, Warning};

/// True when every flagged item (`(is_dependency, acknowledged)`) has been acknowledged, which
/// is what enables Starta in the summary dialog.
pub fn dependencies_acknowledged(items: &[(bool, bool)]) -> bool {
    items.iter().all(|&(dependency, acknowledged)| !dependency || acknowledged)
}

/// The queue's current phase, shown as the dialog's step indicator.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    RestorePoint,
    Vendor,
    Leftovers,
    Done,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ItemState {
    Pending,
    Running,
    Uninstalled,
    /// Removed without its own uninstaller (forced mode, or none was found): only the leftover
    /// scan ran.
    Forced,
    NotUninstalled { exit_code: Option<i32> },
    /// The queue ended before reaching this item.
    Skipped,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FlowState {
    /// Id, name and state of each queued program, in queue order.
    pub items: Vec<(ProgramId, String, ItemState)>,
    pub step: Step,
    pub restore_point: Option<RestorePointStatus>,
    /// Leftover reports keyed by item index.
    pub reports: Vec<(usize, LeftoverReport)>,
    pub warnings: Vec<Warning>,
    pub finished: bool,
}

impl FlowState {
    pub fn new(items: &[(ProgramId, String)]) -> Self {
        FlowState {
            items: items.iter().map(|(id, name)| (id.clone(), name.clone(), ItemState::Pending)).collect(),
            step: Step::RestorePoint,
            restore_point: None,
            reports: Vec::new(),
            warnings: Vec::new(),
            finished: false,
        }
    }

    pub fn apply(&mut self, ev: UninstallEvent) {
        match ev {
            UninstallEvent::RestorePoint(status) => {
                self.restore_point = Some(status);
                self.step = Step::RestorePoint;
            }
            UninstallEvent::Started { index } => {
                self.set_state(index, ItemState::Running);
                self.step = Step::Vendor;
            }
            UninstallEvent::Vendor { index, result } => match result {
                VendorResult::Uninstalled { .. } => self.set_state(index, ItemState::Uninstalled),
                VendorResult::NotUninstalled { exit_code } => {
                    self.set_state(index, ItemState::NotUninstalled { exit_code })
                }
                // Forced removal follows: the leftover scan is still to come.
                VendorResult::NoUninstaller => {}
            },
            UninstallEvent::Leftovers { index, report } => {
                // Still running here means no vendor uninstaller ran: a forced removal.
                if self.items.get(index).is_some_and(|(_, _, s)| *s == ItemState::Running) {
                    self.set_state(index, ItemState::Forced);
                }
                self.reports.push((index, report));
                self.step = Step::Leftovers;
            }
            UninstallEvent::NotUninstalled { index, exit_code } => {
                self.set_state(index, ItemState::NotUninstalled { exit_code })
            }
            UninstallEvent::NoUninstaller { index } => {
                let name = self.items.get(index).map_or("", |(_, name, _)| name.as_str());
                self.warnings.push(Warning::NoUninstaller(name.to_string()));
            }
            // Only the GUI worker sends it, with the reason the uninstall thread did not start.
            UninstallEvent::Warning(detail) => self.warnings.push(Warning::Detail(Msg::UninstallThreadFailed, detail)),
            UninstallEvent::QueueDone => self.finish(),
        }
    }

    /// Marks the queue finished; items it never reached become `Skipped`.
    pub fn finish(&mut self) {
        for (_, _, state) in &mut self.items {
            if *state == ItemState::Pending {
                *state = ItemState::Skipped;
            }
        }
        self.finished = true;
        self.step = Step::Done;
    }

    /// True once the queue is done and every item that was not left installed has a report.
    pub fn all_reports_in(&self) -> bool {
        self.finished
            && self.items.iter().enumerate().all(|(i, (_, _, state))| {
                matches!(state, ItemState::NotUninstalled { .. }) || self.reports.iter().any(|(r, _)| *r == i)
            })
    }

    fn set_state(&mut self, index: usize, state: ItemState) {
        if let Some(item) = self.items.get_mut(index) {
            item.2 = state;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starta_disabled_until_dependency_acknowledged() {
        assert!(dependencies_acknowledged(&[]));
        assert!(dependencies_acknowledged(&[(false, false)]));
        assert!(!dependencies_acknowledged(&[(false, false), (true, false)]));
        assert!(!dependencies_acknowledged(&[(true, true), (true, false)]));
        assert!(dependencies_acknowledged(&[(true, true), (false, false)]));
    }

    fn flow(n: usize) -> FlowState {
        let items: Vec<(ProgramId, String)> =
            (0..n).map(|i| (ProgramId(format!("id{i}")), format!("App {i}"))).collect();
        FlowState::new(&items)
    }

    fn states(f: &FlowState) -> Vec<ItemState> {
        f.items.iter().map(|(_, _, s)| s.clone()).collect()
    }

    #[test]
    fn apply_tracks_item_states_and_reports() {
        let mut f = flow(4);
        assert_eq!(states(&f), vec![ItemState::Pending; 4]);

        f.apply(UninstallEvent::Started { index: 0 });
        assert_eq!(f.items[0].2, ItemState::Running);
        assert_eq!(f.step, Step::Vendor);
        f.apply(UninstallEvent::Vendor { index: 0, result: VendorResult::Uninstalled { exit_code: 0 } });
        assert_eq!(f.items[0].2, ItemState::Uninstalled);
        f.apply(UninstallEvent::Leftovers { index: 0, report: LeftoverReport::default() });
        assert_eq!(f.step, Step::Leftovers);
        assert_eq!(f.items[0].2, ItemState::Uninstalled, "a report does not change the state");

        f.apply(UninstallEvent::Started { index: 1 });
        f.apply(UninstallEvent::Vendor { index: 1, result: VendorResult::NotUninstalled { exit_code: Some(1602) } });
        f.apply(UninstallEvent::NotUninstalled { index: 1, exit_code: Some(1602) });
        assert_eq!(f.items[1].2, ItemState::NotUninstalled { exit_code: Some(1602) });

        f.apply(UninstallEvent::Started { index: 2 });
        f.apply(UninstallEvent::Vendor { index: 2, result: VendorResult::NoUninstaller });
        assert_eq!(f.items[2].2, ItemState::Running, "no uninstaller: leftovers still to come");
        f.apply(UninstallEvent::NoUninstaller { index: 2 });
        f.apply(UninstallEvent::Warning("w".into()));
        f.apply(UninstallEvent::Leftovers { index: 2, report: LeftoverReport::default() });
        assert_eq!(f.items[2].2, ItemState::Forced, "removed without its uninstaller");
        assert!(!f.finished);

        f.apply(UninstallEvent::QueueDone);
        assert!(f.finished);
        assert_eq!(f.step, Step::Done);
        assert_eq!(
            f.warnings,
            vec![Warning::NoUninstaller("App 2".into()), Warning::Detail(Msg::UninstallThreadFailed, "w".into())]
        );
        assert_eq!(f.warnings[0].text(), "App 2: ingen avinstallerare hittades, tvingad borttagning");
        assert_eq!(f.reports.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![0, 2]);
        assert_eq!(f.items[3].2, ItemState::Skipped, "never started");
    }

    #[test]
    fn all_reports_in_ignores_not_uninstalled_items() {
        let mut f = flow(2);
        f.apply(UninstallEvent::Started { index: 0 });
        f.apply(UninstallEvent::NotUninstalled { index: 0, exit_code: None });
        f.apply(UninstallEvent::Started { index: 1 });
        f.apply(UninstallEvent::Leftovers { index: 1, report: LeftoverReport::default() });
        assert!(!f.all_reports_in(), "not finished yet");
        f.apply(UninstallEvent::QueueDone);
        assert!(f.all_reports_in());

        let mut missing = flow(2);
        missing.apply(UninstallEvent::Started { index: 0 });
        missing.apply(UninstallEvent::Leftovers { index: 0, report: LeftoverReport::default() });
        missing.apply(UninstallEvent::QueueDone);
        assert!(!missing.all_reports_in(), "item 1 was skipped and has no report");
    }

    #[test]
    fn restore_point_status_recorded() {
        let mut f = flow(1);
        assert_eq!(f.restore_point, None);
        f.apply(UninstallEvent::RestorePoint(RestorePointStatus::SkippedFrequency));
        assert_eq!(f.restore_point, Some(RestorePointStatus::SkippedFrequency));
        assert_eq!(f.step, Step::RestorePoint);
    }
}
