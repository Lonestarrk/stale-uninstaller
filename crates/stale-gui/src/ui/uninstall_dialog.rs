//! The uninstall queue window: the summary before starting, then the queue's progress and results.

use std::sync::atomic::Ordering;

use eframe::egui;
use stale_core::uninstall::{RestorePointStatus, UninstallMode};
use stale_core::ProgramId;

use crate::app::{PendingQueue, StaleApp, View};
use crate::flow::{FlowState, ItemState, Step};
use crate::strings;

/// What a click in the window asks for, applied after drawing.
enum Action {
    Cancel,
    Start,
    StopWaiting,
    ShowLeftovers,
    Requeue(ProgramId, UninstallMode),
    Close,
}

pub fn show(ui: &mut egui::Ui, app: &mut StaleApp) {
    let ctx = ui.ctx().clone();
    let window = egui::Window::new(strings::QUEUE_TITLE)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]);
    let mut action = None;
    if let Some(pending) = app.pending.as_mut() {
        let rows = &app.rows;
        window.show(&ctx, |ui| {
            let name = |id: &ProgramId| rows.iter().find(|r| &r.program.id == id).map(|r| r.program.name.as_str());
            action = summary(ui, pending, name);
        });
    } else if let (Some(flow), View::Programs) = (app.flow.as_ref(), app.view) {
        let can_requeue = !app.scanning;
        window.show(&ctx, |ui| action = progress(ui, flow, can_requeue));
    }
    match action {
        Some(Action::Cancel) => app.pending = None,
        Some(Action::Start) => app.start_queue(),
        Some(Action::StopWaiting) => {
            if let Some(flag) = &app.stop_waiting {
                flag.store(true, Ordering::Relaxed);
            }
        }
        // Task 11 renders the leftovers from `app.flow`; leaving the program list hides this window.
        Some(Action::ShowLeftovers) => app.view = View::Leftovers,
        Some(Action::Requeue(id, mode)) => app.open_queue_dialog_for(vec![id], mode),
        Some(Action::Close) => app.close_flow(),
        None => {}
    }
}

/// The summary before starting: names, options, Avbryt / Starta.
fn summary<'a>(
    ui: &mut egui::Ui,
    pending: &mut PendingQueue,
    name: impl Fn(&ProgramId) -> Option<&'a str>,
) -> Option<Action> {
    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
        for id in &pending.ids {
            ui.label(name(id).unwrap_or(&id.0));
        }
    });
    ui.separator();
    ui.checkbox(&mut pending.restore_point, strings::OPT_RESTORE_POINT);
    let normal = pending.mode == UninstallMode::Normal;
    if !normal {
        pending.run_vendor = false;
    }
    ui.add_enabled(normal, egui::Checkbox::new(&mut pending.run_vendor, strings::OPT_RUN_VENDOR));
    ui.separator();
    let mut action = None;
    ui.horizontal(|ui| {
        if ui.button(strings::CANCEL).clicked() {
            action = Some(Action::Cancel);
        }
        if ui.button(strings::START).clicked() {
            action = Some(Action::Start);
        }
    });
    action
}

/// The running or finished queue: step indicator, restore point, per-item states, warnings.
fn progress(ui: &mut egui::Ui, flow: &FlowState, can_requeue: bool) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        let steps = [
            (Step::RestorePoint, strings::STEP_RESTORE_POINT),
            (Step::Vendor, strings::STEP_VENDOR),
            (Step::Leftovers, strings::STEP_LEFTOVERS),
            (Step::Done, strings::STEP_DONE),
        ];
        for (i, (step, label)) in steps.into_iter().enumerate() {
            if i > 0 {
                ui.weak(strings::STEP_SEPARATOR);
            }
            if step == flow.step {
                ui.strong(label);
            } else {
                ui.weak(label);
            }
        }
    });
    ui.separator();
    if let Some(status) = &flow.restore_point {
        ui.label(format!("{}: {}", strings::RESTORE_POINT_LABEL, restore_point_text(status)));
    }
    egui::Grid::new("queue_items").num_columns(2).striped(true).show(ui, |ui| {
        for (id, name, state) in &flow.items {
            ui.label(name);
            ui.horizontal(|ui| {
                ui.label(state_text(state));
                if flow.finished && matches!(state, ItemState::NotUninstalled { .. }) {
                    for (label, mode) in [(strings::RETRY, UninstallMode::Normal), (strings::FORCE_REMOVE, UninstallMode::Forced)] {
                        let button = ui.add_enabled(can_requeue, egui::Button::new(label)).on_disabled_hover_text(strings::LOADING);
                        if button.clicked() {
                            action = Some(Action::Requeue(id.clone(), mode));
                        }
                    }
                }
            });
            ui.end_row();
        }
    });
    if !flow.warnings.is_empty() {
        ui.separator();
        for w in &flow.warnings {
            ui.label(w);
        }
    }
    ui.separator();
    ui.horizontal(|ui| {
        if !flow.finished {
            ui.spinner();
            if flow.step == Step::Vendor
                && ui.button(strings::STOP_WAITING).on_hover_text(strings::STOP_WAITING_TIP).clicked()
            {
                action = Some(Action::StopWaiting);
            }
            return;
        }
        if ui.add_enabled(!flow.reports.is_empty(), egui::Button::new(strings::SHOW_LEFTOVERS)).clicked() {
            action = Some(Action::ShowLeftovers);
        }
        if ui.button(strings::CLOSE).clicked() {
            action = Some(Action::Close);
        }
    });
    action
}

fn restore_point_text(status: &RestorePointStatus) -> String {
    match status {
        RestorePointStatus::Created(_) => strings::RP_CREATED.to_string(),
        RestorePointStatus::SkippedFrequency => strings::RP_SKIPPED.to_string(),
        RestorePointStatus::Disabled => strings::RP_DISABLED.to_string(),
        RestorePointStatus::Failed(e) => format!("{}: {e}", strings::RP_FAILED),
    }
}

fn state_text(state: &ItemState) -> String {
    match state {
        ItemState::Pending => strings::STATE_PENDING.to_string(),
        ItemState::Running => strings::STATE_RUNNING.to_string(),
        ItemState::Uninstalled => strings::STATE_UNINSTALLED.to_string(),
        ItemState::NotUninstalled { exit_code: Some(code) } => {
            format!("{} ({} {code})", strings::STATE_NOT_UNINSTALLED, strings::EXIT_CODE)
        }
        ItemState::NotUninstalled { exit_code: None } => strings::STATE_NOT_UNINSTALLED.to_string(),
        ItemState::Skipped => strings::STATE_SKIPPED.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_texts_are_swedish() {
        assert_eq!(state_text(&ItemState::NotUninstalled { exit_code: Some(1602) }), "Inte avinstallerat (kod 1602)");
        assert_eq!(state_text(&ItemState::NotUninstalled { exit_code: None }), "Inte avinstallerat");
        assert_eq!(state_text(&ItemState::Skipped), "Hoppades över");
        assert_eq!(restore_point_text(&RestorePointStatus::Failed("x".into())), "misslyckades: x");
        assert_eq!(restore_point_text(&RestorePointStatus::SkippedFrequency), "hoppades över (en punkt skapades nyligen)");
    }
}
