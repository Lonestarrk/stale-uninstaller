//! Left panel: last-used, drive and size filters with row counts.

use eframe::egui;

use crate::filter::{BucketCounts, Filters, SizeBucket, UsageBucket};
use crate::strings;

pub fn show(ui: &mut egui::Ui, filters: &mut Filters, counts: &BucketCounts) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.strong(strings::t().group_last_used);
        for (bucket, label, n) in [
            (UsageBucket::All, strings::t().bucket_all, counts.all),
            (UsageBucket::OverSixMonths, strings::t().bucket_over_six_months, counts.over_six_months),
            (UsageBucket::OverOneYear, strings::t().bucket_over_one_year, counts.over_one_year),
            (UsageBucket::NeverSeen, strings::t().bucket_never_seen, counts.never_seen),
        ] {
            entry(ui, &mut filters.usage, bucket, label, n);
        }

        ui.add_space(8.0);
        ui.strong(strings::t().group_drive);
        entry(ui, &mut filters.drive, None, strings::t().bucket_all, counts.all);
        for (&d, &n) in &counts.per_drive {
            entry(ui, &mut filters.drive, Some(d), &format!("{d}:"), n);
        }

        ui.add_space(8.0);
        ui.strong(strings::t().group_size);
        entry(ui, &mut filters.size, SizeBucket::All, strings::t().bucket_all, counts.all);
        entry(ui, &mut filters.size, SizeBucket::OverOneGb, strings::t().bucket_over_one_gb, counts.over_one_gb);
    });
}

/// A selectable "label (count)" entry that sets `current` to `value` when clicked.
fn entry<T: PartialEq>(ui: &mut egui::Ui, current: &mut T, value: T, label: &str, count: usize) {
    if ui.selectable_label(*current == value, format!("{label} ({count})")).clicked() {
        *current = value;
    }
}
