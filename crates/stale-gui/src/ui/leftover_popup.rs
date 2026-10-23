//! The popup that lists one program's (or ghost's) leftover items with a check box each.

use eframe::egui;
use stale_core::leftovers::{Confidence, Leftover, LeftoverKind};

use super::leftovers_view::{key_text, note_text};
use super::stale_color;
use crate::leftover_sel::LeftoverGroup;
use crate::{format, strings};

/// Shows the popup: the `info` lines first (warnings and the like), then the files, registry and
/// other items with check boxes (disabled unless `editable`), a scroll area of at most about 70 %
/// of the screen height and a Close button. Returns false when the user closed it.
pub fn show(ctx: &egui::Context, id: egui::Id, title: &str, info: &[String], group: &mut LeftoverGroup, editable: bool) -> bool {
    let mut open = true;
    let mut close = false;
    let max_height = ctx.content_rect().height() * 0.7;
    egui::Window::new(title)
        .id(id)
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(max_height).auto_shrink([true, true]).show(ui, |ui| {
                for line in info {
                    ui.colored_label(stale_color(ui.visuals()), line);
                }
                if !info.is_empty() {
                    ui.separator();
                }
                group_body(ui, group, editable);
            });
            ui.separator();
            if ui.button(strings::t().close).clicked() {
                close = true;
            }
        });
    open && !close
}

/// One program's leftovers in three sections; check boxes are disabled while a removal runs.
fn group_body(ui: &mut egui::Ui, group: &mut LeftoverGroup, editable: bool) {
    for (section, title) in
        [strings::t().section_files, strings::t().section_registry, strings::t().section_other].into_iter().enumerate()
    {
        let indices: Vec<usize> = (0..group.items.len()).filter(|&i| section_of(&group.items[i].kind) == section).collect();
        if indices.is_empty() {
            continue;
        }
        let bytes = indices.iter().filter_map(|&i| group.items[i].size()).sum();
        ui.strong(format!("{title} {}", strings::section_counts(indices.len(), &format::human_size(Some(bytes)))));
        for i in indices {
            item_row(ui, &group.items[i], &mut group.checked[i], editable);
        }
    }
}

fn item_row(ui: &mut egui::Ui, item: &Leftover, checked: &mut bool, editable: bool) {
    ui.horizontal(|ui| {
        ui.add_enabled(editable, egui::Checkbox::without_text(checked));
        ui.weak(kind_label(&item.kind));
        ui.label(item_text(&item.kind));
        if item.confidence == Confidence::Low {
            let warning = match &item.note {
                Some(note) => format!("{} {}", strings::t().warning_icon, note_text(note)),
                None => strings::t().warning_icon.to_string(),
            };
            ui.colored_label(stale_color(ui.visuals()), warning);
        }
        if let LeftoverKind::Dir { size, .. } | LeftoverKind::File { size, .. } = &item.kind {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format::human_size(*size));
            });
        }
    });
}

/// 0: folders and files, 1: registry, 2: everything else.
fn section_of(kind: &LeftoverKind) -> usize {
    match kind {
        LeftoverKind::Dir { .. } | LeftoverKind::File { .. } => 0,
        LeftoverKind::RegKey { .. } | LeftoverKind::RegValue { .. } => 1,
        LeftoverKind::Shortcut { .. } | LeftoverKind::Service { .. } | LeftoverKind::ScheduledTask { .. } => 2,
    }
}

fn kind_label(kind: &LeftoverKind) -> &'static str {
    match kind {
        LeftoverKind::Dir { .. } => strings::t().kind_dir,
        LeftoverKind::File { .. } => strings::t().kind_file,
        LeftoverKind::RegKey { .. } => strings::t().kind_regkey,
        LeftoverKind::RegValue { .. } => strings::t().kind_regvalue,
        LeftoverKind::Shortcut { .. } => strings::t().kind_shortcut,
        LeftoverKind::Service { .. } => strings::t().kind_service,
        LeftoverKind::ScheduledTask { .. } => strings::t().kind_task,
    }
}

/// The path, key or name that identifies a leftover.
fn item_text(kind: &LeftoverKind) -> String {
    match kind {
        LeftoverKind::Dir { path, .. } | LeftoverKind::File { path, .. } => path.display().to_string(),
        LeftoverKind::RegKey { key } => key_text(key),
        LeftoverKind::RegValue { key, name } => format!("{} → {name}", key_text(key)),
        LeftoverKind::Shortcut { lnk, .. } => lnk.display().to_string(),
        LeftoverKind::Service { name, .. } | LeftoverKind::ScheduledTask { name, .. } => name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use stale_core::{Hive, RegKeyPath};

    use super::*;

    #[test]
    fn registry_items_show_hive_abbreviation() {
        let key = |hive| RegKeyPath { hive, path: r"Software\Foo".into() };
        assert_eq!(item_text(&LeftoverKind::RegKey { key: key(Hive::LocalMachine) }), r"HKLM\Software\Foo");
        assert_eq!(item_text(&LeftoverKind::RegKey { key: key(Hive::CurrentUser) }), r"HKCU\Software\Foo");
        assert_eq!(item_text(&LeftoverKind::RegKey { key: key(Hive::Users) }), r"HKU\Software\Foo");
        assert_eq!(
            item_text(&LeftoverKind::RegValue { key: key(Hive::CurrentUser), name: "Foo".into() }),
            r"HKCU\Software\Foo → Foo"
        );
        assert_eq!(kind_label(&LeftoverKind::RegValue { key: key(Hive::Users), name: String::new() }), strings::t().kind_regvalue);
    }

    #[test]
    fn sections_split_files_registry_other() {
        let path = PathBuf::from(r"C:\x");
        assert_eq!(section_of(&LeftoverKind::File { path: path.clone(), size: None }), 0);
        assert_eq!(section_of(&LeftoverKind::RegKey { key: RegKeyPath { hive: Hive::LocalMachine, path: "x".into() } }), 1);
        assert_eq!(section_of(&LeftoverKind::Service { name: "s".into(), image: path }), 2);
    }
}
