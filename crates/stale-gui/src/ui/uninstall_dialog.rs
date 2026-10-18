//! The uninstall queue window: the summary before starting, then the queue's progress and results.

use std::sync::atomic::Ordering;

use eframe::egui;
use stale_core::uninstall::{RestorePointStatus, UninstallMode};
use stale_core::{ProgramId, Scope};

use crate::app::{PendingQueue, StaleApp, View};
use crate::flow::{dependencies_acknowledged, FlowState, ItemState, Step};
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
    let window = egui::Window::new(strings::t().queue_title)
        .id(egui::Id::unique("queue_window"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]);
    let mut action = None;
    if let Some(pending) = app.pending.as_mut() {
        window.show(&ctx, |ui| action = summary(ui, pending));
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
        // Leaving the program list hides this window.
        Some(Action::ShowLeftovers) => app.open_leftovers_from_flow(),
        Some(Action::Requeue(id, mode)) => app.open_queue_dialog_for(vec![id], mode),
        Some(Action::Close) => app.close_flow(),
        None => {}
    }
}

/// The summary before starting: names, options, Avbryt / Starta.
fn summary(ui: &mut egui::Ui, pending: &mut PendingQueue) -> Option<Action> {
    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
        for entry in &mut pending.entries {
            if entry.scope == Scope::OtherUser {
                ui.label(strings::other_account_item(&entry.name, entry.account.as_deref()));
                ui.weak(entry.uninstall_path.as_deref().unwrap_or(strings::t().unknown_value));
            } else {
                ui.label(&entry.name);
            }
            if let Some(reason) = entry.dependency {
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(0xd0, 0x80, 0x20), strings::t().dependency_warning)
                        .on_hover_text(strings::dependency_reason_in(strings::lang(), reason));
                });
                ui.checkbox(&mut entry.acknowledged, strings::t().dependency_understand);
            }
        }
    });
    if pending.entries.iter().any(|e| e.scope == Scope::OtherUser) {
        ui.separator();
        ui.label(strings::t().other_account_queue);
    }
    ui.separator();
    ui.checkbox(&mut pending.restore_point, strings::t().opt_restore_point);
    let normal = pending.mode == UninstallMode::Normal;
    if !normal {
        pending.run_vendor = false;
    }
    ui.add_enabled(normal, egui::Checkbox::new(&mut pending.run_vendor, strings::t().opt_run_vendor));
    ui.separator();
    let mut action = None;
    ui.horizontal(|ui| {
        if ui.button(strings::t().cancel).clicked() {
            action = Some(Action::Cancel);
        }
        let flagged: Vec<(bool, bool)> = pending.entries.iter().map(|e| (e.dependency.is_some(), e.acknowledged)).collect();
        if ui.add_enabled(dependencies_acknowledged(&flagged), egui::Button::new(strings::t().start)).clicked() {
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
            (Step::RestorePoint, strings::t().step_restore_point),
            (Step::Vendor, strings::t().step_vendor),
            (Step::Leftovers, strings::t().step_leftovers),
            (Step::Done, strings::t().step_done),
        ];
        for (i, (step, label)) in steps.into_iter().enumerate() {
            if i > 0 {
                ui.weak(strings::t().step_separator);
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
        ui.label(format!("{}: {}", strings::t().restore_point_label, restore_point_text(status)));
    }
    egui::Grid::new("queue_items").num_columns(2).striped(true).show(ui, |ui| {
        for (id, name, state) in &flow.items {
            ui.label(name);
            ui.horizontal(|ui| {
                ui.label(state_text(state));
                if flow.finished && matches!(state, ItemState::NotUninstalled { .. }) {
                    for (label, mode) in [(strings::t().retry, UninstallMode::Normal), (strings::t().force_remove, UninstallMode::Forced)] {
                        let button = ui.add_enabled(can_requeue, egui::Button::new(label)).on_disabled_hover_text(strings::t().loading);
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
            ui.label(w.text());
        }
    }
    ui.separator();
    ui.horizontal(|ui| {
        if !flow.finished {
            ui.spinner();
            if flow.step == Step::Vendor
                && ui.button(strings::t().stop_waiting).on_hover_text(strings::t().stop_waiting_tip).clicked()
            {
                action = Some(Action::StopWaiting);
            }
            return;
        }
        if ui.add_enabled(!flow.reports.is_empty(), egui::Button::new(strings::t().show_leftovers)).clicked() {
            action = Some(Action::ShowLeftovers);
        }
        if ui.button(strings::t().close).clicked() {
            action = Some(Action::Close);
        }
    });
    action
}

fn restore_point_text(status: &RestorePointStatus) -> String {
    match status {
        RestorePointStatus::Created(_) => strings::t().rp_created.to_string(),
        RestorePointStatus::SkippedFrequency => strings::t().rp_skipped.to_string(),
        RestorePointStatus::Disabled => strings::t().rp_disabled.to_string(),
        RestorePointStatus::Failed(e) => format!("{}: {e}", strings::t().rp_failed),
    }
}

fn state_text(state: &ItemState) -> String {
    match state {
        ItemState::Pending => strings::t().state_pending.to_string(),
        ItemState::Running => strings::t().state_running.to_string(),
        ItemState::Uninstalled => strings::t().state_uninstalled.to_string(),
        ItemState::Forced => strings::t().state_forced.to_string(),
        ItemState::NotUninstalled { exit_code: Some(code) } => {
            format!("{} ({} {code})", strings::t().state_not_uninstalled, strings::t().exit_code)
        }
        ItemState::NotUninstalled { exit_code: None } => strings::t().state_not_uninstalled.to_string(),
        ItemState::Skipped => strings::t().state_skipped.to_string(),
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
        assert_eq!(state_text(&ItemState::Forced), "Tvingad");
        assert_eq!(restore_point_text(&RestorePointStatus::Failed("x".into())), "misslyckades: x");
        assert_eq!(restore_point_text(&RestorePointStatus::SkippedFrequency), "hoppades över (en punkt skapades nyligen)");
    }
}
